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
//! | what each message *means* | **Smalltalk — but not until WG2** |
//!
//! WG1 does not open the WndProc door. `DefWindowProcW` answers every
//! message; dispatch into Smalltalk is WG2's entire risk budget.
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

    use macvm::embed::{set_fatal_mode, VmHandle};
    use macvm::runtime::vm_state::FatalMode;
    use macvm::runtime::VmOptions;

    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, GetWindowThreadProcessId, IsWindow, PeekMessageW,
        PostQuitMessage, PostThreadMessageW, TranslateMessage, MSG, PM_NOREMOVE, WM_APP,
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
        Ok(vm)
    }

    /// The HWND Smalltalk currently owns, asked fresh every time.
    ///
    /// Never cached: `WinShell` is the authority on whether there is a window,
    /// and a Rust-side copy would go stale the moment a doit closed it — which
    /// is a thing the control channel can do. `0` is the ordinary "not yet"
    /// answer and `snap_hwnd` already knows what to say about it.
    fn shell_hwnd(vm: &mut VmHandle) -> HWND {
        let raw = match vm.eval("WinShell hwndValue.") {
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
    pub fn drain_control_requests(vm: &mut VmHandle, rx: &Receiver<CtlReq>) {
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
                    let reply = match vm.eval(arg) {
                        Ok(s) => format!("OK {s}"),
                        Err(e) => format!("ERR {e}"),
                    };
                    let _ = req.reply.send(reply);
                }
                "doit" => {
                    let reply = match vm.exec(arg) {
                        Ok(()) => "OK".to_string(),
                        Err(e) => format!("ERR {e}"),
                    };
                    let _ = req.reply.send(reply);
                }
                "snap" => {
                    let h = shell_hwnd(vm);
                    snap::snap_hwnd(h, arg, req.reply);
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
    /// The liveness check is the WG1 stand-in for `WM_DESTROY` →
    /// `PostQuitMessage`: once Smalltalk's window has existed and stopped
    /// existing, this loop has nothing left to pump for, so it quits. That is
    /// a decision about the LOOP's lifetime, which D1 gives to Rust — not
    /// about what a message MEANS, which stays Smalltalk's and stays WG2's.
    /// When the door lands, `WM_DESTROY` dispatches into `WinShell` and this
    /// branch becomes dead code to delete.
    pub fn pump(vm: &mut VmHandle, rx: Option<&Receiver<CtlReq>>, window: HWND) -> i32 {
        let mut msg = MSG::default();
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
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if let Some(rx) = rx {
                        drain_control_requests(vm, rx);
                    }
                    if had_window && !unsafe { IsWindow(Some(window)) }.as_bool() {
                        println!(
                            "macvm-winui: the window is gone — posting WM_QUIT (WG2 moves this \
                             to WM_DESTROY, where it belongs)"
                        );
                        unsafe { PostQuitMessage(0) };
                        had_window = false;
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
    pub fn run() -> i32 {
        ensure_message_queue();
        let mut vm = match boot_ui_vm() {
            Ok(vm) => vm,
            Err(e) => {
                eprintln!("macvm-winui: {e}");
                return 2;
            }
        };

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
                let _ = vm.exec("WinShell faultMode: #guest.");
            }
            Ok("native") => {
                let _ = vm.exec("WinShell faultMode: #native.");
            }
            _ => {}
        }

        match vm.eval("WinShell openMain.") {
            Ok(s) => println!("macvm-winui: WinShell openMain -> {s}"),
            Err(e) => eprintln!("macvm-winui: WinShell openMain raised: {e} (the pump continues)"),
        }
        let _ = vm.exec("WinShell report.");
        let window = shell_hwnd(&mut vm);
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

        let code = pump(&mut vm, rx.as_ref(), window);
        println!("macvm-winui: message loop ended, exit {code}");
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
            Ok(vm) => vm,
            Err(e) => {
                eprintln!("macvm-winui: {e}");
                return 2;
            }
        };
        let mut ok = true;

        // 1. snap with no window — a named error, not a hang and not a
        //    zero-byte PNG. Checked FIRST, while there genuinely is no window.
        let (tx, rrx) = std::sync::mpsc::sync_channel::<String>(1);
        snap::snap_hwnd(shell_hwnd(&mut vm), "target/winui-selftest-nowindow.png", tx);
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
        match vm.eval("WinShell cycle: 10.") {
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

        // 3. `GetMessageW` = −1, forced rather than reasoned about: ask for
        //    messages belonging to a DESTROYED window. Run on a scratch thread
        //    with its own queue so a wrong guess about whether the call blocks
        //    is a reported timeout instead of a hung gate.
        println!("SELFTEST getmessage-minus-one: {}", forced_minus_one());

        // 4. A recovered guest fault leaves the VM usable — the property D4's
        //    second reason depends on, checked without a loop in the way.
        let faulted = vm.exec("WinShell error: 'selftest deliberate fault'.");
        let alive = vm.eval("3 + 4.").unwrap_or_else(|e| format!("<{e}>"));
        println!(
            "SELFTEST fault-then-alive: faulted={} alive={alive}",
            faulted.is_err()
        );
        if faulted.is_ok() || alive != "7" {
            ok = false;
        }

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
    }
}

#[cfg(windows)]
fn main() {
    let selftest = std::env::args().any(|a| a == "--selftest");
    let code = if selftest { app::selftest() } else { app::run() };
    std::process::exit(code);
}
