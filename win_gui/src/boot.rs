//! The parked-main boot handshake (WG4 D1, `docs/sprints/sprint_wg4_detail.md`).
//!
//! The Windows twin of `cocoa_gui/src/boot.rs`, and deliberately its close
//! copy: the chicken-and-egg is identical — *the primary registers the UI
//! worker, but the UI worker must run on the thread the process started on,
//! because that is the thread Windows demands for window creation, the message
//! loop, and every `SendMessage`* — so the resolution is identical too. Boot
//! the UI worker VM **in place on main** and let Rust own the loop.
//!
//! This module holds the machine-checkable half: the two-VM wiring up to (but
//! not entering) the message loop. It touches **no Win32** beyond the wake
//! function it is handed, so [`handshake_wire_vms`] is unit-testable headless —
//! no window, no pump — which is what WG4's D1 gate asks of it.
//!
//! Boot sequence (design §2.2 commitment 2, §3's shape):
//! 1. *(main)* Spawn the **primary VM on a background thread**, then park
//!    awaiting its "ready" signal — [`handshake_wire_vms`] does both (the park
//!    is the blocking `recv`).
//! 2. *(background)* The primary boots the world, becomes a primary
//!    ([`VmHandle::set_worker_boot`]), **registers the UI worker as an
//!    externally-hosted peer** (no `thread::spawn` — its thread is main), and
//!    signals ready with the wiring.
//! 3. *(main)* Boots the **UI worker VM in place**, loads the conditional
//!    `winui.list` layer, takes on its Worker role so its replies reach the
//!    primary, and hands the caller a live [`WiredVms`].
//! 4. *(main, Rust)* `main.rs` opens the window from Smalltalk and pumps.
//!
//! **Why this exists at all**, in the author's words: *the split is core to the
//! design — we have a UI VM that does not block, because the VMs doing work are
//! not the UI thread.* A long doit on the primary must leave the window
//! pumping; that is the property, and D1's gate measures it rather than
//! asserting it.
//!
//! **The wake.** A hosted worker's `InboxWakeFn` fires whenever the primary
//! sends to it, and its job is to get the UI thread to *look at the inbox*. On
//! Windows that is precisely WG3's drain request — `request_drain(hwnd)` posts
//! `WM_APP_DRAIN` behind its own latch, so a burst of primary traffic
//! coalesces into one wake exactly as a burst of `WM_SIZE` does. The reply pump
//! is then just another drain callee, which is what keeps §2.4a's rule intact
//! with a second VM in the picture: **the door records, the drain works**, and
//! a reply continuation never runs inside a handler.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;

use macvm::embed::{FatalMode, VmHandle};
use macvm::runtime::workers::{HostedInbox, InboxSender, InboxWakeFn};
use macvm::runtime::{VmError, VmOptions};

/// Address-space reservation for each VM, in MiB (reservation, not commitment —
/// the world boots inside a small committed working set). Overridable per
/// `VmOptions::from_env`'s `MACVM_HEAP`.
const DEFAULT_HEAP_MIB: usize = 512;

/// Both VMs boot the same base world with the same options — the UI worker then
/// loads `winui.list` on top. Kept in one place so the two heaps cannot drift
/// apart in size or JIT policy and make a bug look like an architecture
/// difference.
fn vm_options() -> VmOptions {
    VmOptions {
        heap_mib: DEFAULT_HEAP_MIB,
        ..VmOptions::from_env()
    }
}

/// The two wired VMs, handed back to `main` for the window + pump phase.
///
/// `ui_worker` runs on the caller's (main) thread; the primary lives on
/// `primary_thread`.
pub struct WiredVms {
    /// The UI VM: pinned to main, holds every handle, pumps every message.
    pub ui_worker: VmHandle,
    /// The UI worker's id in the primary's registry.
    pub hosted_id: u32,
    /// The channel the UI worker drains for primary→UI traffic.
    pub hosted_inbox: HostedInbox,
    /// The (detached) background thread hosting the persistent primary VM. It
    /// is never joined — it parks for the process lifetime — so this is a
    /// liveness token, not a handle anyone waits on.
    pub primary_thread: JoinHandle<()>,
}

