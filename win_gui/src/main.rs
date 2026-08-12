//! `macvm-winui` — the host for MACVM's Windows-native Smalltalk UI.
//!
//! The architecture this answers to, in the author's own words
//! (`docs/win_gui_design.md` §2.2):
//!
//! > **UI VM on the UI thread, messaging to a Smalltalk independent GUI
//! > layer. All in Smalltalk, with COM and API.**
//!
//! So this file is the *thin* half, and its thinness is the deliverable. It
//! boots one VM on the process's real main thread, layers `world/winui.list`
//! on top of the base world, asks Smalltalk to open a window, and then does
//! exactly one thing forever: pump messages. Every Win32 call that makes,
//! styles, sizes, titles or closes a window is in `world/91_winui_shell.mst`,
//! reached through the P5 FFI resolver — this crate registers no class,
//! creates no window, calls no DWM function and asks for no DPI.
//!
//! ## D1 — who owns what
//!
//! | concern | owner |
//! |---|---|
//! | window class, creation, styles, title, DPI, Mica/dark-mode | **Smalltalk** (`WinShell`) |
//! | the pump (`GetMessageW`/`TranslateMessage`/`DispatchMessageW`) | **here** |
//! | what each message *means* | **Smalltalk** — from WG2, through the door |
//! | the door itself (trampoline, allowlist, depth guard) | `macvm::runtime::win_wndproc` |
//!
//! ## WG2 — what changed here, and what deliberately did not
//!
//! The door is `macvm`'s (`runtime::win_wndproc`), not this crate's, for the
//! reason its module doc gives: the address channel is a **primitive**, and a
//! downstream crate cannot add a row to the `PRIMITIVES` table. This host's
//! WG2 job is therefore three small things and no Win32 at all beyond what it
//! already had:
//!
//! 1. **Publish the UI VM** (`embed::publish_ui_vm`) so the trampoline can find
//!    it. That is the CG3 mechanism, thread-local and pointer-shaped, and it is
//!    NOT `register_hosted_worker` — WG1's Δ 1 is explicit that the latter
//!    mints an entry in a *primary's* registry and there is still no primary.
//! 2. **Bracket every `eval`/`exec` with a [`macvm::runtime::win_wndproc::BusyGuard`]**,
//!    because `CreateWindowExW` and `SetWindowPos` SEND messages synchronously
//!    and `openMain` runs inside `vm.eval` — without the bracket the door
//!    re-enters the VM on the very first run. See that function's doc.
//! 3. **Stop holding a `&mut VmHandle` across `DispatchMessageW`.** The
//!    trampoline re-borrows the same `VmHandle` from the published raw pointer;
//!    a `&mut` held by the pump across the dispatch would alias it. The pump
//!    now carries the raw pointer and re-borrows only where the door cannot be
//!    running — which is the same discipline `cocoa_gui` follows around
//!    `[NSApp run]`.
//!
//! ## D4 — why Rust holds the pump
//!
//! Three reasons, each worth stating because WG2 will be tempted to move it:
//! it must run before `winui.list` has loaded; a guest fault inside a
//! Smalltalk-owned loop would abandon the pump and freeze the window (P2
//! recovers the VM, not a loop); and `GetMessageW` BLOCKS, which a Smalltalk
//! `[true] whileTrue:` would do with no way for the control channel to
//! interleave.
//!
//! ## D2 — the thread arrangement
//!
//! ```text
//! main thread ── macvm-winui::main
//!                ├─ boot the UI VM            ← in place, not spawned
//!                ├─ load_list("winui.list")
//!                ├─ arm MACVM_WINUI_CTL
//!                ├─ eval "WinShell openMain." ← creates the window HERE
//!                ├─ assert creating tid == pumping tid
//!                └─ loop { GetMessageW; Translate; Dispatch; drain control }
//! ```
//!
//! Windows requires window creation and the pump to share a thread —
//! `CreateWindowExW` binds the HWND to whichever thread made it, and
//! `DispatchMessageW` delivers only on that thread. WG0 got away with a
//! non-main thread only because its window was hidden, loop-less and
//! immediately destroyed; that exemption ends here, so the invariant is
//! ASSERTED at startup (`GetWindowThreadProcessId` against
//! `GetCurrentThreadId`) rather than assumed.
//!
//! > **Δ (WG1, measured).** `sprint_wg1_detail.md` D2 writes
//! > `register_hosted_worker(&mut vm)` into this sequence. That call mints a
//! > worker entry **in a PRIMARY VM's registry** — it is how a primary hands
//! > the UI worker an id and a reply channel (CG1) — and the same document
//! > says, correctly, that WG1 has **no primary VM**. There is nothing for a
//! > hosted worker to be hosted *by*, and calling it on the only VM in the
//! > process would answer `None` (not a primary) or, worse, make this VM its
//! > own primary for no reason. The thing D2 actually wants — "the VM lives
//! > on the thread that called this, not on a spawned one" — is what
//! > `VmHandle::boot` on `main` already is. `register_hosted_worker` comes
//! > back at WG4+, when commitment 2 (a primary messaging the UI VM) has
//! > something worth messaging about.
//!
//! ## §3.1 — the capture channel is INHERITED
//!
//! `control.rs` and `snap.rs` below are `#[path]`-included from `gui/src/`,
//! not copied. They were built and proven against the WebView2 GUI before
//! WG1 needed them, and `snap` is `PrintWindow` + `PW_RENDERFULLCONTENT`
//! *specifically* so it captures any HWND — so the Smalltalk window works
//! through it unchanged, with no WebView2 in this process at all. A second
//! capture implementation would be a regression dressed as a deliverable.
//! The only WG1-specific decision is the env var: `MACVM_WINUI_CTL`, so both
//! apps stay independently drivable in one session.

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "macvm-winui is the WINDOWS-native UI host (docs/win_gui_design.md); \
         on macOS the equivalent is macvm-cocoa."
    );
    std::process::exit(1);
}

// WG4 D1: the two-VM handshake (docs/sprints/sprint_wg4_detail.md). A local
// module, not a `#[path]` include: the Cocoa twin it mirrors is close but not
// identical (the wake is a `PostMessageW`, not a run-loop hop), and a shared
// copy would have to grow a platform switch inside it.
#[cfg(windows)]
mod boot;
mod debugger;
mod game;
mod game_input;
mod text_overlay;

#[cfg(windows)]
#[path = "../../gui/src/control.rs"]
mod control;

#[cfg(windows)]
#[path = "../../gui/src/shell/snap.rs"]
mod snap;

#[cfg(windows)]
mod app {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Receiver;
    use std::sync::Arc;

