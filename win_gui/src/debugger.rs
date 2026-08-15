//! WG7-1 — the Windows bridge for the VM's GUI debugger frontend.
//!
//! The twin of `cocoa_gui/src/debugger.rs`, and deliberately its close copy:
//! the mechanism is platform-neutral (`macvm::runtime::debug::DebugFrontend`)
//! and the hard part was never the frontend — it was having a UI that stays
//! alive while the VM being debugged is frozen.
//!
//! **That part is already solved here.** WG4 D1 put the primary on its own
//! thread and the UI VM on main, so a primary parked inside `debug::halt`
//! blocks nothing: the pump keeps pumping, the window keeps painting, the
//! metrics cluster keeps ticking. `73_cocoadebugger.mst` says the same of the
//! Mac — *"the whole GUI stays live while the primary is frozen in the halt
//! loop — no reentrancy work"* — and it is the whole reason this slice is a
//! port rather than a project.
//!
//! # Blast, don't patch
//!
//! The halt publishes its FULL report and the view renders it wholesale. Same
//! rule as the Monitor, the metrics cluster and the Editor's viewport, and for
//! the same reason 60's own note gives: the incremental alternative drifted.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

use macvm::embed::VmHandle;
use macvm::runtime::debug::DebugFrontend;
use macvm::runtime::workers::InboxWakeFn;

/// The latest full report (`RUNNING\n` when nothing is halted).
static REPORT: Mutex<String> = Mutex::new(String::new());
/// A fresh report arrived — the pump pushes it into the guest.
static HALT_ARRIVED: AtomicBool = AtomicBool::new(false);
/// The live command sender, replaced per primary GENERATION. WG7-3 makes
/// generations a real thing here: a restart installs a new frontend, and a
/// stale sender to the dead generation's channel errors harmlessly.
static CMD_TX: Mutex<Option<Sender<String>>> = Mutex::new(None);
/// Debug ▸ Halt on Error. Default ON, which is the point of a debugger.
pub static HALT_ON_ERROR: AtomicBool = AtomicBool::new(true);

struct GuiFrontend {
    rx: Mutex<Receiver<String>>,
    wake: InboxWakeFn,
}

impl DebugFrontend for GuiFrontend {
    fn publish(&self, report: &str) {
        if let Ok(mut cell) = REPORT.lock() {
            report.clone_into(&mut cell);
        }
        HALT_ARRIVED.store(true, Ordering::Release);
        // FIRE THE WAKE OURSELVES. The primary's beat is what normally nudges
        // the UI, and it is parked inside this very halt — so without this the
        // report would sit until something else happened to wake the pump. The
        // Mac's frontend does the same and for the same reason.
        (self.wake)();
    }

    fn next_command(&self) -> String {
        // A dead channel RESUMES rather than hangs. Belt and braces: the
        // sender is only replaced when a new generation installs, and a
        // generation cannot be replaced while it is parked in its own halt —
        // but a debugger that can hang the VM it is debugging is worse than
        // one that occasionally continues.
        self.rx
            .lock()
            .ok()
            .and_then(|rx| rx.recv().ok())
            .unwrap_or_else(|| "continue".into())
    }
}

/// Install the frontend on a fresh primary generation. Called from the
/// primary's own thread at boot AND after every WG7-3 restart, exactly as the
/// Mac re-installs on every respawn.
pub fn install(primary: &mut VmHandle, ui_wake: InboxWakeFn) {
    let (tx, rx) = channel();
    if let Ok(mut slot) = CMD_TX.lock() {
        *slot = Some(tx);
    }
    if let Ok(mut cell) = REPORT.lock() {
        "RUNNING\n".clone_into(&mut cell);
    }
    primary.set_debug_frontend(std::sync::Arc::new(GuiFrontend {
        rx: Mutex::new(rx),
        wake: ui_wake,
    }));
    primary.set_halt_on_error(HALT_ON_ERROR.load(Ordering::Acquire));
    // The old generation's halt state went with it; make the view re-render
    // so it stops showing a stack from a VM that no longer exists.
    HALT_ARRIVED.store(true, Ordering::Release);
}

/// The pump: is there a fresh report to push?
pub fn take_halt_arrived() -> bool {
    HALT_ARRIVED.swap(false, Ordering::AcqRel)
}

/// The current full report.
pub fn report() -> String {
    REPORT.lock().map(|r| r.clone()).unwrap_or_default()
}

/// Is the primary parked in a halt right now?
pub fn halted() -> bool {
    !REPORT
        .lock()
        .map(|r| r.starts_with("RUNNING"))
        .unwrap_or(true)
}

/// One command line into the parked halt loop.
///
/// **DROPPED when nothing is halted**, and that is not defensive coding — it
/// is a measured defect on the Mac: a command sent while RUNNING sits in the
/// channel and is consumed the instant the NEXT halt parks, so an exploratory
/// `continue` silently resumes the breakpoint you just planted. The same
/// channel has the same behaviour here, so it gets the same guard.
pub fn send_command(line: String) -> bool {
    if !halted() {
        return false;
    }
    if let Ok(slot) = CMD_TX.lock() {
        if let Some(tx) = slot.as_ref() {
            return tx.send(line).is_ok();
        }
    }
    false
}