/// Wire the two VMs and hand back a live [`WiredVms`]. Runs on the thread that
/// will own the message loop; blocks until the primary signals ready.
///
/// `wake` is called from the PRIMARY's thread whenever it sends to the UI
/// worker, so it must be cheap and thread-safe — `request_drain(hwnd)` is both
/// (one `PostMessageW` behind the posted-latch).
pub fn handshake_wire_vms(
    world_dir: PathBuf,
    winui_list: PathBuf,
    ui_fatal_mode: FatalMode,
    wake: InboxWakeFn,
) -> Result<WiredVms, VmError> {
    // The primary's "ready" payload: the UI worker id + the channel to drain +
    // the reply link back to the primary. All `Send` — the primary mints them
    // on its thread, main receives them on the pump thread.
    #[allow(clippy::type_complexity)]
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, HostedInbox, InboxSender), VmError>>();

    let world_for_primary = world_dir.clone();
    let primary_thread = std::thread::Builder::new()
        .name("macvm-winui-primary".into())
        .spawn(move || primary_thread_main(world_for_primary, wake, ready_tx))
        .map_err(|e| VmError {
            msg: format!("could not spawn the primary VM thread: {e}"),
        })?;

    // Park until the primary is up and serving (or reports a boot failure, or
    // dies before signalling).
    let (hosted_id, hosted_inbox, to_primary) = match ready_rx.recv() {
        Ok(Ok(payload)) => payload,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(VmError {
                msg: "the primary VM thread died before signalling ready".into(),
            })
        }
    };

    // Boot the UI worker VM IN PLACE on THIS thread — boot must run on the
    // driving thread, because its foreign-fault handler (P2's VEH) and its
    // recovery slot are thread-scoped.
    let mut ui_worker = VmHandle::boot(vm_options(), &world_dir).map_err(|e| VmError {
        msg: format!("UI worker boot failed: {}", e.msg),
    })?;
    // On the main thread a true fatal (heap exhaustion, stack overflow) must
    // exit the PROCESS, not tear main down into a headless zombie with a
    // window still on screen. `boot` armed `ExitThread`; flip it now. (Tests
    // pass `ExitThread` to stay harness-safe.)
    macvm::embed::set_fatal_mode(ui_fatal_mode);
    // The conditional Windows GUI world layer, loaded ONLY here — the CLI, the
    // base test suite and the primary itself carry none of it. Commitment 3:
    // the GUI is its own body of Smalltalk, loadable and browsable on its own
    // terms.
    ui_worker.load_list(&winui_list).map_err(|e| VmError {
        msg: format!("loading {} failed: {}", winui_list.display(), e.msg),
    })?;
    // Take on the Worker role so the UI worker's future `reply:`/`send:` reach
    // the primary — the same wiring a spawned worker installs for itself.
    ui_worker.install_worker_role(hosted_id, to_primary);

    Ok(WiredVms {
        ui_worker,
        hosted_id,
        hosted_inbox,
        primary_thread,
    })
}