    use macvm::embed::{publish_ui_vm, set_fatal_mode, VmHandle};
    use macvm::runtime::vm_state::FatalMode;
    use macvm::runtime::win_wndproc;
    use macvm::runtime::VmOptions;

    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetDlgItem, GetMessageW, GetWindowThreadProcessId, IsWindow,
        PeekMessageW, PostMessageW, PostQuitMessage, PostThreadMessageW, SendMessageW,
        SetWindowPos, TranslateMessage, HWND_TOP, MSG, PM_NOREMOVE, PM_REMOVE, SWP_NOMOVE,
        SWP_NOZORDER, WM_APP, WM_QUIT,
    };

    use crate::control::CtlReq;
    use crate::snap;

    /// The wake the control channel's listener thread posts. A THREAD message,
    /// not a window message, and that is the WG1-specific half of inheriting
    /// the channel: `macvm-gui` pokes its HWND (reading it at notify time, the
    /// P4 fix), but this host arms its channel BEFORE Smalltalk has made a
    /// window and must stay wakeable if `openMain` ever fails. A thread
    /// message needs no window, and `DispatchMessageW` ignores it — which is
    /// exactly right, because its only job is to make `GetMessageW` return so
    /// the loop reaches `drain_control_requests`.
    const WM_MACVM_CTL: u32 = WM_APP + 1;

    /// What one `GetMessageW` return means.
    ///
    /// This exists as a named function with a test because of the pitfall
    /// `sprint_wg1_detail.md` opens with: **`GetMessageW` returns −1 on
    /// error**, and the idiomatic `while GetMessageW(..).as_bool()` treats −1
    /// as TRUE and spins forever on a bad `MSG` pointer. `BOOL(-1).as_bool()`
    /// is `-1 != 0`, i.e. true — the bug is invisible at the call site and
    /// only appears when something has already gone wrong.
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub enum Step {
        /// A real message: translate, dispatch, then drain the control queue.
        Dispatch,
        /// `WM_QUIT` — the loop's designed exit, and the only one that is a
        /// success.
        Quit,
        /// −1: an invalid window handle or filter. Leave the loop and say so.
        Failed,
    }

    pub fn classify(rc: i32) -> Step {
        match rc {
            -1 => Step::Failed,
            0 => Step::Quit,
            _ => Step::Dispatch,
        }
    }

    /// Where the `.mst` world lives. `MACVM_WORLD` overrides; the default is
    /// the repo-relative `world`, which is what every other host in this tree
    /// assumes and what `just` runs from.
    fn world_dir() -> PathBuf {
        std::env::var_os("MACVM_WORLD")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("world"))
    }

    fn vm_options() -> VmOptions {
        // `from_env` so MACVM_JIT / MACVM_HEAP behave as they do everywhere
        // else. The JIT stays ON by default: this VM runs real interactive
        // code, and WG0's Δ 10 (GetLastError clobbered by tier-1 compilation)
        // was found precisely because the JIT was live — running the UI VM
        // interpreted would hide that class of bug rather than fix it.
        VmOptions::from_env()
    }

    // ── WG2: every top-level VM entry this host makes goes through these ─────
    //
    // `win_wndproc::BusyGuard` marks the thread as "a host-initiated top-level
    // VM entry is live", and the door declines while one is. It is not
    // optional and it is not belt-and-braces: `openMain` calls
    // `CreateWindowExW`, which SENDS `WM_CREATE`/`WM_SIZE` synchronously to the
    // window proc — which is now the door — from inside this very `eval`. The
    // depth guard is legitimately 0 there, so without the bracket the first
    // message of the first run is a re-entrant VM entry sharing the one
    // per-thread `sigsetjmp` slot.
    //
    // These two functions are the ONLY place this crate calls `eval`/`exec`,
    // and `every_vm_entry_is_bracketed` (below) fails if a bare one appears.

    /// `vm.eval`, with the door held shut for the duration.
    fn guarded_eval(vm: &mut VmHandle, src: &str) -> Result<String, macvm::embed::GuestError> {
        let _busy = win_wndproc::BusyGuard::enter();
        vm.eval(src)
    }

    /// `vm.exec`, with the door held shut for the duration.
    fn guarded_exec(vm: &mut VmHandle, src: &str) -> Result<(), macvm::embed::GuestError> {
        let _busy = win_wndproc::BusyGuard::enter();
        vm.exec(src)
    }

    /// `1536` -> `"1.5K"`, base-1024, one decimal past the first suffix — the
    /// same compact style the Mac's toolbar and the WKWebView GUI both use, so
    /// a screenshot of either reads the same way (§2.1: this is identity, not
    /// decoration).
    fn format_bytes(n: u64) -> String {
        const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
        let mut v = n as f64;
        let mut u = 0;
        while v >= 1024.0 && u < UNITS.len() - 1 {
            v /= 1024.0;
            u += 1;
        }
        if u == 0 {
            format!("{n}{}", UNITS[0])
        } else {
            format!("{v:.1}{}", UNITS[u])
        }
    }

    /// WG4 D2: sample the PRIMARY's live metrics and push one formatted readout
    /// into the cluster.
    ///
    /// The sample is read off the shared snapshot the primary republishes each
    /// beat — not asked for over the request seam, because a metric is a sample
    /// and not a request (see `boot::MetricsSnapshot`). The five values travel
    /// as ONE call, so the cluster can never show a mixture of two moments.
    ///
    /// Throttled to ~4 Hz: this runs from the pump, which turns as fast as
    /// Windows delivers messages, and a resize drag would otherwise spend a VM
    /// entry per frame formatting numbers nobody can read that fast.
    /// WG7-2: the Monitor's roster, pushed into the guest ~1 Hz.
    ///
    /// `macvm::embed::monitor_snapshot()` is fed by each VM from its OWN
    /// thread, so reading it crosses into nobody — which is the property that
    /// makes a roster of live VMs safe to render at all.
    ///
    /// PUSHED, not pulled, and from the EXE rather than a DLL. The roster is a
    /// process-wide `static`, and a cdylib that linked `macvm` separately would
    /// get its OWN copy — a Monitor reading it would show one VM (its own) and
    /// look plausible while being completely wrong. `win_gui` is the exe every
    /// VM in this process was booted by, so it is the only place the roster is
    /// the real one. That is the same "two copies is split state" pitfall
    /// WG6d's design records for `winui_render`.
    ///
    /// ONE STRING for the WHOLE table each refresh — blast, don't patch, the
    /// rule 60's editor note settled and `85_cocoamonitor.mst` states for this
    /// view specifically. Rows are newline-separated, fields US-separated
    /// (0x1F), the same wire format WG6b's Find already uses and the guest
    /// already parses.
    /// Where File In writes its buffer, and where the restart reads it.
    ///
    /// BOTH SIDES DERIVE IT, neither carries it: the guest builds the same
    /// path from `GetTempPathW` and both read TMP/TEMP, so nothing has to
    /// marshal a string through a private message's `wParam`. The coupling is
    /// this constant, named in both files.
    fn filein_scratch_path() -> std::path::PathBuf {
        std::env::temp_dir().join("macvm-editor-filein.mst")
    }

    fn refresh_monitor(vm: &mut VmHandle) {
        use std::time::{Duration, Instant};
        static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
        {
            let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = *last {
                if t.elapsed() < Duration::from_millis(1000) {
                    return;
                }
            }
            *last = Some(Instant::now());
        }
        const US: char = '\u{1f}';
        let rows: Vec<String> = macvm::embed::monitor_snapshot()
            .into_iter()
            .map(|r| {
                let m = &r.metrics;
                // STALENESS is a column, not a footnote: a VM whose owner has
                // stopped publishing shows its age rather than a confident,
                // frozen, wrong number. Same argument the metrics cluster makes.
                let age = match r.age_ms {
                    Some(ms) if ms > 2000 => format!("{}s", ms / 1000),
                    Some(_) => "live".to_string(),
                    None => "—".to_string(),
                };
                let state = if !r.alive {
                    "dead"
                } else if r.busy {
                    "busy"
                } else {
                    "idle"
                };
                format!(
                    // ASCII ONLY in this table. The guest writes cells from a
                    // String that is UTF-8 BYTES, and the renderer takes
                    // CODEPOINTS — so a multi-byte character arrives as two
                    // cells of raw bytes and shows as mojibake. A middot in
                    // the GC column rendered as `0Â·0`, measured on screen.
                    // The general fix is the UTF-8→codepoint conversion that
                    // 106 already names as a known gap; not emitting non-ASCII
                    // from here is the honest fix for THIS table.
                    "{}{US}{}{US}{}{US}{}{US}{}{US}{}/{}{US}{}{US}{}",
                    r.label,
                    r.kind,
                    state,
                    age,
                    format_bytes(m.eden_used + m.old_used),
                    m.scavenges,
                    m.full_gcs,
                    m.bytes_allocated,
                    m.compilations,
                )
            })
            .collect();
        // Escaped for a Smalltalk literal: the labels are ours, but a doit
        // built by string concatenation is a place where "ours" is an
        // assumption rather than a fact.
        let payload = rows.join("\n").replace('\'', "''");
        let _ = guarded_exec(vm, &format!("WinShell monitorRowsArrived: '{payload}'."));
    }

    fn refresh_metrics(
        vm: &mut VmHandle,
        snap: &crate::boot::MetricsSnapshot,
        prev_alloc: &mut Option<u64>,
    ) {
        use std::time::{Duration, Instant};
        static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
        {
            let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = *last {
                if t.elapsed() < Duration::from_millis(250) {
                    return;
                }
            }
            *last = Some(Instant::now());
        }
        let Some((m, taken)) = (match snap.lock() {
            Ok(g) => *g,
            Err(e) => *e.into_inner(),
        }) else {
            return; // the primary has not published its first sample yet
        };
        // `old_reserved`, not `old_committed`: the committed figure starts small
        // and grows on demand, so it reads as "this VM can only ever use 20 MiB"
        // — misleadingly small against a 512 MiB reservation. Same choice the
        // Mac's own cluster makes, and for the same reason.
        let mem = format!(
            "{}/{}",
            format_bytes(m.eden_used + m.old_used),
            format_bytes(m.eden_capacity + m.old_reserved)
        );
        let jit = if m.compilations == 0 {
            "—".to_string()
        } else {
            format!("{}c", m.compilations)
        };
        let code = format!("{} nm", m.nmethods);
        let alloc = match *prev_alloc {
            // One beat is 250ms, so *4 approximates bytes/sec.
            Some(prev) => format!(
                "{}/s",
                format_bytes(m.bytes_allocated.saturating_sub(prev).saturating_mul(4))
            ),
            None => "—".to_string(),
        };
        *prev_alloc = Some(m.bytes_allocated);
        let gc = format!("{}·{}", m.scavenges, m.full_gcs);
        // STALENESS, which is the whole point of reading a live primary rather
        // than being fed literals: if the primary is wedged in a long doit or
        // dead, its samples stop and the cluster says so instead of showing a
        // confident, frozen, wrong number.
        let age = taken.elapsed().as_millis();
        let gc = if age > 2000 {
            format!("{gc} (stale {}s)", age / 1000)
        } else {
            gc
        };
        let doit = format!(
            "WinShell updateMetricsMem: '{mem}' jit: '{jit}' code: '{code}' alloc: '{alloc}' gc: '{gc}'."
        );
        let _ = guarded_exec(vm, &doit);
    }

    /// The UI window's HWND, cached for ONE purpose: the primary's wake.
    ///
    /// `shell_hwnd` deliberately never caches — `WinShell` is the authority and
    /// a Rust copy goes stale the moment a doit closes the window. This is the
    /// one exception, and it is safe for the same reason it is necessary: the
    /// wake fires on the PRIMARY's thread, which may not touch the UI VM at
    /// all, so it cannot ask. A stale handle costs nothing — `PostMessageW`
    /// fails and answers 0, and the heartbeat still runs — whereas asking
    /// across the seam would be exactly the shared-state coupling §2.2 forbids.
    static UI_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

    /// Publish (or clear) the HWND the primary's wake posts to. Called from the
    /// UI thread only.
    fn publish_ui_hwnd(h: isize) {
        UI_HWND.store(h, std::sync::atomic::Ordering::Release);
    }

    /// The primary's `InboxWakeFn`: get the UI thread to LOOK AT ITS INBOX.
    ///
    /// Runs on the primary's thread, so it does exactly one thing and that
    /// thing is thread-safe by design: post `WM_APP_DRAIN`. The reply pump is
    /// then an ordinary drain callee, which is what keeps §2.4a intact with a
    /// second VM in the picture — the wake RECORDS that there is work; the
    /// drain does it.
    ///
    /// It cannot call `win_wndproc::request_drain`: that reads and writes the
    /// UI thread's own thread-locals (the requested flag and the posted latch),
    /// and setting them from here would mark the wrong thread. The cost is that
    /// a chatty primary posts more than the latch would allow; the WORK still
    /// coalesces in the drain, which is the property that matters.
    pub(crate) fn wake_ui_thread() {
        let h = UI_HWND.load(std::sync::atomic::Ordering::Acquire);
        if h == 0 {
            return; // no window yet — the heartbeat will find the work
        }
        unsafe {
            let _ = PostMessageW(
                Some(HWND(h as *mut core::ffi::c_void)),
                macvm::runtime::win_wndproc::WM_APP_DRAIN,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }

    /// The two per-sprint escape hatches, applied to a UI VM that has already
    /// loaded `winui.list`. Shared by both boot paths, because a hatch that
    /// only worked on one of them would silently stop bisecting the moment the
    /// default path changed — which is exactly what the hatches exist to
    /// prevent.
    fn apply_layer_hatches(vm: &mut VmHandle) {
        // WG3: `tests_wg3.md` item 1 wants WG2's whole gate re-run "with the
        // drain installed and NO CONTROL CREATED" — a themed list view
        // repaints, a repaint sends `NM_CUSTOMDRAW`, and WG2's gate counts
        // every message that crossed the door; controls also put white
        // list-view pixels exactly where WG1's and WG2's gates read the
        // window's background fill.
        if matches!(
            std::env::var("MACVM_WINUI_CONTROLS").as_deref(),
            Ok("off") | Ok("0") | Ok("false")
        ) {
            if let Err(e) = guarded_exec(vm, "WinShell controlsEnabled: false.") {
                eprintln!("macvm-winui: MACVM_WINUI_CONTROLS=off: {e}");
            }
        }
        // WG4: the same hatch one sprint on. `gate-wg3` runs with the shell
        // off, because its assertions are about WG3's three controls in WG3's
        // three bands.
        if matches!(
            std::env::var("MACVM_WINUI_WG4").as_deref(),
            Ok("off") | Ok("0") | Ok("false")
        ) {
            if let Err(e) = guarded_exec(vm, "WinShell wg4Enabled: false.") {
                eprintln!("macvm-winui: MACVM_WINUI_WG4=off: {e}");
            }
        }
    }

    /// Boot the UI VM **on the current thread**, then layer `winui.list`.
    fn boot_ui_vm() -> Result<VmHandle, String> {
        let world = world_dir();
        let mut vm = VmHandle::boot(vm_options(), &world).map_err(|e| e.msg)?;
        // `boot` arms `ExitThread` (right for a spawned worker, wrong here):
        // a genuine VM-fatal on MAIN must end the PROCESS, not `pthread_exit`
        // main and leave a headless zombie still holding a visible window.
        // Same ordering the Cocoa host documents — flip it before any work
        // that could fault.
        set_fatal_mode(FatalMode::ExitProcess);
        let list = world.join("winui.list");
        vm.load_list(&list)
            .map_err(|e| format!("loading {}: {}", list.display(), e.msg))?;
        // WG3: `tests_wg3.md` item 1 wants WG2's whole gate re-run "with the
        // drain installed and NO CONTROL CREATED", and that phrase is
        // load-bearing rather than incidental — a themed list view repaints, a
        // repaint sends `NM_CUSTOMDRAW`, and WG2's gate counts every message
        // that crossed the door; controls also put white list-view pixels
        // exactly where WG1's and WG2's gates read the window's background
        // fill. So the older gates set this and go on testing the
        // configuration they were written against. Default is ON: this is the
        // sprint that adds controls.
        apply_layer_hatches(&mut vm);
        Ok(vm)
    }

    /// The HWND Smalltalk currently owns, asked fresh every time.
    ///
    /// Never cached: `WinShell` is the authority on whether there is a window,
    /// and a Rust-side copy would go stale the moment a doit closed it — which
    /// is a thing the control channel can do. `0` is the ordinary "not yet"
    /// answer and `snap_hwnd` already knows what to say about it.
    fn shell_hwnd(vm: &mut VmHandle) -> HWND {
        let raw = match guarded_eval(vm, "WinShell hwndValue.") {
            Ok(s) => s.trim().parse::<u64>().unwrap_or(0),
            Err(_) => 0,
        };
        HWND(raw as *mut core::ffi::c_void)
    }

    /// Serve every queued control request. Runs on the UI thread, which here
    /// is also the VM's thread — so unlike `macvm-gui` (whose VM is on a
    /// worker and can only be *submitted* to), `eval` answers INLINE with the
    /// real `printString`. That is what makes the gate a script: `gui eval
    /// "WinShell clientWidth"` returns the number Win32 just gave Smalltalk.
    pub fn drain_control_requests(
        vm: &mut VmHandle,
        rx: &Receiver<CtlReq>,
        link: &mut Option<crate::boot::PrimaryLink>,
    ) {
        while let Ok(req) = rx.try_recv() {
            let cmd = req.cmd.trim().to_string();
            let (verb, arg) = match cmd.split_once(' ') {
                Some((v, a)) => (v, a.trim()),
                None => (cmd.as_str(), ""),
            };
            match verb {
                "ping" => {
                    let _ = req.reply.send("OK pong".into());
                }
                "eval" => {
                    let reply = match guarded_eval(vm, arg) {
                        Ok(s) => format!("OK {s}"),
                        Err(e) => format!("ERR {e}"),
                    };
                    let _ = req.reply.send(reply);
                }
                "doit" => {
                    let reply = match guarded_exec(vm, arg) {
                        Ok(()) => "OK".to_string(),
                        Err(e) => format!("ERR {e}"),
                    };
                    let _ = req.reply.send(reply);
                }
                "snap" => {
                    let h = shell_hwnd(vm);
                    snap::snap_hwnd(h, arg, req.reply);
                }
                // ── WG2's three door verbs ───────────────────────────────
                //
                // `door` reports what the door has done (counters + D5's two
                // latency numbers), `doorreset` zeroes them so a measurement
                // describes steady state rather than the first-ever entry's
                // tier-1 compilation, and `door on|off` is the allowlist
                // master switch — implementation-order step 1's transparency
                // proof and D5's baseline in one lever.
                //
                // Read here rather than through a primitive because these are
                // facts about the HOST's door, not about the guest's world:
                // WG1's Δ 3 rule pointed the other way for the same reason.
                "door" => {
                    let reply = match arg {
                        "on" => {
                            win_wndproc::set_door_enabled(true);
                            format!("OK {}", win_wndproc::stats_line())
                        }
                        "off" => {
                            win_wndproc::set_door_enabled(false);
                            format!("OK {}", win_wndproc::stats_line())
                        }
                        "reset" => {
                            win_wndproc::reset_stats();
                            format!("OK {}", win_wndproc::stats_line())
                        }
                        "" => format!("OK {}", win_wndproc::stats_line()),
                        other => format!("ERR unknown door subcommand '{other}'"),
                    };
                    let _ = req.reply.send(reply);
                }
                // The scripted resize, and the ONE place this host calls a
                // Win32 geometry function.
                //
                // `tests_wg2.md` item 2 says to drive it with "SetWindowPos via
                // FFI from a doit", and that cannot work: SetWindowPos SENDS
                // `WM_SIZE` synchronously, so from a doit the wndproc runs
                // inside `vm.exec` and the door correctly refuses it (see
                // `win_wndproc::vm_busy`). A `WM_SIZE` that reaches Smalltalk
                // must originate OUTSIDE every VM entry — which is where every
                // real one does: the pump, or the modal move/size loop Windows
                // runs while a user drags the frame, both of which call
                // SetWindowPos with the VM at rest. This drain is that same
                // place, so this verb is the user's mouse, in a script.
                //
                // D1 is not bent by it: the host still creates, styles and
                // measures nothing. `WinShell resizeWindowTo:by:` is the
                // guest's own version of the same call, kept for WG3's layout
                // code, and it is the WINDOW rect that moves here — the client
                // size the message carries is Win32's arithmetic, not ours.
                "resize" => {
                    let reply = resize_window(vm, arg);
                    let _ = req.reply.send(reply);
                }
                // ── WG3's four drain/control verbs ───────────────────────
                //
                // Every one of them exists for the same reason `resize` does,
                // and it is now a permanent constraint on every WG gate (WG2
                // Δ 3): **a message that reaches Smalltalk must originate
                // OUTSIDE every VM entry.** A doit cannot drive a control,
                // because `SendMessageW` from inside `exec` is correctly
                // refused by the busy guard. This drain is the one place in
                // the process that is neither a VM entry nor a wndproc — the
                // same place the pump and Windows' own modal loop call from.
                //
                // `drain`  — report the drain's tallies (the coalescing ratio
                //            lives here), or force one pass synchronously.
                // `track`  — send the real WM_ENTERSIZEMOVE/WM_EXITSIZEMOVE, so
                //            D2 is proven through the message path a drag uses
                //            and not by poking a Rust flag.
                // `burst`  — N resizes with NO pump turn between them, which is
                //            the only shape in which coalescing is observable:
                //            one `gui resize` per round trip lets the pump
                //            drain each one, and 1:1 is the CORRECT answer at
                //            that rate.
                // `send`   — one synthesised message to the shell or to one of
                //            its controls by id. The user's mouse, in a script.
                "drain" => {
                    let reply = drain_verb(vm, arg);
                    let _ = req.reply.send(reply);
                }
                "track" => {
                    let msg = match arg {
                        "on" => win_wndproc::WM_ENTERSIZEMOVE,
                        "off" => win_wndproc::WM_EXITSIZEMOVE,
                        "menuon" => win_wndproc::WM_ENTERMENULOOP,
                        "menuoff" => win_wndproc::WM_EXITMENULOOP,
                        other => {
                            let _ = req.reply.send(format!(
                                "ERR track wants on|off|menuon|menuoff, got '{other}'"
                            ));
                            continue;
                        }
                    };
                    let reply = send_to(vm, 0, msg, 0, 0)
                        .map(|r| {
                            format!("OK track {arg} lresult={r} {}", win_wndproc::drain_line())
                        })
                        .unwrap_or_else(|e| e);
                    let _ = req.reply.send(reply);
                }
                "burst" => {
                    let reply = resize_burst(vm, arg);
                    let _ = req.reply.send(reply);
                }
                "send" => {
                    let reply = send_verb(vm, arg);
                    let _ = req.reply.send(reply);
                }
                // WG7-3. `restart` replaces the primary WITHOUT touching the
                // window: same HWND, same UI VM, same views, a brand new world
                // behind them. It is the machinery File In and Add to World
                // need — WG6c-3 left both unbuilt for want of it — and it runs
                // HERE, on the control drain between dispatches, because
                // joining a thread from inside the door would be a VM entry
                // waiting on another VM.
                // WG7-1: one command line into the parked halt loop —
                // `step`, `over`, `finish`, `continue`, `abort`. Answers
                // whether it was ACCEPTED, because a command sent while
                // nothing is halted is dropped on purpose (see `send_command`).
                "dbg" => {
                    let ok = crate::debugger::send_command(arg.to_string());
                    let _ = req.reply.send(if ok {
                        format!("OK dbg {arg}")
                    } else {
                        "ERR dbg: nothing is halted".to_string()
                    });
                }
                "dbgreport" => {
                    let _ = req.reply.send(format!("OK {}", crate::debugger::report()));
                }
                "restart" => {
                    let reply = match link.as_mut() {
                        None => "ERR no primary to restart (single-VM path)".to_string(),
                        Some(l) => {
                            let before = l.hosted_id;
                            match crate::boot::restart_primary(
                                vm,
                                l,
                                world_dir(),
                                std::sync::Arc::new(wake_ui_thread),
                                crate::boot::PrimarySeed::none(),
                            ) {
                                Ok(()) => {
                                    format!("OK restart hosted_id {} -> {}", before, l.hosted_id)
                                }
                                Err(e) => format!("ERR restart: {}", e.msg),
                            }
                        }
                    };
                    let _ = req.reply.send(reply);
                }
                // The control channel's own exit path. `sprint_wg1_detail.md`
                // records why it exists: closing is TWO events — `WM_CLOSE`
                // destroys the window and `WM_DESTROY` should `PostQuitMessage`
                // — and the second half needs the door WG2 builds. Until then
                // the loop ends either because a script said so here, or
                // because it noticed its window had gone (see `pump`).
                "quit" => {
                    unsafe { PostQuitMessage(0) };
                    let _ = req.reply.send("OK".into());
                }
                other => {
                    let _ = req
                        .reply
                        .send(format!("ERR unknown control verb '{other}'"));
                }
            }
        }
    }

    /// `resize <w> <h>`: move the window's frame and let Win32 send the
    /// `WM_SIZE` that follows — from here, on the pump's thread, with no VM
    /// entry live.
    ///
    /// The HWND is asked of Smalltalk (which gates it with `IsWindow`, WG1's
    /// Δ 2) and then re-checked here, because between the `eval` returning and
    /// the `SetWindowPos` there is a doit-shaped hole a script could close the
    /// window through. A remembered HWND is not a window.
    fn resize_window(vm: &mut VmHandle, arg: &str) -> String {
        let mut it = arg.split_whitespace();
        let (Some(w), Some(h)) = (it.next(), it.next()) else {
            return "ERR resize wants <width> <height>".into();
        };
        let (Ok(w), Ok(h)) = (w.parse::<i32>(), h.parse::<i32>()) else {
            return "ERR resize wants two integers".into();
        };
        let hwnd = shell_hwnd(vm);
        if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return "ERR no window yet".into();
        }
        // SetWindowPos dispatches WM_SIZE into the door SYNCHRONOUSLY from
        // here — which is exactly the point: no `eval` is live, no door entry
        // is live, so the depth guard is 0 and the message crosses.
        let ok =
            unsafe { SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER) };
        match ok {
            Ok(()) => format!("OK resized to {w}x{h}"),
            Err(e) => format!("ERR SetWindowPos: {e}"),
        }
    }

    /// `drain` / `drain reset` / `drain now`: the flag-and-drain pass's own
    /// instrument panel.
    ///
    /// `drain` reports the tallies — `requests` against `passes` IS the
    /// coalescing ratio `tests_wg3.md` item 3 asks for, and `suppressed`
    /// against `passes` is D2's. `drain now` forces one pass synchronously from
    /// here (still outside every VM entry), which is what makes a gate line
    /// read a number instead of sleeping and hoping the heartbeat has fired.
    fn drain_verb(vm: &mut VmHandle, arg: &str) -> String {
        match arg {
            "" => format!("OK {}", win_wndproc::drain_line()),
            "reset" => {
                win_wndproc::reset_stats();
                format!("OK {}", win_wndproc::drain_line())
            }
            "now" => {
                // Requested, then pumped: exactly the path a real wake takes,
                // so this verb tests the mechanism rather than bypassing it. A
                // direct call to the pass would prove the pass runs, not that
                // the wake reaches it — and, crucially, it would also run while
                // TRACKING, which is the one state the gate most needs to see
                // suppressed.
                //
                // `request_drain` rather than a bare post, because "service the
                // drain" has to mean "there is work" — a heartbeat with no
                // request costs no VM entry by design (that is what keeps
                // `drainPasses` a measurement of the drain rather than of the
                // clock), so a post on its own would be a no-op.
                let hwnd = shell_hwnd(vm);
                if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                    return "ERR no window yet".into();
                }
                win_wndproc::request_drain(hwnd.0 as isize);
                pump_pending(hwnd);
                format!("OK {}", win_wndproc::drain_line())
            }
            other => format!("ERR unknown drain subcommand '{other}'"),
        }
    }

    /// Dispatch every message already in this thread's queue, then return.
    ///
    /// The control drain runs *between* `GetMessageW` calls, so a message this
    /// verb posts would not be delivered until the script's NEXT round trip —
    /// which would make every gate line read the state before its own action.
    /// `PeekMessageW(PM_REMOVE)` in a loop is the same dispatch the pump does,
    /// on the same thread, and it stops when the queue empties rather than
    /// blocking.
    fn pump_pending(_hwnd: HWND) {
        let mut msg = MSG::default();
        // Bounded: a handler that posted a message per message would otherwise
        // spin here forever, and a control verb that never answers is
        // indistinguishable from a hang.
        for _ in 0..4096 {
            let got = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool();
            if !got {
                return;
            }
            if msg.message == WM_QUIT {
                unsafe { PostQuitMessage(msg.wParam.0 as i32) };
                return;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// `send [<controlId>] <msg> <wParam> <lParam>` — one synthesised message,
    /// from the one place in this process that is neither a VM entry nor a
    /// wndproc.
    ///
    /// A control id of 0 (or omitted) targets the shell window itself; anything
    /// else is resolved with `GetDlgItem`, which is a lookup and not a policy —
    /// the host still creates nothing, styles nothing and decides nothing. This
    /// is how `tests_wg3.md` item 6 clicks a button: `WM_COMMAND` with the
    /// button's id and `BN_CLICKED` in the high word, arriving exactly as
    /// Windows would deliver it.
    fn send_verb(vm: &mut VmHandle, arg: &str) -> String {
        let f: Vec<&str> = arg.split_whitespace().collect();
        let (id, rest) = match f.len() {
            3 => (0i32, &f[0..3]),
            4 => match f[0].parse::<i32>() {
                Ok(n) => (n, &f[1..4]),
                Err(_) => return "ERR send: the first of four fields is a control id".into(),
            },
            _ => return "ERR send wants [<controlId>] <msg> <wParam> <lParam>".into(),
        };
        let (Ok(msg), Ok(wp), Ok(lp)) = (
            parse_word(rest[0]),
            parse_word(rest[1]),
            rest[2].parse::<i64>(),
        ) else {
            return "ERR send: msg/wParam/lParam must be integers".into();
        };
        match send_to(vm, id, msg as u32, wp as usize, lp as isize) {
            Ok(r) => format!("OK send lresult={r}"),
            Err(e) => e,
        }
    }

    /// Accepts decimal or `0x`/`16r` hex — message numbers read better in hex
    /// and `WM_COMMAND`'s wParam is a packed pair.
    fn parse_word(s: &str) -> Result<i64, std::num::ParseIntError> {
        if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("16r")) {
            i64::from_str_radix(h, 16)
        } else {
            s.parse::<i64>()
        }
    }

    /// `SendMessageW` to the shell window or one of its controls, with the WG1
    /// Δ 2 rule applied at both ends: a remembered HWND is not a window.
    fn send_to(
        vm: &mut VmHandle,
        control_id: i32,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> Result<isize, String> {
        let shell = shell_hwnd(vm);
        if shell.0.is_null() || !unsafe { IsWindow(Some(shell)) }.as_bool() {
            return Err("ERR no window yet".into());
        }
        let target = if control_id == 0 {
            shell
        } else {
            let h = unsafe { GetDlgItem(Some(shell), control_id) }
                .map_err(|e| format!("ERR GetDlgItem({control_id}): {e}"))?;
            if h.0.is_null() {
                return Err(format!("ERR no control with id {control_id}"));
            }
            h
        };
        let r = unsafe { SendMessageW(target, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) };
        Ok(r.0)
    }

    /// `burst <n> [<w0> <h0>]`: N `SetWindowPos` calls with **no pump turn
    /// between them** — the only shape in which the drain's coalescing is
    /// observable, and the shape a real storm has.
    ///
    /// One `gui resize` per round trip lets the pump service each wake, and a
    /// 1:1 pass-to-message ratio is the CORRECT answer at that rate — the drain
    /// is not meant to skip work that nothing is racing. What it is meant to do
    /// is absorb a burst, so the burst has to be a burst.
    fn resize_burst(vm: &mut VmHandle, arg: &str) -> String {
        let mut it = arg.split_whitespace();
        let Some(Ok(n)) = it.next().map(|s| s.parse::<i32>()) else {
            return "ERR burst wants <count> [<w0> <h0>]".into();
        };
        let w0 = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(600);
        let h0 = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(440);
        let hwnd = shell_hwnd(vm);
        if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return "ERR no window yet".into();
        }
        for i in 0..n {
            let ok = unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    w0 + i,
                    h0 + i,
                    SWP_NOMOVE | SWP_NOZORDER,
                )
            };
            if let Err(e) = ok {
                return format!("ERR SetWindowPos #{i}: {e}");
            }
        }
        format!(
            "OK burst {n} from {w0}x{h0} last={}x{} {}",
            w0 + n - 1,
            h0 + n - 1,
            win_wndproc::drain_line()
        )
    }

    /// The pump. D4's loop, with the −1 arm the naive form drops.
    ///
    /// `window` is read from Smalltalk ONCE, before the loop, and its liveness
    /// is then checked with Rust's own `IsWindow` — never by asking the VM
    /// again. That is not a micro-optimisation: `vm.eval("WinShell hwndValue")`
    /// is a parse, a compile and a send, and doing it per message would put a
    /// full Smalltalk round-trip inside a `WM_MOUSEMOVE` storm. The pump's
    /// question is "does my window still exist", which is Win32's to answer
    /// cheaply; the VM's answer is needed only when the window CHANGES, which
    /// in WG1 it cannot.
    ///
    /// ## WG2: the VM crosses this function as a RAW POINTER, deliberately
    ///
    /// `DispatchMessageW` calls the door, which re-borrows the same `VmHandle`
    /// from the pointer `publish_ui_vm` published. A `&mut VmHandle` held by
    /// this loop across the dispatch would alias that re-borrow. So the pump
    /// carries `*mut VmHandle` and materialises a `&mut` only where the door
    /// provably is not running — inside `drain_control_requests`, which is
    /// itself bracketed by a `BusyGuard`.
    ///
    /// ## The liveness check is now a BACKSTOP, and says so
    ///
    /// WG1 used it as its stand-in for `WM_DESTROY` → `PostQuitMessage`, and
    /// `sprint_wg2_detail.md` says to delete it once the door lands. It is kept
    /// — with a DIFFERENT message — for one reason: if `WinShell>>onDestroy`
    /// ever fails to post the quit (a raise inside it, a `winui.list` that
    /// loaded a broken version), the alternative to this branch is a process
    /// pumping an empty queue forever with a window that no longer exists,
    /// which is precisely the WG1 Δ 2 hang this port has already paid for once.
    ///
    /// Keeping it costs nothing and would make gate item 3 ("`WM_DESTROY` posts
    /// the quit from **Smalltalk**, not from Rust") unprovable — so the two
    /// paths print different lines and `just gate-wg2` asserts it saw the
    /// Smalltalk one and did NOT see this one. Safety and provability, rather
    /// than a choice between them.
    /// `inbox`: the primary -> UI channel, when the two-VM split is on (WG4
    /// D1). `None` on the single-VM path, where there is no primary to hear
    /// from — the older gates' configuration.
    pub fn pump(
        vmp: *mut VmHandle,
        rx: Option<&Receiver<CtlReq>>,
        window: HWND,
        link: &mut Option<crate::boot::PrimaryLink>,
    ) -> i32 {
        let mut msg = MSG::default();
        // WG7-2: this VM's own row in the Monitor's roster. Registered here
        // rather than at boot because this is the thread that will publish it,
        // and a slot published from anywhere but its owner's thread is a heap
        // read racing the mutator.
        let ui_mon = macvm::embed::monitor_register("ui".into(), "ui");
        // The previous allocation total, so ALLOC can report a RATE rather than
        // a running sum — the Mac's own cluster does the same.
        let mut prev_alloc: Option<u64> = None;
        // WG11-W1: the Canvas pane's hwnd, learned once. Zero until the view is
        // built; re-asked only while it is zero, so a running game costs no VM
        // entry to find out where its pixels go.
        let mut game_pane_hwnd: i64 = 0;
        let mut game_mode_set = false;
        let mut had_window = !window.0.is_null();
        loop {
            let rc = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
            match classify(rc) {
                Step::Failed => {
                    eprintln!(
                        "macvm-winui: GetMessageW answered -1 (invalid window handle or filter); \
                         leaving the loop rather than spinning on a bad MSG"
                    );
                    return 1;
                }
                Step::Quit => return 0,
                Step::Dispatch => {
                    // The door may run inside here. No `&mut VmHandle` is live
                    // across it.
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if let Some(rx) = rx {
                        // SAFETY: the handle is boxed, published on this
                        // thread, and outlives the loop; the door is not
                        // running here (we are between dispatches on the one
                        // thread that dispatches), so this `&mut` is unique.
                        let vm: &mut VmHandle = unsafe { &mut *vmp };
                        drain_control_requests(vm, rx, link);
                    }
                    // WG4 D1: the primary -> UI inbox, pumped HERE and nowhere
                    // else. This is the Windows twin of the Mac's
                    // `drain_perform` envelope loop, and the placement is the
                    // whole of §2.4a's rule with a second VM in the picture:
                    // `DispatchMessageW` has RETURNED, so the VM is quiescent
                    // and provably not inside a callback. An envelope
                    // dispatched from the door would be a nested VM entry —
                    // the exact failure the drain exists to prevent.
                    // WG4 D2: the metrics cluster, sampled off the primary.
                    // Runs on the same "between dispatches" beat as the inbox
                    // pump and throttles itself to ~4 Hz.
                    // WG7-3: read through the LINK every pass rather than
                    // holding a borrow across the loop. A restart replaces the
                    // inbox and the metrics slot, and a pump still draining the
                    // old ones would be draining a channel nobody sends on —
                    // silent, and indistinguishable from a primary that has
                    // stopped answering.
                    if let Some(snap) = link.as_ref().map(|l| l.metrics.clone()) {
                        let vm: &mut VmHandle = unsafe { &mut *vmp };
                        refresh_metrics(vm, &snap, &mut prev_alloc);
                    }
                    // WG7-2: the Monitor's roster, on the same between-
                    // dispatches beat and throttled to its own 1 Hz. This VM
                    // publishes its OWN row first, from its own thread, which
                    // is the same rule the primary follows on its beat.
                    {
                        let vm: &mut VmHandle = unsafe { &mut *vmp };
                        ui_mon.publish(vm.metrics());
                        refresh_monitor(vm);
                    }
                    // WG7-1: a fresh halt report. Pushed on the SAME beat and
                    // for the same reason everything else is — the primary is
                    // parked inside the halt, so it cannot be asked; it can
                    // only have told us. That the pump is still running at all
                    // while the VM it debugs is frozen is WG4 D1's two-VM
                    // split doing its job.
                    // WG6c-3/WG7: FILE IN — a fresh world, then the file the
                    // guest just wrote. Acted on HERE because it joins the
                    // primary's thread, which a wndproc must never do.
                    if macvm::runtime::win_wndproc::take_filein_requested() {
                        let vm: &mut VmHandle = unsafe { &mut *vmp };
                        let path = filein_scratch_path();
                        let msg = match link.as_mut() {
                            None => "file-in needs the two-VM path".to_string(),
                            Some(l) => match crate::boot::restart_primary(
                                vm,
                                l,
                                world_dir(),
                                std::sync::Arc::new(wake_ui_thread),
                                crate::boot::PrimarySeed::file(path.clone()),
                            ) {
                                Ok(()) => format!("filed in {}", path.display()),
                                Err(e) => format!("file-in FAILED: {}", e.msg),
                            },
                        };
                        let _ = guarded_exec(
                            vm,
                            &format!("WinShell appendTranscript: 'editor: {msg}'."),
                        );
                    }
                    // WG9-2: LOAD THE TEST CORPUS — a fresh world, then the
                    // world's own SUnit classes, so the Tests view has the real
                    // 8000-assertion corpus to run rather than an empty image.
                    // Same door and the same reason as file-in: it joins the
                    // primary's thread.
                    // WG11-W1: the GAME FRAME. A presented frame is uploaded to
                    // the Canvas pane and shown. Done HERE, between dispatches,
                    // because the renderer's per-hwnd state is thread_local to
                    // this thread and the frame was drawn on the primary's.
                    //
                    // The pane's hwnd is CACHED and re-asked only while it is
                    // zero: a `guarded_eval` per frame would be sixty VM entries
                    // a second to learn a number that changes once.
                    // Learn the pane as soon as the game is RUNNING, not only
                    // once a frame is pending: the input driver needs the hwnd
                    // to scale the pointer, and waiting for the first Present
                    // would make the first steps report (0,-1,-1,0).
                    if crate::game::is_running() || crate::game::frame_pending() {
                        let vm: &mut VmHandle = unsafe { &mut *vmp };
                        if game_pane_hwnd == 0 {
                            game_pane_hwnd = guarded_eval(vm, "WinShell canvasPaneHwnd")
                                .ok()
                                .and_then(|s| s.trim().parse::<i64>().ok())
                                .unwrap_or(0);
                        }
                        if game_pane_hwnd != 0 {
                            // ONCE: tell the Canvas a game owns it, so its own
                            // WM_PAINT stops redrawing the shell's demo over
                            // the game's frame. One VM entry per game, not
                            // per frame.
                            if !game_mode_set {
                                let _ = guarded_exec(vm, "WinShell canvasMode: #game.");
                                // The input driver cannot see this local, and
                                // needs the pane to scale the pointer into
                                // game pixels.
                                crate::game::set_pane_hwnd(game_pane_hwnd);
                                game_mode_set = true;
                            }
                            // HAND THE CANVAS BACK when the game ends, and clear
                            // the latch — otherwise a second launch never
                            // re-issues #game and the shell keeps the pane.
                            if game_mode_set && !crate::game::is_running()
                                && !crate::game::frame_pending()
                            {
                                let _ = guarded_exec(vm, "WinShell canvasMode: #plasma.");
                                game_mode_set = false;
                            }
                            crate::game::upload_and_present(game_pane_hwnd);
                        }
                    }
                    if macvm::runtime::win_wndproc::take_loadtests_requested() {
                        let vm: &mut VmHandle = unsafe { &mut *vmp };
                        let msg = match link.as_mut() {
                            None => "loading tests needs the two-VM path".to_string(),
                            Some(l) => match crate::boot::restart_primary(
                                vm,
                                l,
                                world_dir(),
                                std::sync::Arc::new(wake_ui_thread),
                                crate::boot::PrimarySeed::tests(),
                            ) {
                                Ok(()) => "test corpus loaded into a fresh world".to_string(),
                                Err(e) => format!("loading the test corpus FAILED: {}", e.msg),
                            },
                        };
                        let _ = guarded_exec(
                            vm,
                            &format!("WinShell testCorpusArrived: 'tests: {msg}'."),
                        );
                    }
                    if crate::debugger::take_halt_arrived() {
                        let vm: &mut VmHandle = unsafe { &mut *vmp };
                        let payload = crate::debugger::report().replace('\u{27}', "''");
                        let _ = guarded_exec(
                            vm,
                            &format!("WinShell haltArrived: '{payload}'."),
                        );
                    }
                    if let Some(inbox) = link.as_ref().map(|l| &l.inbox) {
                        // SAFETY: as above — between dispatches, on the one
                        // thread that dispatches, no door running.
                        let vm: &mut VmHandle = unsafe { &mut *vmp };
                        while let Some(env) = inbox.poll() {
                            // A payload-less envelope is a BARE NUDGE — the
                            // primary's boot poke is exactly one, and its only
                            // job is to make the wake fire so this loop runs.
                            // Handing empty bytes to the unpickler raises `bad
                            // pickle bytes` in the guest, and that recovered
                            // error unwinds the dispatch and leaves the reply
                            // routing broken for every LATER envelope — which
                            // is how a working round trip silently stopped
                            // working after the first one. Measured, then
                            // fixed here.
                            if env.bytes.is_empty() {
                                continue;
                            }
                            let _ = vm.dispatch_hosted_envelope(env);
                        }
                    }
                    if had_window && !unsafe { IsWindow(Some(window)) }.as_bool() {
                        // WG1's Δ 2 rule, still: a remembered HWND is not a
                        // window, so this asks Win32 rather than the VM. What
                        // is new is the second clause — if the door handled
                        // `WM_DESTROY`, Smalltalk has already posted the quit
                        // and this branch would be a second one, plus a log
                        // line saying the opposite of what happened. The
                        // backstop is for the case where `onDestroy` RAISED.
                        had_window = false;
                        if win_wndproc::guest_handled_destroy() {
                            println!(
                                "macvm-winui: the window is gone and Smalltalk's WM_DESTROY \
                                 handler posted the quit — nothing for the backstop to do"
                            );
                        } else {
                            println!(
                                "macvm-winui: BACKSTOP — the window is gone and Smalltalk's \
                                 WM_DESTROY handler did not post the quit; posting WM_QUIT here"
                            );
                            unsafe { PostQuitMessage(0) };
                        }
                    }
                }
            }
        }
    }

    /// Force this thread's message queue to exist before anything posts to it.
    ///
    /// `PostThreadMessageW` FAILS against a thread that has never called a
    /// message function — and the control channel's listener starts posting
    /// the moment a script connects, which can be before Smalltalk has made a
    /// window. One `PeekMessageW` creates the queue; this is the standard
    /// answer and skipping it is a race that only shows up under a fast
    /// script.
    fn ensure_message_queue() {
        let mut msg = MSG::default();
        unsafe {
            let _ = PeekMessageW(&mut msg, None, WM_APP, WM_APP, PM_NOREMOVE);
        }
    }

    fn arm_control_channel() -> Option<Receiver<CtlReq>> {
        let tid = unsafe { GetCurrentThreadId() };
        crate::control::start(
            "MACVM_WINUI_CTL",
            "macvm-winui",
            Arc::new(move || unsafe {
                let _ = PostThreadMessageW(tid, WM_MACVM_CTL, WPARAM(0), LPARAM(0));
            }),
        )
    }

    /// Report the same-thread invariant, measured from BOTH ends.
    ///
    /// Win32 is the authority on which thread owns a window, so the check is
    /// `GetWindowThreadProcessId(hwnd)` against this thread's own id — not
    /// Smalltalk's record of what it did, which would only prove Smalltalk is
    /// self-consistent. `false` here means `DispatchMessageW` would silently
    /// stop delivering, which is the failure mode with no other symptom.
    fn assert_thread_invariant(vm: &mut VmHandle) -> bool {
        let hwnd = shell_hwnd(vm);
        let here = unsafe { GetCurrentThreadId() };
        if hwnd.0.is_null() {
            eprintln!("macvm-winui: no window, so no thread invariant to assert (pump tid {here})");
            return false;
        }
        let owner = unsafe { GetWindowThreadProcessId(hwnd, None) };
        let ok = owner == here;
        println!(
            "macvm-winui: thread invariant {} — window owned by tid {owner}, pumping on tid {here}",
            if ok { "HOLDS" } else { "BROKEN" }
        );
        if !ok {
            eprintln!(
                "macvm-winui: the window was created off the pump's thread; DispatchMessageW \
                 will never deliver to it (sprint_wg1_detail.md D2)"
            );
        }
        ok
    }

    /// The default mode: open the window and pump until it goes away.
    /// Is the two-VM split on? Default YES — it is §2.2's commitment 2 and the
    /// reason the UI does not block. `MACVM_WINUI_PRIMARY=off` boots the single
    /// VM WG1..WG3 were written against: those gates count MESSAGES, and the
    /// primary's boot poke is one more `WM_APP_DRAIN` than their arithmetic
    /// expects. Same shape of hatch, same reason, as `MACVM_WINUI_WG4`.
    fn primary_enabled() -> bool {
        !matches!(
            std::env::var("MACVM_WINUI_PRIMARY").as_deref(),
            Ok("off") | Ok("0") | Ok("false")
        )
    }

    pub fn run() -> i32 {
        // WINARM (WG6d): THE JIT IS ON HERE UNLESS TOLD OTHERWISE.
        //
        // `VmOptions::from_env` defaults `MACVM_JIT` to `Off`, and that default
        // is right for the reason its own comment gives: hundreds of pre-S10
        // tests were verified against pure-interpreter behaviour and
        // `test_vm()` reads the same env, so defaulting it on would let
        // ambient shell state change unrelated test results.
        //
        // NONE OF THAT APPLIES TO A WINDOW. This process is not a test
        // harness; it is a GUI whose every keystroke runs guest Smalltalk, and
        // inheriting a test-suite default meant the whole world interpreted.
        // Reported as "the UI is chronically slow", correctly, and the other
        // half of that was a debug build.
        //
        // Set rather than forced: an explicit `MACVM_JIT` still wins, so
        // `MACVM_JIT=off` remains the way to measure the interpreter.
        if std::env::var_os("MACVM_JIT").is_none() {
            std::env::set_var("MACVM_JIT", "threshold=20");
        }
        ensure_message_queue();
        // WG4 D1: the primary VM on a background thread, the UI VM in place on
        // THIS one. The handshake parks until the primary is up and has
        // registered us as its hosted peer, so by the time this returns the
        // seam is live in both directions.
        let (mut vm, wired) = if primary_enabled() {
            match crate::boot::handshake_wire_vms(
                world_dir(),
                world_dir().join("winui.list"),
                FatalMode::ExitProcess,
                std::sync::Arc::new(wake_ui_thread),
            ) {
                Ok(w) => {
                    let boxed = Box::new(w.ui_worker);
                    println!(
                        "macvm-winui: two VMs wired — primary on {:?}, UI worker id {} on main",
                        w.link
                            .thread
                            .as_ref()
                            .and_then(|t| t.thread().name().map(|n| n.to_string()))
                            .unwrap_or_else(|| "?".into()),
                        w.link.hosted_id,
                    );
                    (boxed, Some(w.link))
                }
                Err(e) => {
                    eprintln!("macvm-winui: {}", e.msg);
                    return 2;
                }
            }
        } else {
            match boot_ui_vm() {
                Ok(vm) => (Box::new(vm), None),
                Err(e) => {
                    eprintln!("macvm-winui: {e}");
                    return 2;
                }
            }
        };
        // The handshake loads `winui.list` itself, but the two env hatches
        // WG3 and WG4 own are applied by `boot_ui_vm` — so apply them here too
        // on the two-VM path, before anything opens a window.
        if wired.is_some() {
            apply_layer_hatches(&mut vm);
        }
        let _wired = wired;
        // WG2: the door's half of the arrangement. `publish_ui_vm` is the CG3
        // mechanism — a thread-local `*mut VmHandle` a trampoline can read with
        // no Rust lifetime to borrow against — and it is NOT
        // `register_hosted_worker`, which WG1's Δ 1 measured to be incoherent
        // without a primary and which WG2 was told not to reach for either.
        //
        // `Box` because the pointer must stay valid for the life of the process
        // while the pump also borrows the same handle: a stack local's address
        // is stable in practice, but a boxed one is stable by contract, and
        // "in practice" is not a thing to write under a raw pointer that Win32
        // dereferences.
        //
        // Published BEFORE `openMain`, because `CreateWindowExW` calls the door
        // before it returns the HWND.
        let vmp: *mut VmHandle = &mut **&mut vm;
        publish_ui_vm(vmp);
        println!(
            "macvm-winui: WndProc door published at 16r{:X} (allowlist {} messages, enabled={})",
            macvm::runtime::win_wndproc::door_address(),
            macvm::runtime::win_wndproc::ALLOWLIST.len(),
            macvm::runtime::win_wndproc::door_enabled(),
        );

        // Armed BEFORE the window exists, deliberately: requests that land
        // early just queue, and the thread-message wake needs no window. That
        // also makes the "a guest fault inside openMain" case survivable — the
        // script can still reach a VM whose `openMain` never returned.
        let rx = arm_control_channel();

        // The gate's stress row: a fault injected at the END of `openMain`,
        // so the window is fully real and fully shown when the VM falls over.
        // P2's recovery turns it into an `Err` here and the pump below never
        // learns it happened, which is the whole point of D4's second reason.
        match std::env::var("MACVM_WINUI_FAULT").as_deref() {
            Ok("guest") => {
                let _ = guarded_exec(&mut vm, "WinShell faultMode: #guest.");
            }
            Ok("native") => {
                let _ = guarded_exec(&mut vm, "WinShell faultMode: #native.");
            }
            _ => {}
        }

        match guarded_eval(&mut vm, "WinShell openMain.") {
            Ok(s) => println!("macvm-winui: WinShell openMain -> {s}"),
            Err(e) => eprintln!("macvm-winui: WinShell openMain raised: {e} (the pump continues)"),
        }
        let _ = guarded_exec(&mut vm, "WinShell report.");
        // The window's own creation sent messages to the door from inside the
        // `eval` above; the busy guard declined every one of them, which is
        // correct and which is why this line exists — a nonzero `busy` here is
        // the arrangement working, not a fault.
        println!("macvm-winui: {}", win_wndproc::stats_line());
        // Everything before this point was setup. D5's numbers describe steady
        // state, so the door starts counting from a live, shown window.
        win_wndproc::reset_stats();
        let window = shell_hwnd(&mut vm);
        // WG4 D1: publish the handle the PRIMARY's wake posts to. Until this
        // runs the wake is a no-op and a reply only gets noticed when some
        // other message happens to wake the pump — which works, and hides a
        // latency bug behind whatever else the window was doing. Published
        // once the window is real, cleared when it goes (below).
        publish_ui_hwnd(window.0 as isize);
        // A window owned by another thread would never receive a dispatched
        // message and the app would look hung with nothing in any log, so
        // this is a refusal to start rather than a warning. No window at all
        // is a different case and NOT fatal: `openMain` may have raised (the
        // gate injects exactly that), and the control channel is how anyone
        // finds out — killing the process here would take the diagnosis with
        // it.
        if !window.0.is_null() && !assert_thread_invariant(&mut vm) {
            return 3;
        }

        let mut link = _wired;
        let code = pump(vmp, rx.as_ref(), window, &mut link);
        println!("macvm-winui: {}", win_wndproc::stats_line());
        println!("macvm-winui: message loop ended, exit {code}");
        // The door outlives nothing: unpublish before the box drops, so a late
        // message (Windows can deliver one during teardown) finds a null door
        // and answers `DefWindowProcW` rather than dereferencing a freed VM.
        publish_ui_vm(std::ptr::null_mut());
        code
    }

    /// `--selftest`: the headless stress items from `tests_wg1.md` that need a
    /// process of their own rather than a slot in `it_world` (WG0's Δ 11 —
    /// several VMs share one process there, in parallel threads, and a visible
    /// window with a message loop does not belong in a parallel harness).
    /// Opens and closes a real window ten times, proves `snap` before there is
    /// a window is a named error, and forces `GetMessageW` to answer −1.
    pub fn selftest() -> i32 {
        ensure_message_queue();
        let mut vm = match boot_ui_vm() {
            Ok(vm) => Box::new(vm),
            Err(e) => {
                eprintln!("macvm-winui: {e}");
                return 2;
            }
        };
        // The door is published here too, so `cycle: 10` really does register
        // the door's class and really does create and destroy ten windows with
        // it — the selftest would otherwise prove WG1's arrangement, not WG2's.
        publish_ui_vm(&mut *vm);
        let mut ok = true;

        // 1. snap with no window — a named error, not a hang and not a
        //    zero-byte PNG. Checked FIRST, while there genuinely is no window.
        let (tx, rrx) = std::sync::mpsc::sync_channel::<String>(1);
        snap::snap_hwnd(
            shell_hwnd(&mut vm),
            "target/winui-selftest-nowindow.png",
            tx,
        );
        let reply = rrx.recv().unwrap_or_else(|_| "<no reply>".into());
        println!("SELFTEST snap-before-window: {reply}");
        if reply != "ERR no window yet" {
            ok = false;
        }
        if Path::new("target/winui-selftest-nowindow.png").exists() {
            eprintln!("SELFTEST snap-before-window wrote a file, which it must not");
            ok = false;
        }

        // 2. Open and close a real window ten times in one process: no
        //    class-registration leak, no HWND handed back twice.
        match guarded_eval(&mut vm, "WinShell cycle: 10.") {
            Ok(s) => {
                println!("SELFTEST cycle: {s}");
                if !s.contains("'classUsableEveryTime'->true") || !s.contains("'opened'->10") {
                    ok = false;
                }
            }
            Err(e) => {
                eprintln!("SELFTEST cycle raised: {e}");
                ok = false;
            }
        }

        // 2b. WG2. Ten windows were just created and destroyed with the DOOR
        //     as their wndproc, from inside a doit — so every message those
        //     `CreateWindowExW`/`DestroyWindow` calls sent arrived with the VM
        //     already inside a top-level entry. Every one must have been
        //     declined by the busy guard, the depth counter must be 0, and the
        //     VM must have been entered exactly zero times. That is the
        //     re-entrancy hazard `sprint_wg2_detail.md` does not name, checked
        //     where it actually fires rather than argued about.
        println!("SELFTEST door-after-cycle: {}", win_wndproc::stats_line());
        if win_wndproc::depth() != 0 {
            eprintln!("SELFTEST the depth guard leaked across cycle: 10");
            ok = false;
        }
        if win_wndproc::vm_entries() != 0 {
            eprintln!(
                "SELFTEST the door entered the VM re-entrantly from inside a doit \
                 ({} times) — the busy guard is not doing its job",
                win_wndproc::vm_entries()
            );
            ok = false;
        }
        if win_wndproc::busy_declined() == 0 {
            eprintln!(
                "SELFTEST no message reached the door at all during cycle: 10 — \
                 the class was probably registered with the system default handler, not the door"
            );
            ok = false;
        }
        if win_wndproc::panics_caught() != 0 {
            eprintln!("SELFTEST the trampoline caught a panic, which must never happen");
            ok = false;
        }
        // The address channel (D2) answers the same non-zero number twice, and
        // it is the one Smalltalk actually registered.
        let addr = guarded_eval(&mut vm, "WinApi wndProcAddress.").unwrap_or_default();
        let addr2 = guarded_eval(&mut vm, "WinApi wndProcAddress.").unwrap_or_default();
        let want = format!("{}", win_wndproc::door_address());
        println!("SELFTEST door-address: {addr} (rust says {want})");
        if addr.trim() != want || addr2.trim() != want || want == "0" {
            eprintln!("SELFTEST primitive 272 does not answer the door's address");
            ok = false;
        }

        // 3. `GetMessageW` = −1, forced rather than reasoned about: ask for
        //    messages belonging to a DESTROYED window. Run on a scratch thread
        //    with its own queue so a wrong guess about whether the call blocks
        //    is a reported timeout instead of a hung gate.
        println!("SELFTEST getmessage-minus-one: {}", forced_minus_one());

        // 4. A recovered guest fault leaves the VM usable — the property D4's
        //    second reason depends on, checked without a loop in the way.
        let faulted = guarded_exec(&mut vm, "WinShell error: 'selftest deliberate fault'.");
        let alive = guarded_eval(&mut vm, "3 + 4.").unwrap_or_else(|e| format!("<{e}>"));
        println!(
            "SELFTEST fault-then-alive: faulted={} alive={alive}",
            faulted.is_err()
        );
        if faulted.is_ok() || alive != "7" {
            ok = false;
        }

        publish_ui_vm(std::ptr::null_mut());
        if ok {
            println!("SELFTEST OK");
            0
        } else {
            println!("SELFTEST FAILED");
            1
        }
    }

    /// Call `GetMessageW` filtered to a window handle Win32 does not know.
    /// It validates the handle before it waits and answers −1.
    ///
    /// Bounded by a channel timeout, on a scratch thread with its own queue,
    /// so that if the call ever BLOCKS instead the gate reports
    /// "inconclusive" rather than hanging — a suspected hang gets
    /// instrumentation, never a guess, and never a wedged CI run either.
    fn forced_minus_one() -> String {
        let (tx, rx) = std::sync::mpsc::sync_channel::<i32>(1);
        std::thread::spawn(move || {
            // A dead HWND: any handle value Win32 does not know is invalid to
            // GetMessageW, and one from a window we just destroyed is the
            // honest version of "invalid" — it was real a moment ago.
            let dead = HWND(0xDEAD_BEEFusize as *mut core::ffi::c_void);
            let mut msg = MSG::default();
            let rc = unsafe { GetMessageW(&mut msg, Some(dead), 0, 0) }.0;
            let _ = tx.send(rc);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(rc) => format!(
                "rc={rc} classify={:?} (loop would {})",
                classify(rc),
                match classify(rc) {
                    Step::Failed => "leave, not spin",
                    Step::Quit => "leave",
                    Step::Dispatch => "DISPATCH — wrong",
                }
            ),
            Err(_) => "inconclusive — GetMessageW did not return within 3s".to_string(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The pitfall, made a test. `while GetMessageW(..).as_bool()` — which
        /// is what `gui/src/shell/win.rs::run` still does — treats −1 as TRUE,
        /// because `BOOL(-1).as_bool()` is `-1 != 0`. This loop must not.
        #[test]
        fn minus_one_leaves_the_loop_rather_than_spinning() {
            assert_eq!(classify(-1), Step::Failed);
            assert_eq!(classify(0), Step::Quit);
            assert_eq!(classify(1), Step::Dispatch);
            assert_eq!(classify(27), Step::Dispatch);
            // And the shape of the bug this replaces, spelled out so nobody
            // "simplifies" it back: the naive predicate is true for -1.
            assert!(windows::core::BOOL(-1).as_bool());
        }

        /// The control channel is INHERITED, not rebuilt (§3.1). If this file
        /// ever grows its own PNG writer or its own listener, that is the
        /// sprint's stated failure mode and this catches it.
        #[test]
        fn capture_and_control_are_included_not_copied() {
            let src = include_str!("main.rs");
            assert!(
                src.contains("#[path = \"../../gui/src/shell/snap.rs\"]"),
                "snap must be the shared source from gui/src/shell/snap.rs"
            );
            assert!(
                src.contains("#[path = \"../../gui/src/control.rs\"]"),
                "the control channel must be the shared source from gui/src/control.rs"
            );
            // Split so the needle is not itself a match — this file is its
            // own haystack, and a literal here would fail the assertion it
            // makes, which is a funnier bug than it is a useful one.
            assert!(
                !src.contains(concat!("fn ", "write_png")),
                "writing a second PNG writer is this sprint's named failure mode"
            );
            assert!(
                !src.contains(concat!("fn ", "capture_png")),
                "the capture must be the shared one, not a second implementation"
            );
        }

        /// D1's line, made checkable: this crate pumps and captures, and does
        /// not create, register, style or measure a window. Every one of those
        /// belongs to `world/91_winui_shell.mst`.
        ///
        /// WG2 added exactly one Win32 call to this file — `SetWindowPos`, in
        /// the `resize` control verb — and it is deliberately NOT on this list.
        /// See `resize_window`'s doc: a `WM_SIZE` that reaches Smalltalk has to
        /// originate outside every VM entry, so the scripted resize has to live
        /// where the pump lives. It creates nothing, styles nothing and decides
        /// nothing; it is the user's mouse in a script.
        #[test]
        fn rust_owns_the_pump_and_nothing_else() {
            let src = include_str!("main.rs");
            for forbidden in [
                "CreateWindowExW",
                "RegisterClassW",
                "DwmSetWindowAttribute",
                "SetProcessDpiAwarenessContext",
                "ShowWindow",
                "SetWindowTextW",
            ] {
                assert!(
                    !src.contains(&format!("{forbidden}(")),
                    "{forbidden} is Smalltalk's call to make (D1), not this host's"
                );
            }
        }

        /// WG2's own D1 line: the DOOR is `macvm::runtime::win_wndproc`'s, not
        /// this crate's. A trampoline, an allowlist or a depth counter
        /// re-implemented here would be a second door — the same category of
        /// regression a second PNG writer would be, and this is the same shape
        /// of test §3.1 already earns.
        #[test]
        fn the_door_is_the_shared_one_not_a_local_copy() {
            let src = include_str!("main.rs");
            assert!(
                !src.contains(concat!("extern ", "\"system\" fn")),
                "the wndproc trampoline lives in macvm::runtime::win_wndproc, not here"
            );
            // Comments talk about `DefWindowProcW` constantly — that is the
            // whole subject — so this reads CODE lines only. Same reason
            // `capture_and_control_are_included_not_copied` splits its needles:
            // a file that is its own haystack needs the distinction.
            let needle = concat!("DefWindow", "ProcW");
            for (i, l) in code_lines(src) {
                assert!(
                    !l.contains(needle),
                    "line {i}: this host never answers a message itself; the door does — {l}"
                );
            }
            assert!(
                src.contains(concat!("BusyGuard", "::enter();")),
                "every host-side VM entry must be bracketed (win_wndproc::vm_busy)"
            );
        }

        /// Numbered, non-comment, non-empty lines — the haystack for the two
        /// tests below, which are about what this file DOES and not about what
        /// it says.
        fn code_lines(src: &str) -> impl Iterator<Item = (usize, &str)> {
            src.lines().enumerate().filter_map(|(i, line)| {
                let l = line.trim();
                if l.is_empty() || l.starts_with("//") {
                    None
                } else {
                    Some((i + 1, l))
                }
            })
        }

        /// The bracket, enforced rather than remembered. `CreateWindowExW` and
        /// `SetWindowPos` send messages synchronously, so a bare `vm.eval` /
        /// `vm.exec` anywhere in this file is a re-entrant VM entry waiting for
        /// the right doit — the failure mode being a clobbered `sigsetjmp` slot
        /// and a `longjmp` into a returned frame, which has no symptom until it
        /// has a catastrophic one.
        #[test]
        fn every_vm_entry_is_bracketed() {
            let src = include_str!("main.rs");
            // Exactly two raw entries exist in this file — the bodies of
            // `guarded_eval` and `guarded_exec` — and exactly two brackets.
            // Counting is the enforceable form: a third `vm.eval(` anywhere,
            // however well-intentioned, fails here.
            // The needles are split so this line is not itself a match — the
            // file is its own haystack, and a literal here would fail the
            // assertion it makes.
            let eval = concat!("vm.", "eval(");
            let exec = concat!("vm.", "exec(");
            let raw: Vec<_> = code_lines(src)
                .filter(|(_, l)| l.contains(eval) || l.contains(exec))
                .collect();
            assert_eq!(
                raw.len(),
                2,
                "every VM entry must go through guarded_eval/guarded_exec so the \
                 WG2 door cannot re-enter the VM; found {raw:?}"
            );
            let bracket = concat!("BusyGuard", "::enter()");
            let sites: Vec<_> = code_lines(src)
                .filter(|(_, l)| l.contains(bracket) && !l.contains("concat!"))
                .collect();
            assert_eq!(
                sites.len(),
                2,
                "exactly two bracket sites: guarded_eval and guarded_exec; found {sites:?}"
            );
        }

        /// The allowlist is a CLOSED set and this test is its second signature.
        /// Asserted from THIS side of the crate boundary as well, because the
        /// count is the design decision — "do not route every message" — and
        /// an entry appearing silently is how that decision gets lost.
        ///
        /// The set has grown with the sprints, deliberately each time: WG2's
        /// six, WG3's five (WM_NOTIFY and the four modal-loop transitions),
        /// then WG4's WM_PAINT and WM_DRAWITEM, and the mouse trio plus
        /// WM_MOUSEWHEEL that FreeCell's dragging and the editor's scrolling
        /// asked for — all under flag-and-drain, where a storm of arrivals
        /// coalesces into one drain pass instead of one VM entry each.
        #[test]
        fn the_allowlist_is_the_messages_d1_names() {
            use macvm::runtime::win_wndproc as door;
            assert_eq!(door::ALLOWLIST.len(), 17);
            for m in [
                door::WM_DRAWITEM,
                door::WM_PAINT,
                door::WM_MOUSEWHEEL,
                door::WM_LBUTTONDOWN,
                door::WM_LBUTTONUP,
                door::WM_MOUSEMOVE,
                door::WM_CLOSE,
                door::WM_DESTROY,
                door::WM_SIZE,
                door::WM_COMMAND,
                door::WM_KEYDOWN,
                door::WM_CHAR,
                door::WM_NOTIFY,
                door::WM_ENTERSIZEMOVE,
                door::WM_EXITSIZEMOVE,
                door::WM_ENTERMENULOOP,
                door::WM_EXITMENULOOP,
            ] {
                assert!(door::ALLOWLIST.contains(&m));
            }
            assert!(
                !door::ALLOWLIST.contains(&0x0020),
                "WM_SETCURSOR arrives in storms and must never cross"
            );
            // The drain's own plumbing is handled by the trampoline BEFORE the
            // allowlist and must never be routed to `WinShell`: a heartbeat
            // that cost a VM entry would make the coalescing ratio a
            // measurement of the timer.
            assert!(!door::ALLOWLIST.contains(&door::WM_TIMER));
            assert!(!door::ALLOWLIST.contains(&door::WM_APP_DRAIN));
        }

        /// WG3 D6: the manifest is embedded by the LINKER, from a file this
        /// crate owns. Both halves are checkable without running the binary,
        /// and both have to hold — a `build.rs` naming a file that is not there
        /// fails the build, but a `build.rs` that stopped emitting the link
        /// arguments would silently produce an unthemed app.
        #[test]
        fn the_visual_styles_manifest_is_embedded() {
            let build = include_str!("../build.rs");
            assert!(build.contains("/MANIFEST:EMBED"));
            assert!(build.contains("/MANIFESTINPUT:"));
            let manifest = include_str!("../macvm-winui.manifest");
            assert!(
                manifest.contains("Microsoft.Windows.Common-Controls")
                    && manifest.contains("6.0.0.0"),
                "the manifest must name common controls v6 — without it every \
                 control renders in its Windows-95 skin and nothing else says so"
            );
            // DPI stays a runtime decision Smalltalk makes (WG1 D3); a manifest
            // element here would silently win over it.
            // The needle is the ELEMENT, not the word: the file's own comment
            // explains at length why the element is absent, and a test that
            // failed on its own rationale would be a funnier bug than a useful
            // one (`capture_and_control_are_included_not_copied` splits its
            // needles for the same reason).
            assert!(
                !manifest.contains("<dpiAware"),
                "DPI awareness is SetProcessDpiAwarenessContext's, in Smalltalk"
            );
        }
    }
}

#[cfg(windows)]
fn main() {
    let selftest = std::env::args().any(|a| a == "--selftest");
    let code = if selftest {
        app::selftest()
    } else {
        app::run()
    };
    std::process::exit(code);
}
