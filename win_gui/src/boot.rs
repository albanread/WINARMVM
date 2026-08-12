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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use macvm::embed::{FatalMode, VmHandle, VmMetrics};
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

/// The primary's most recent `VmMetrics`, and when it was taken.
///
/// WG4 D2. Shared rather than messaged, and the distinction is the design: a
/// metric is a SAMPLE, not a REQUEST. Routing a 4 Hz push over the
/// `#uiReq`/`#uiReply` seam would load the very channel Do It's latency
/// depends on, to carry data nobody is waiting on. `cocoa_gui` publishes into
/// a `Mutex` for the same reason and this is its twin.
///
/// The `Instant` is what makes STALENESS observable: a display fed by hand
/// cannot go stale, and one read off a live primary can — if the primary is
/// wedged in a long doit, or dead, the numbers stop moving and the UI is the
/// only place that can notice.
pub type MetricsSnapshot = Arc<Mutex<Option<(VmMetrics, Instant)>>>;

/// The two wired VMs, handed back to `main` for the window + pump phase.
///
/// `ui_worker` runs on the caller's (main) thread; the primary lives on
/// `primary_thread`.
pub struct WiredVms {
    /// The UI VM: pinned to main, holds every handle, pumps every message.
    pub ui_worker: VmHandle,
    /// Everything about the primary that a RESTART replaces.
    pub link: PrimaryLink,
}

/// The primary, as something that can be replaced without touching the window.
///
/// WG7-3. Every field here is re-minted by a restart and nothing outside is:
/// the id changes (a fresh registry), the inbox changes (a fresh channel), the
/// thread changes, and the metrics slot changes. Keeping them in ONE struct is
/// what makes "swap the primary" a single assignment rather than four, and
/// what stops a caller keeping half of the old one — which is exactly the
/// defect class `47_worker.mst` warns about for in-flight `#uiReq`s.
pub struct PrimaryLink {
    /// The UI worker's id in the primary's registry.
    pub hosted_id: u32,
    /// The channel the UI worker drains for primary→UI traffic.
    pub inbox: HostedInbox,
    /// The background thread hosting the primary VM.
    ///
    /// `Option` because a restart TAKES it to join it. Before WG7-3 this was
    /// never joined at all — the primary parked for the process lifetime and
    /// the handle was a liveness token. A restart is the first thing that ever
    /// needs the old one to be finished before the new one starts, because two
    /// primaries serving one UI worker would race over its registry entry.
    pub thread: Option<JoinHandle<()>>,
    /// WG4 D2: the primary's own metrics, republished every beat.
    pub metrics: MetricsSnapshot,
    /// Set by the UI thread, read by the primary's beat. The ONLY way the
    /// dispatch loop ever ends — it is otherwise `loop {}` by design.
    pub stop: Arc<AtomicBool>,
}

/// Wire the two VMs and hand back a live [`WiredVms`]. Runs on the thread that
/// will own the message loop; blocks until the primary signals ready.
///
/// `wake` is called from the PRIMARY's thread whenever it sends to the UI
/// worker, so it must be cheap and thread-safe — `request_drain(hwnd)` is both
/// (one `PostMessageW` behind the posted-latch).
/// Spawn a primary and park until it is serving. The half of the handshake a
/// RESTART repeats — extracted rather than duplicated, so a restart cannot
/// drift from a boot and quietly produce a differently-wired VM.
#[allow(clippy::type_complexity)]
fn spawn_primary(
    world_dir: PathBuf,
    wake: InboxWakeFn,
) -> Result<(u32, HostedInbox, InboxSender, JoinHandle<()>, MetricsSnapshot, Arc<AtomicBool>), VmError>
{
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, HostedInbox, InboxSender), VmError>>();
    let metrics: MetricsSnapshot = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let world_for_primary = world_dir.clone();
    let metrics_for_primary = metrics.clone();
    let stop_for_primary = stop.clone();
    let thread = std::thread::Builder::new()
        .name("macvm-winui-primary".into())
        .spawn(move || {
            primary_thread_main(
                world_for_primary,
                wake,
                ready_tx,
                metrics_for_primary,
                stop_for_primary,
            )
        })
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
    Ok((hosted_id, hosted_inbox, to_primary, thread, metrics, stop))
}