/// The primary VM's thread: boot the world, become a primary, register the UI
/// worker, signal ready, then hold the VM alive for the process lifetime.
fn primary_thread_main(
    world_dir: PathBuf,
    wake: InboxWakeFn,
    ready_tx: mpsc::Sender<Result<(u32, HostedInbox, InboxSender), VmError>>,
) {
    // The persistent primary — the environment's state, the VM a user would
    // call "their image". `ExitThread` (boot's default) is right here: a
    // supervisor respawns the primary on a fatal doit, and the window is
    // expected to survive that (the CG9 restart-in-place behaviour §2.5 says
    // the Windows shell owes from day one).
    let mut primary = match VmHandle::boot(vm_options(), &world_dir) {
        Ok(h) => h,
        Err(e) => {
            let _ = ready_tx.send(Err(VmError {
                msg: format!("primary boot failed: {}", e.msg),
            }));
            return;
        }
    };
    // Installing a worker-boot fn makes this VM the PRIMARY (creates its inbox
    // + registry) — required before `register_hosted_worker`, and it lets the
    // primary spawn compute workers later.
    let boot_world = world_dir.clone();
    primary.set_worker_boot(Arc::new(move || VmHandle::boot(vm_options(), &boot_world)));

    // Register the UI worker as an externally-hosted peer — no `thread::spawn`,
    // because its thread is main and it is already there. WG1's Δ 1 is the
    // warning this call answers: minting a hosted entry requires a primary to
    // host it, and until this sprint there was none.
    let Some((id, hosted_inbox, to_primary)) = primary.register_hosted_worker(wake) else {
        let _ = ready_tx.send(Err(VmError {
            msg: "register_hosted_worker failed (not a primary, or the fleet is at its cap)".into(),
        }));
        return;
    };

    if ready_tx.send(Ok((id, hosted_inbox, to_primary))).is_err() {
        return; // main gone — nothing to serve
    }

    // Boot connectivity poke: exercise the primary→UI link and its wake once,
    // right after registration, so broken wiring fails fast and loudly rather
    // than at the first real request. Empty bytes = a bare nudge.
    primary.send_to_worker(id, 0, Vec::new());

    let trace = std::env::var("MACVM_WINUI_PRIMARY_TRACE").is_ok();
    eprintln!("macvm-winui: primary serving (trace={trace})");
    // The dispatch loop: one `Worker pumpInbox:` beat per iteration, forever.
    //
    // `pumpInbox:` SLEEPS IN THE CHANNEL for up to `BEAT_MS` and returns as soon
    // as an envelope arrives — the OS wake is the router, so this loop costs
    // nothing while idle and adds no latency when a request lands. It returns
    // to Rust every beat rather than looping inside the guest, which is what
    // gives a future supervisor a seam to observe liveness and respawn through
    // (a `runLoopWhile:` that never returned would give it none).
    //
    // This is the half that makes the split real rather than structural: the
    // primary is now SERVING, so a `#uiReq` shipped from the UI VM is answered
    // here, on this thread — and the UI thread is free to keep pumping while it
    // happens. That is §2.2's whole point, and D1's second gate claim.
    let mut beats: u64 = 0;
    loop {
        beats += 1;
        // WG4 D1 bring-up trace: `MACVM_WINUI_PRIMARY_TRACE=1` reports what the
        // beat actually saw. Temporary, and cheap enough to leave until D1 has
        // its gate — "the primary is serving" is otherwise unobservable from
        // outside this thread, and guessing about it cost an hour.
        if trace {
            let served = primary
                .eval("Worker pumpInbox: 250.")
                .unwrap_or_else(|e| format!("<err {e}>"));
            if served.trim() != "false" || beats <= 3 {
                eprintln!("macvm-winui: primary beat {beats} -> {}", served.trim());
            }
            continue;
        }
        if let Err(e) = primary.exec(&format!("Worker pumpInbox: {BEAT_MS}.")) {
            // An ordinary guest `error:` inside a served request is recovered
            // by the default policy and never reaches here; anything that does
            // is worth saying out loud rather than spinning silently.
            eprintln!("macvm-winui: primary beat: {e}");
            std::thread::sleep(std::time::Duration::from_millis(BEAT_MS));
        }
    }
}

/// The primary's dispatch-loop beat, in milliseconds. It is not a poll
/// interval — `pumpInbox:` returns immediately when mail arrives — it is the
/// idle cadence at which the loop returns to Rust so a supervisor can observe
/// liveness, and the cadence on which service deadlines are swept.
const BEAT_MS: u64 = 250;