/// Replace the primary WITHOUT touching the window, the UI VM's heap, or any
/// view.
///
/// WG7-3, and the slice `docs/sprints/sprint_wg7_detail.md` orders first. Its
/// contract is two-sided and both halves matter: the new primary must be
/// indistinguishable from a fresh boot TO THE WORLD (a class defined at
/// runtime is gone; `world/user_*.mst` is back), and the restart must be
/// invisible TO THE WINDOW (no rebuild, no view state lost, `viewBuildCount`
/// unchanged).
///
/// Order is load-bearing. The old primary is stopped and JOINED before the new
/// one is spawned, because two primaries serving one UI worker would both hold
/// registry entries for it and race over its inbox. Joining is also what makes
/// the old VM's heap actually go away rather than linger behind a detached
/// thread.
///
/// The UI worker is then RE-ROLED onto the new link. `install_worker_role`
/// overwrites the worker state, which is a caller bug on a primary and exactly
/// right here — a worker's role is precisely "which primary do I answer to".
pub fn restart_primary(
    ui_worker: &mut VmHandle,
    link: &mut PrimaryLink,
    world_dir: PathBuf,
    wake: InboxWakeFn,
) -> Result<(), VmError> {
    // 1. Ask the old beat to finish, and WAIT for it. `pumpInbox:` returns
    //    every beat by design (that is the seam D1 kept for exactly this), so
    //    the flag is seen within one beat rather than never.
    link.stop.store(true, Ordering::Relaxed);
    if let Some(t) = link.thread.take() {
        let _ = t.join();
    }

    // 2. A fresh primary, wired the same way a boot wires one.
    let (hosted_id, inbox, to_primary, thread, metrics, stop) =
        spawn_primary(world_dir, wake)?;

    // 3. Point the UI worker at it. Anything still holding the OLD id or the
    //    OLD sender is now stale by construction, which is the point: a
    //    half-swapped link is the failure mode this struct exists to prevent.
    ui_worker.install_worker_role(hosted_id, to_primary);

    link.hosted_id = hosted_id;
    link.inbox = inbox;
    link.thread = Some(thread);
    link.metrics = metrics;
    link.stop = stop;
    Ok(())
}

pub fn handshake_wire_vms(
    world_dir: PathBuf,
    winui_list: PathBuf,
    ui_fatal_mode: FatalMode,
    wake: InboxWakeFn,
) -> Result<WiredVms, VmError> {
    let (hosted_id, hosted_inbox, to_primary, primary_thread, metrics, stop) =
        spawn_primary(world_dir.clone(), wake)?;

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
        link: PrimaryLink {
            hosted_id,
            inbox: hosted_inbox,
            thread: Some(primary_thread),
            metrics,
            stop,
        },
    })
}

fn primary_thread_main(
    world_dir: PathBuf,
    wake: InboxWakeFn,
    ready_tx: mpsc::Sender<Result<(u32, HostedInbox, InboxSender), VmError>>,
    metrics: MetricsSnapshot,
    stop: Arc<AtomicBool>,
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
    let wake_for_debugger = wake.clone();
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

    // WG7-1: the GUI debugger frontend, installed on THIS generation. Re-run
    // for every restart (WG7-3), exactly as the Mac re-installs on respawn —
    // a frontend belongs to the VM that halts, and a restarted primary is a
    // different VM.
    crate::debugger::install(&mut primary, wake_for_debugger);

    // WG7-2: join the Monitor's roster. `monitor_register` REUSES a dead slot
    // with the same label, so a restarted primary (WG7-3) keeps ONE row and
    // flips it back to alive on its first beat rather than accreting a
    // tombstone per restart — which is exactly what makes the two slices
    // compose instead of fighting.
    let monitor = macvm::embed::monitor_register("primary".into(), "primary");

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
        // WG7-3: the ONE way this loop ends. It is `loop {}` by design — the
        // primary parks for the process lifetime — so a restart needs a seam,
        // and the beat is it: `pumpInbox:` returns every BEAT_MS whether or not
        // mail arrived, which D1 kept deliberately so a supervisor could
        // observe liveness. That same property is what bounds how long a
        // restart waits.
        if stop.load(Ordering::Relaxed) {
            eprintln!("macvm-winui: primary stopping after {beats} beats (restart)");
            // The row goes dead rather than silently freezing: a Monitor that
            // showed a stopped VM as merely quiet would be lying with a
            // straight face.
            monitor.mark_dead();
            return;
        }
        beats += 1;
        // WG4 D2: republish this VM's own metrics for the UI to read. Taken
        // HERE, on the primary's thread, between beats — the only place the
        // primary's heap can be read without racing the mutator.
        let m = primary.metrics();
        if let Ok(mut slot) = metrics.lock() {
            *slot = Some((m, Instant::now()));
        }
        // WG7-2: and into the ROSTER, which is a different audience. The
        // metrics slot above feeds one cluster in the bar; this feeds the
        // Monitor's row for this VM. Publishing here rather than from the UI
        // thread is the whole safety argument — a VM's heap is read by its own
        // thread, between beats, and never by anyone else.
        //
        // It also makes the AGE column mean something: a primary wedged inside
        // a long doit stops publishing, and a growing age while still marked
        // alive is precisely "stuck in guest code" — the busy signal for a VM
        // whose pump blocks inside `exec`.
        monitor.publish(m);

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
