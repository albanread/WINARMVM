//! WINARM (WG2) — **the WndProc door**: Win32 calls Smalltalk.
//!
//! Everything before this sprint was "can Smalltalk call Win32". This module is
//! the other direction. One `extern "system"` trampoline is registered as a
//! window class's `lpfnWndProc`; Windows calls it for every message that window
//! ever receives, and for a small, stated set it forwards
//! `(hwnd, msg, wParam, lParam)` into the UI VM as a **top-level entry** —
//! `WinShell class>>window:message:wParam:lParam:` — which answers either an
//! `LRESULT` or the Symbol `#defwindowproc`.
//!
//! This is `runtime::objc_delegate`'s twin (C6 reverse dispatch) and the
//! comparison is what makes it small: one uniform four-word signature, no
//! runtime class synthesis, no IMP shapes, no exceptions to unwind — Win32
//! reports failure by value. What is left hard is re-entrancy and crash safety,
//! and that is where the whole of this file's care goes.
//!
//! ## Why this lives in the CORE crate and not in `win_gui`
//!
//! `sprint_wg2_detail.md`'s deliverables put "the trampoline, the guard, the
//! allowlist, **and a primitive** publishing the trampoline's address" in
//! `win_gui`. The last of those is not possible: primitive numbers and the
//! `PRIMITIVES` table are `macvm`'s, a downstream crate cannot add a row to it,
//! and a guest that could only reach the door when one particular *binary* was
//! the host would leave `world/tests/62_winui_door_tests.mst` unable to see the
//! address at all under the plain CLI. So the door is core — exactly where its
//! Cocoa twin already is (`runtime::objc_delegate` is core; `cocoa_gui` is the
//! thin host) — and `win_gui` keeps the pump and publishes the VM pointer. That
//! also preserves `win_gui`'s own stated deliverable, which is its thinness.
//!
//! ## D1 — the allowlist, and why it is a closed set
//!
//! `WM_MOUSEMOVE`, `WM_NCHITTEST` and `WM_SETCURSOR` arrive in storms. Routing
//! every message would make the door the slowest thing in the system and
//! multiply the re-entrancy surface for nothing. [`ALLOWLIST`] is the whole set
//! that crosses; everything else goes straight to `DefWindowProcW` without the
//! VM ever being asked. The list is two-sided: Rust asks only for these, and
//! `WinShell` answers `#defwindowproc` for anything it does not itself handle,
//! so the two halves can disagree safely — but only in the direction of doing
//! *less*.
//!
//! **WG4 D5 admitted `WM_MOUSEMOVE`, deliberately, and it is the exception
//! that proves the rule.** The transcript splitter is dragged, and a drag is
//! made of exactly the message this list was written to keep out. Three things
//! make the cost acceptable rather than merely tolerated:
//!
//! * the allowlist scan is the FIRST thing `macvm_wndproc` does, so an
//!   off-drag storm is a handful of integer compares and a `DefWindowProcW`;
//! * the door only ever RECORDS (§2.4a flag-and-drain), so a move that
//!   arrives mid-drag costs a flag, not a VM entry;
//! * `WinShell` answers `#defwindowproc` when no drag is active, which is the
//!   two-sided property above doing its job — the expensive half is gated by
//!   the side that knows whether it is needed.
//!
//! `WM_NCHITTEST` and `WM_SETCURSOR` stay off the list and always have. If a
//! future sprint wants one of them, it owes the same three-part argument.
//!
//! ## D3 — the depth guard, and exactly what it covers
//!
//! `DispatchMessageW` re-enters: `DefWindowProcW` generates nested messages, a
//! `SendMessageW` from inside a handler runs another wndproc call on the same
//! thread, and a modal drag loop pumps internally. A VM top-level entry inside a
//! VM top-level entry is not safe. So: a thread-local depth counter; a nested
//! entry answers `DefWindowProcW` without touching the VM.
//!
//! **The guard covers the VM entry ONLY, never the `DefWindowProcW` call**, and
//! that scoping is a decision, not an accident — it is the answer to this
//! sprint's named pitfall (`WM_DESTROY` arrives during `DestroyWindow`, which
//! the door may itself have caused). With the tight scope:
//!
//! ```text
//! WM_CLOSE   → depth 0→1 → WinShell answers #defwindowproc → depth 1→0
//!            → DefWindowProcW(WM_CLOSE) → DestroyWindow
//!              → WM_DESTROY → depth 0→1 → WinShell → PostQuitMessage(0)
//! ```
//!
//! `WM_DESTROY` is therefore NOT nested and reaches Smalltalk, which is what
//! `tests_wg2.md` item 3 asks for. Had the guard wrapped the default call too,
//! `WM_DESTROY` would have been declined as a nested entry and the loop would
//! never have ended — the pitfall, made harmless by where one brace goes.
//!
//! ## D4 — and the leak this sprint most fears
//!
//! P2's recovery is a **non-unwinding `longjmp`**: it skips ordinary returns.
//! A guard released by a plain decrement after the VM entry would therefore
//! leak on every recovered fault, and the door would be silently, permanently
//! disabled — the symptom being "Smalltalk stopped receiving messages" with
//! nothing in any log. The release is an RAII [`Drop`] ([`DepthGuard`]), and
//! `dispatch_callback`'s `longjmp` lands *inside* this frame (its `sigsetjmp`
//! is deeper), so the guard's scope ends normally on every path — fault
//! included. [`depth_guard_drops_on_early_return`] and
//! [`depth_returns_to_zero_after_a_recovered_guest_fault`] are the two tests
//! that hold this down.
//!
//! A Rust `panic!` unwinding into Win32's dispatcher is undefined behaviour, so
//! the whole trampoline body is inside `catch_unwind` and a caught panic answers
//! `DefWindowProcW`.

#![cfg(windows)]
#![allow(unsafe_code)]

use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::oops::smi::SmallInt;
use crate::oops::wrappers::{KlassOop, MemOop};
use crate::oops::Oop;
use crate::runtime::vm_state::VmState;

// ── the one Win32 import, and the clock ──────────────────────────────────────
//
// Declared rather than depended on: `macvm` links no `windows` crate (only
// `win_gui` and `gui` do), and the door needs exactly one user32 export plus
// the two kernel32 counters D5 names. Four extern lines is a smaller price
// than a new dependency for the whole core crate.

#[link(name = "user32")]
extern "system" {
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    /// WG3 D1's wake. **`PostMessageW`, never `SendMessageW`** — a send would
    /// call this very trampoline synchronously from inside itself, which is the
    /// re-entrancy the whole flag-and-drain pattern exists to avoid.
    /// `drain_wake_is_post_not_send` is the test that says so.
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn QueryPerformanceCounter(counter: *mut i64) -> i32;
    fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
}

fn qpc() -> i64 {
    let mut t: i64 = 0;
    unsafe { QueryPerformanceCounter(&mut t) };
    t
}

/// Ticks per second — read once; Windows guarantees it is fixed at boot.
pub fn qpc_frequency() -> i64 {
    static FREQ: AtomicU64 = AtomicU64::new(0);
    let cached = FREQ.load(Ordering::Relaxed);
    if cached != 0 {
        return cached as i64;
    }
    let mut f: i64 = 0;
    unsafe { QueryPerformanceFrequency(&mut f) };
    FREQ.store(f as u64, Ordering::Relaxed);
    f
}

// ── D1: the allowlist ────────────────────────────────────────────────────────

/// `WM_CLOSE` — the app decides whether to close.
pub const WM_CLOSE: u32 = 0x0010;
/// `WM_DESTROY` — must `PostQuitMessage(0)`; WG1 explicitly deferred this here.
pub const WM_DESTROY: u32 = 0x0002;
/// `WM_SIZE` — layout is Smalltalk's job from WG3 on.
pub const WM_SIZE: u32 = 0x0005;
/// `WM_COMMAND` — menus and controls (WG3+).
pub const WM_COMMAND: u32 = 0x0111;
/// `WM_KEYDOWN` — the editor and shortcuts (WG5+).
pub const WM_KEYDOWN: u32 = 0x0100;
/// `WM_CHAR` — likewise.
pub const WM_CHAR: u32 = 0x0102;
/// `WM_NOTIFY` — WG3: how `SysListView32`/`SysTreeView32` speak to their
/// parent. Its `lParam` is an `NMHDR*` that is valid **only during the call**;
/// see [`read_nmhdr`].
pub const WM_NOTIFY: u32 = 0x004E;
/// `WM_ENTERSIZEMOVE` — WG3 D2: a modal move/size loop is starting, and it
/// pumps the queue itself. Drains are suppressed until its twin arrives.
pub const WM_ENTERSIZEMOVE: u32 = 0x0231;
/// `WM_EXITSIZEMOVE` — tracking ends; the accumulated flags get **one** pass.
pub const WM_EXITSIZEMOVE: u32 = 0x0232;
/// `WM_ENTERMENULOOP` — the other modal pump Windows runs behind our back.
pub const WM_ENTERMENULOOP: u32 = 0x0211;
/// `WM_EXITMENULOOP` — likewise.
pub const WM_EXITMENULOOP: u32 = 0x0212;
/// `WM_TIMER` — the drain's **heartbeat**, not its wake (WG3's pitfall list:
/// `WM_TIMER` is low-resolution and coalesces already). It catches a flag that
/// was set with no message following, and a `drainPass` that asked to be run
/// again. Never routed to `WinShell`: it is host machinery, and the guest's
/// timer is only its clock.
pub const WM_TIMER: u32 = 0x0113;

/// The drain's **wake**: a private `WM_APP+n`, posted (never sent) by
/// [`request_drain`]. `WM_APP` is 0x8000 and everything from there up is the
/// application's own; +7 is arbitrary and reserved here so WG4's shell can pick
/// its own numbers without colliding.
pub const WM_APP_DRAIN: u32 = 0x8000 + 7;

/// The wake must be in the application's own range. A number below `WM_APP`
/// would collide with something Windows already means, and the symptom would be
/// a window that behaves oddly rather than a drain that fails — so this is a
/// BUILD error rather than a test.
const _WM_APP_DRAIN_IS_PRIVATE: () = assert!(WM_APP_DRAIN >= 0x8000);

/// **The closed set.** A message not named here never reaches the VM entry
/// point, whatever `WinShell` may or may not implement — `allowlist_is_a_closed_set`
/// is the test, and the probe counter it reads is [`vm_entries`].
///
/// `WM_PAINT` is deliberately absent: WG2 kept `DefWindowProcW`'s erase, and
/// WG3's controls paint themselves (that is what visual styles v6 buys), so
/// nothing here needs a paint handler yet. Eleven messages, and the fact that
/// it is eleven rather than "all of them" is the design decision, not an
/// omission.
///
/// `WM_TIMER` and [`WM_APP_DRAIN`] are **not** on it either, and that is a
/// different kind of absence: they are handled by the trampoline itself,
/// *before* the allowlist test, because they are the drain's own plumbing
/// rather than messages whose meaning is Smalltalk's. See [`service_drain`].
///
/// These are transcribed here rather than asked of winkb, and that is the one
/// place in this layer where transcription is right: they are ABI-frozen
/// message numbers a Rust `const` must have at compile time, the database is a
/// machine-local artifact this crate must build without, and
/// `allowlist_matches_winkb` (world side, `62_winui_door_tests.mst`) checks
/// every one of them against winkb when the database IS present — so a typo is
/// caught by data rather than trusted to care.
/// `WM_DRAWITEM` (0x2B). WG4 D3: an owner-drawn control asks its PARENT to
/// paint it. Allowlisted for the same reason `WM_NOTIFY` is — its meaning is
/// Smalltalk's — but it is the one message whose handler may not simply flag
/// and return: a paint cannot be deferred to the drain without leaving the
/// strip blank until something else repaints it.
///
/// The permission is narrow and stated (sprint_wg4_detail.md D3): painting is
/// a RESPONSE, not work — bounded, allocation-free, and re-entrancy-free by
/// construction, because every GDI call goes into a DC Windows just handed us
/// and none of them sends a message back. The handler draws from fields
/// already copied out of the struct, calls no VM entry of its own, and touches
/// no control other than the one being drawn.
pub const WM_DRAWITEM: u32 = 0x002B;

/// WG4 D5: the three messages a splitter drag is made of. Allowlisted as a
/// set, because any one without the others is a drag that starts and never
/// ends (or ends without having begun) — and a captured mouse that never sees
/// its button-up leaves the pointer captured for the life of the window.
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_MOUSEMOVE: u32 = 0x0200;

pub const ALLOWLIST: [u32; 15] = [
    WM_DRAWITEM,
    WM_LBUTTONDOWN,
    WM_LBUTTONUP,
    WM_MOUSEMOVE,
    WM_CLOSE,
    WM_DESTROY,
    WM_SIZE,
    WM_COMMAND,
    WM_KEYDOWN,
    WM_CHAR,
    WM_NOTIFY,
    WM_ENTERSIZEMOVE,
    WM_EXITSIZEMOVE,
    WM_ENTERMENULOOP,
    WM_EXITMENULOOP,
];

/// The messages whose arrival **asks for a drain pass**.
///
/// This is the flag-and-drain split made concrete: the door records the
/// message (a shallow VM entry that sets a Smalltalk flag and returns) and
/// notes here that *some* deferred work now exists. The pass itself decides
/// what that work is — Rust never knows.
///
/// It is a Rust-side set rather than a guest decision for one reason:
/// `dispatch_callback` has a single `u64` return that is already spoken for by
/// the LRESULT, so a handler cannot say "and drain me" in the same breath. The
/// guest's escape hatch is the other end — `drainPass` answering non-zero means
/// "there is more, run me again" — which is exactly the case Rust cannot know.
pub const DRAIN_REQUESTING: [u32; 3] = [WM_SIZE, WM_COMMAND, WM_NOTIFY];

/// Is `msg` one the door forwards? A plain linear scan over six `u32`s — this
/// runs per message, and six compares is cheaper than any hash.
#[inline]
pub fn is_allowlisted(msg: u32) -> bool {
    if !door_enabled() {
        return false;
    }
    ALLOWLIST.contains(&msg)
}

/// Master switch, for D5's baseline and for implementation-order step 1
/// ("register the door with every message still defaulted and prove WG1's
/// entire gate passes unchanged"). Off ⇒ the allowlist is empty ⇒ the door is
/// transparent: registered, called for every message, and answering exactly
/// what `DefWindowProcW` would have answered.
///
/// Defaults ON. `MACVM_WINUI_DOOR=off` turns it off at startup, and
/// [`set_door_enabled`] (reached from the control channel) toggles it live, so
/// the two D5 numbers can be measured in ONE warm process if wanted.
static DOOR_ENABLED: AtomicBool = AtomicBool::new(true);
static DOOR_ENV_READ: AtomicBool = AtomicBool::new(false);

pub fn door_enabled() -> bool {
    if !DOOR_ENV_READ.swap(true, Ordering::Relaxed) {
        if let Ok(v) = std::env::var("MACVM_WINUI_DOOR") {
            let v = v.trim().to_ascii_lowercase();
            if v == "off" || v == "0" || v == "false" {
                DOOR_ENABLED.store(false, Ordering::Relaxed);
            }
        }
    }
    DOOR_ENABLED.load(Ordering::Relaxed)
}

pub fn set_door_enabled(on: bool) {
    DOOR_ENV_READ.store(true, Ordering::Relaxed);
    DOOR_ENABLED.store(on, Ordering::Relaxed);
}

// ── D3: the depth guard ──────────────────────────────────────────────────────

thread_local! {
    /// How many door entries are live on THIS thread. `DispatchMessageW` is
    /// re-entrant and the VM's top-level entry is not, so anything above 0
    /// means "answer `DefWindowProcW`, do not touch the VM".
    ///
    /// Thread-local, not global: a wndproc runs on the thread that created its
    /// window, several VMs share one process under `cargo test`, and a global
    /// counter would let one thread's nesting mute another thread's door.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// The current door depth on this thread. 0 between messages — which is what
/// `tests_wg2.md` item 6 reads back, through the control channel's `door` verb.
pub fn depth() -> u32 {
    DEPTH.with(Cell::get)
}

thread_local! {
    /// How many *host-initiated* top-level VM entries (`eval`/`exec`) are live
    /// on this thread. See [`vm_busy`].
    static VM_BUSY: Cell<u32> = const { Cell::new(0) };

    /// Set once a `WM_DESTROY` has crossed into Smalltalk and been ANSWERED
    /// (an LRESULT, not a default). See [`guest_handled_destroy`].
    static GUEST_HANDLED_DESTROY: Cell<bool> = const { Cell::new(false) };
}

/// Has Smalltalk handled this thread's `WM_DESTROY`?
///
/// The host keeps a liveness backstop — "the window is gone and nobody posted
/// the quit, so post it" — because a `WM_DESTROY` handler that raised would
/// otherwise leave the process pumping an empty queue forever, which is exactly
/// the WG1 Δ 2 hang this port has already paid for once. But a backstop that
/// fires when the guest DID post the quit makes gate item 3 ("`WM_DESTROY`
/// posts the quit from Smalltalk, not from Rust") unprovable, and it did fire
/// on the first run: the pump's `IsWindow` check is *after* the dispatch that
/// destroyed the window, so it always sees the corpse.
///
/// So the backstop asks this instead of asking Win32 alone. Rust cannot know
/// what the handler *did*, but it knows the handler ran and answered — which,
/// given `WinShell>>onDestroy` is the only thing on the other side, is the same
/// question one indirection out.
pub fn guest_handled_destroy() -> bool {
    GUEST_HANDLED_DESTROY.with(Cell::get)
}

/// Is a host-initiated top-level VM entry (`eval`/`exec`) already running on
/// this thread?
///
/// **This guard is not in the sprint spec and the door is unsound without it.**
/// D3 names one re-entrancy source — `DispatchMessageW` re-entering itself —
/// and [`DEPTH`] answers that one. There is a second, and it fires on the very
/// first run: `CreateWindowExW` and `SetWindowPos` **send** `WM_SIZE`
/// synchronously, and `openMain` is called from `vm.eval`. So the trampoline is
/// invoked *inside* a live `eval`, with `DEPTH` legitimately 0.
///
/// That is exactly the corruption D3 exists to prevent, arriving by a door D3
/// did not name. `deopt_trap::claim_jmp_slot` hands out **one slot per thread**
/// (it is keyed by thread id and reuses the caller's own slot), so a nested
/// entry's `sigsetjmp` OVERWRITES the outer `eval`'s saved context; when the
/// nested entry returns, the outer `eval`'s recovery buffer points into a dead
/// frame, and a later fault in that same `eval` would `longjmp` into it. The
/// single idle-baseline watermark is overwritten the same way.
///
/// So the host brackets every `eval`/`exec` with a [`BusyGuard`] and the door
/// declines while one is live. The consequence is worth stating because it
/// shapes how WG3 must test: **a Win32 call made from a doit cannot get its
/// synchronous messages to Smalltalk.** A resize has to arrive from outside
/// every VM entry — from the pump, from the modal move/size loop a user's drag
/// runs, or from the control channel's drain — which is what real messages do
/// anyway.
///
/// The right long-term home for this flag is `eval`/`exec` themselves, so every
/// host gets it without cooperating; that is a core change with a blast radius
/// bigger than this sprint's, recorded in the WG2 Δ instead.
pub fn vm_busy() -> bool {
    VM_BUSY.with(Cell::get) > 0
}

/// Marks this thread as inside a host-initiated top-level VM entry for as long
/// as it lives. RAII for the same reason [`DepthGuard`] is — `eval`'s own
/// recovery arms return normally, but a host that decremented by hand would
/// leak the flag on its own `?` and disable the door with no symptom.
pub struct BusyGuard(());

impl BusyGuard {
    pub fn enter() -> BusyGuard {
        VM_BUSY.with(|c| c.set(c.get() + 1));
        BusyGuard(())
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        VM_BUSY.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

// ── WG3 D1/D2: the drain ─────────────────────────────────────────────────────
//
// `win_gui_design.md` §2.4a is the specification, and its whole content is a
// split: the DOOR records a flag and returns; a separate DRAIN pass, running
// with the VM quiescent and provably not inside a callback, does the real work
// with a fresh top-level entry. WG2's depth guard makes nesting SAFE; this
// makes nesting NOT HAPPEN — which matters because the Cocoa side measured what
// inline work costs (a refresh run inside a callback fails closed behind the
// guard with nothing in any log, and once JIT-compiled corrupts the tier-link
// invariant `walk_frames` asserts).
//
// Three thread-local bits carry it, and the difference between the first two is
// the coalescing claim:
//
//   REQUESTED  "there is deferred work" — set by the door, cleared by the pass
//   POSTED     "a WM_APP_DRAIN is already in the queue" — the latch that stops
//              N messages from queueing N wakes. Without it the work would
//              still coalesce but the PASSES would not, and `drainPasses <
//              messages` is the gate.
//   TRACKING   D2: a modal move/size or menu loop is pumping the queue itself.

thread_local! {
    /// Deferred work exists. Set by the door for a [`DRAIN_REQUESTING`]
    /// message, and by a `drainPass` that answered non-zero ("run me again").
    static DRAIN_REQUESTED: Cell<bool> = const { Cell::new(false) };
    /// A `WM_APP_DRAIN` is in the queue and has not been consumed yet.
    static DRAIN_POSTED: Cell<bool> = const { Cell::new(false) };
    /// D2's suppression, as a COUNTER rather than a flag: a menu loop can open
    /// inside a size loop, and a bool would be cleared by the inner one's exit
    /// while the outer was still tracking.
    static TRACKING: Cell<u32> = const { Cell::new(0) };
}

/// Is a modal move/size or menu loop pumping this thread's queue right now?
///
/// **Windows has no `NSDefaultRunLoopMode`.** Cocoa restricts its drain to that
/// mode and therefore never runs one mid-tracking for free; here the modal loop
/// runs its OWN `GetMessage`/`DispatchMessage`, so a posted `WM_APP_DRAIN` and a
/// `WM_TIMER` are both delivered *inside* it. Without this flag a window drag
/// would run a full layout pass per mouse-move, from inside a modal loop, with
/// the VM entered from a stack frame the pump has never seen.
pub fn tracking() -> bool {
    TRACKING.with(Cell::get) > 0
}

/// Test/host hook: force the tracking state without a real modal loop. The gate
/// drives the real thing (`WM_ENTERSIZEMOVE` sent from the control drain);
/// this exists so `tracking_flag_suppresses_drain` needs neither a window nor a
/// mouse.
pub fn set_tracking(on: bool) {
    TRACKING.with(|c| {
        if on {
            c.set(c.get() + 1)
        } else {
            c.set(c.get().saturating_sub(1))
        }
    });
}

/// Is there deferred work waiting for a pass?
pub fn drain_requested() -> bool {
    DRAIN_REQUESTED.with(Cell::get)
}

/// **The door's half of D1**: note that deferred work exists and, unless a wake
/// is already queued or a modal loop is pumping, post one.
///
/// `PostMessageW` and never `SendMessageW`: a send would run this trampoline
/// again, synchronously, from inside itself — the exact re-entrancy the pattern
/// exists to avoid, and the reason `drain_wake_is_post_not_send` exists.
pub fn request_drain(hwnd: isize) {
    DRAIN_REQUESTED.with(|c| c.set(true));
    bump(|c| c.drain_requests += 1);
    if hwnd == 0 || tracking() || DRAIN_POSTED.with(Cell::get) {
        return;
    }
    if unsafe { PostMessageW(hwnd, WM_APP_DRAIN, 0, 0) } != 0 {
        DRAIN_POSTED.with(|c| c.set(true));
        bump(|c| c.drain_posts += 1);
    }
}

/// `WM_APP_DRAIN` (the wake) and `WM_TIMER` (the heartbeat) both land here.
///
/// Answers `true` when a `drainPass` actually ran. Everything else — tracking,
/// nothing to do, the VM busy — is a cheap no-op that leaves the request
/// standing, so the next wake or the next heartbeat services it. That is what
/// makes this safe to call from inside a modal loop's dispatch: it declines.
fn service_drain(hwnd: isize) -> bool {
    // This wake has been consumed, whatever we do with it.
    DRAIN_POSTED.with(|c| c.set(false));

    // D2. The flags accumulate and ONE pass runs on exit — which is where
    // `WM_EXITSIZEMOVE` posts a fresh wake. Deliberately does NOT re-post here:
    // during a drag `request_drain` is already declining to post, so a
    // suppressed pass that re-posted would spin the modal loop for nothing.
    if tracking() {
        bump(|c| c.drain_suppressed += 1);
        return false;
    }
    if !drain_requested() {
        // The heartbeat with nothing to do — the overwhelmingly common case, and
        // it must not cost a VM entry. This is why `drainPasses` counts real
        // work rather than clock ticks, which is what makes the coalescing ratio
        // a measurement of the drain rather than of the timer.
        bump(|c| c.drain_idle += 1);
        return false;
    }
    // The same two guards the message door has, for the same two reasons: a
    // pass is a top-level VM entry and must not be nested inside another one.
    // Declining leaves REQUESTED set, so the heartbeat retries.
    if depth() > 0 || vm_busy() {
        bump(|c| c.drain_declined += 1);
        return false;
    }
    let vmp = crate::embed::ui_vm();
    if vmp.is_null() {
        bump(|c| c.drain_declined += 1);
        return false;
    }
    // Cleared BEFORE the pass, not after: a flag the handler sets while the
    // pass is running is new work, and clearing afterwards would lose it. The
    // pass's own non-zero answer is how it says so.
    DRAIN_REQUESTED.with(|c| c.set(false));

    let _guard = DepthGuard::enter();
    bump(|c| c.drain_passes += 1);
    // SAFETY: as `cross_into_smalltalk` — the handle is boxed and published by
    // the host on this thread, and the depth guard above establishes that no
    // other `&mut VmState` is live.
    let handle: &mut crate::embed::VmHandle = unsafe { &mut *vmp };
    let raw = handle.dispatch_callback(NOT_HANDLED, perform_drain_pass);
    // The guest's escape hatch, and the answer to this sprint's own pitfall
    // ("do not post the drain message from inside the drain"): a pass that
    // discovered more work says so by ANSWERING non-zero, the request is left
    // standing, and the HEARTBEAT services it. Posting from here would let a
    // handler that always answers non-zero spin the pump.
    if raw != NOT_HANDLED && raw != 0 {
        DRAIN_REQUESTED.with(|c| c.set(true));
        bump(|c| c.drain_reasked += 1);
    }
    let _ = hwnd;
    true
}

/// Inside the recovery door, with a live `&mut VmState`: send
/// `WinShell class>>drainPass`.
///
/// A `SmallInteger` answer is "how much is left" — non-zero means run me again.
/// Anything else (no `WinShell`, no `drainPass`, a raise, a recovered fault) is
/// [`NOT_HANDLED`], which the caller reads as "nothing more", so a pass that
/// blows up is not retried forever. `tests_wg3.md`'s stress row asks for exactly
/// that: the pass completes, the flag is cleared, the next pass runs normally.
fn perform_drain_pass(vm: &mut VmState) -> u64 {
    let Some(cls) = win_shell_class(vm) else {
        return NOT_HANDLED;
    };
    let sel = vm.universe.intern(b"drainPass");
    let k = crate::runtime::primitives::klass_of(vm, cls);
    let Some(m) = crate::runtime::lookup::lookup(vm, k, sel) else {
        return NOT_HANDLED;
    };
    let result = crate::interpreter::run_method_reentrant(vm, m, cls, &[]);
    match SmallInt::try_from(result) {
        Some(n) => n.value() as u64,
        None => NOT_HANDLED,
    }
}

/// RAII, and the reason is the whole of D4's third row: P2's recovery
/// `longjmp`s and skips ordinary returns, so a hand-written decrement after the
/// VM entry would leak the guard on every recovered fault and disable the door
/// forever after — with nothing in any log. `Drop` runs when this frame's scope
/// ends, and the recovery lands *inside* that scope (`dispatch_callback`'s
/// `sigsetjmp` is deeper on the stack), so every path — normal return, guest
/// `error:`, native fault, and a caught panic — releases exactly once.
struct DepthGuard;

impl DepthGuard {
    fn enter() -> DepthGuard {
        DEPTH.with(|d| d.set(d.get() + 1));
        DepthGuard
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

// ── counters, and D5's two numbers ───────────────────────────────────────────
//
// PER-THREAD, not process-wide, and for two independent reasons. The honest
// one: a wndproc runs on the thread that owns its window, so "how many messages
// crossed" is a per-window-thread fact and a process-wide sum of two windows'
// traffic would answer a question nobody asked. The practical one: `cargo test`
// runs several VMs in one process on parallel threads (WG0's Δ 11), and the
// probe assertions below ("an off-list message must not reach the VM") are
// before/after comparisons that a process-wide counter would let another test
// move underneath — a flake by construction. Everything that READS these (the
// control channel's drain) runs on the pump thread, which is the door's thread,
// so nothing is lost. A thread-local `Cell` increment is also cheaper than an
// atomic on a path that runs per message.

/// One thread's door tallies, plus D5's two tick accumulators.
#[derive(Clone, Copy, Default)]
struct Counters {
    /// Messages that actually reached the VM.
    vm_entries: u64,
    /// Declined: the door was already open on this thread (D3).
    nested: u64,
    /// Declined: a host `eval`/`exec` was already running — see [`vm_busy`].
    busy: u64,
    /// Never offered: off the allowlist, or the door switched off.
    not_listed: u64,
    /// Declined: no UI VM published on this thread.
    no_vm: u64,
    /// VM entries whose answer was not an LRESULT — `#defwindowproc`, a raise,
    /// a recovered fault, or anything else. Every one answered `DefWindowProcW`.
    defaulted: u64,
    /// Panics caught at the FFI boundary. Must stay 0; a nonzero value is a bug
    /// report, not a statistic.
    panics: u64,
    /// D5: QPC ticks spent inside the VM entry, and how many `WM_SIZE`s that
    /// was. Only `WM_SIZE` is sampled — D5 names it, and averaging a `WM_CLOSE`
    /// in with a resize would describe neither.
    door_ticks: u64,
    door_samples: u64,
    /// D5's baseline: QPC ticks spent inside `DefWindowProcW` for the same
    /// message with the door switched off.
    base_ticks: u64,
    base_samples: u64,
    // ── WG3's drain (D1/D2) ─────────────────────────────────────────────
    /// Messages that asked for a pass. The DENOMINATOR of the coalescing
    /// ratio, and deliberately counted here rather than in Smalltalk so the
    /// two sides can be compared (WG2 Δ13's lesson pointed the other way for
    /// tallies the GUEST owns; this one is the host's own mechanism).
    drain_requests: u64,
    /// Wakes actually posted. `drain_requests - drain_posts` is what the latch
    /// saved.
    drain_posts: u64,
    /// Passes that entered the VM. The NUMERATOR. `drain_passes < sizeCount`
    /// is `tests_wg3.md` item 3, and a ratio of 1.0 would mean this whole
    /// mechanism is doing nothing.
    drain_passes: u64,
    /// Wakes/heartbeats declined because a modal loop was pumping (D2).
    drain_suppressed: u64,
    /// Heartbeats with nothing to do. Must not cost a VM entry.
    drain_idle: u64,
    /// Declined because the VM was already inside an entry. Retried by the
    /// heartbeat; a number that grows without bound is a bug report.
    drain_declined: u64,
    /// Passes that answered "there is more" and were re-armed for the
    /// heartbeat rather than re-posted.
    drain_reasked: u64,
}

thread_local! {
    static COUNTERS: Cell<Counters> = const { Cell::new(Counters {
        vm_entries: 0, nested: 0, busy: 0, not_listed: 0, no_vm: 0,
        defaulted: 0, panics: 0,
        door_ticks: 0, door_samples: 0, base_ticks: 0, base_samples: 0,
        drain_requests: 0, drain_posts: 0, drain_passes: 0,
        drain_suppressed: 0, drain_idle: 0, drain_declined: 0, drain_reasked: 0,
    }) };
}

#[inline]
fn bump(f: impl FnOnce(&mut Counters)) {
    COUNTERS.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}

fn counters() -> Counters {
    COUNTERS.with(Cell::get)
}

pub fn vm_entries() -> u64 {
    counters().vm_entries
}
pub fn nested_declined() -> u64 {
    counters().nested
}
pub fn busy_declined() -> u64 {
    counters().busy
}
pub fn not_listed() -> u64 {
    counters().not_listed
}
pub fn no_vm() -> u64 {
    counters().no_vm
}
pub fn defaulted() -> u64 {
    counters().defaulted
}
pub fn panics_caught() -> u64 {
    counters().panics
}
pub fn drain_requests() -> u64 {
    counters().drain_requests
}
pub fn drain_posts() -> u64 {
    counters().drain_posts
}
pub fn drain_passes() -> u64 {
    counters().drain_passes
}
pub fn drain_suppressed() -> u64 {
    counters().drain_suppressed
}
pub fn drain_declined() -> u64 {
    counters().drain_declined
}

/// One line the gate greps for the whole drain story — requests against
/// passes is the coalescing ratio, and `suppressed`/`passes` during a drag is
/// D2's.
pub fn drain_line() -> String {
    let c = counters();
    format!(
        "drain tracking={} requested={} requests={} posts={} passes={} suppressed={} \
         idle={} declined={} reasked={}",
        tracking(),
        drain_requested(),
        c.drain_requests,
        c.drain_posts,
        c.drain_passes,
        c.drain_suppressed,
        c.drain_idle,
        c.drain_declined,
        c.drain_reasked,
    )
}

/// `(door ticks, door samples, baseline ticks, baseline samples, qpc hz)`.
pub fn latency_raw() -> (u64, u64, u64, u64, i64) {
    let c = counters();
    (
        c.door_ticks,
        c.door_samples,
        c.base_ticks,
        c.base_samples,
        qpc_frequency(),
    )
}

/// Mean nanoseconds per sample for each path, `-1` where nothing was sampled.
pub fn latency_ns() -> (i64, i64) {
    let (dt, ds, bt, bs, hz) = latency_raw();
    let ns = |ticks: u64, n: u64| -> i64 {
        if n == 0 || hz == 0 {
            -1
        } else {
            ((ticks as i128 * 1_000_000_000i128) / (hz as i128 * n as i128)) as i64
        }
    };
    (ns(dt, ds), ns(bt, bs))
}

/// Zero this thread's tallies — the warm/cold line for D5's measurement, so the
/// numbers reported describe steady state and not the first-ever entry's
/// tier-1 compilation.
pub fn reset_stats() {
    COUNTERS.with(|c| c.set(Counters::default()));
}

/// One line a gate script can `grep`, and a human can read.
pub fn stats_line() -> String {
    let (door_ns, base_ns) = latency_ns();
    format!(
        "door enabled={} depth={} entries={} defaulted={} nested={} busy={} notlisted={} novm={} \
         panics={} doorNs={} baseNs={} doorSamples={} baseSamples={}",
        door_enabled(),
        depth(),
        vm_entries(),
        defaulted(),
        nested_declined(),
        busy_declined(),
        not_listed(),
        no_vm(),
        panics_caught(),
        door_ns,
        base_ns,
        counters().door_samples,
        counters().base_samples,
    )
}

// ── D2: the address channel ──────────────────────────────────────────────────

/// The trampoline's address, as the Integer a Smalltalk-built `WNDCLASSW` puts
/// in `lpfnWndProc`. This is WG0 Δ8's missing channel, and it is deliberately
/// **policy-free** in exactly the way the winkb primitives are: it answers an
/// address; it knows nothing about windows, classes or messages. The guest
/// decides what to do with it.
///
/// The alternative — having Rust register the window class — was rejected in
/// the sprint spec: it would move window creation out of Smalltalk and break
/// §2.2 commitment 4 and WG1's D1 ownership line for the sake of one integer.
pub fn door_address() -> i64 {
    macvm_wndproc as *const () as usize as i64
}

// ── the trampoline ───────────────────────────────────────────────────────────

/// The `lpfnWndProc` Win32 calls. Never panics across this boundary; never
/// enters the VM re-entrantly; never leaves the depth counter wrong.
///
/// The order of the four early exits is itself a performance statement, and it
/// is WG1 Δ3's rule ("the pump must not ask the VM anything per message")
/// applied to the door: the allowlist test is six integer compares and comes
/// FIRST, so a `WM_MOUSEMOVE` storm costs a scan and a call and touches nothing
/// else — no thread-local read, no atomic, no VM.
pub extern "system" fn macvm_wndproc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    // 0. WG3 D2. The tracking transitions are pure Rust and run BEFORE any
    //    guard, because they must hold even when the VM is busy and the entry
    //    below will be declined: the modal loop is starting whether or not
    //    Smalltalk gets to hear about it, and a drain that ran inside it would
    //    be the bug. `WM_EXITSIZEMOVE` re-posts, because `request_drain`
    //    deliberately did not while tracking.
    match msg {
        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => set_tracking(true),
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            set_tracking(false);
            if drain_requested() {
                request_drain(hwnd);
            }
        }
        _ => {}
    }
    // 0b. WG3 D1. The wake and the heartbeat, handled by the trampoline itself
    //     rather than routed to `WinShell` — they are the drain's plumbing, not
    //     messages whose meaning is Smalltalk's, and they must work identically
    //     whether our own pump dispatched them or a modal loop did.
    //
    //     Inside `catch_unwind` for the same reason everything else is: a Rust
    //     panic unwinding into Win32's dispatcher is UB.
    if msg == WM_APP_DRAIN {
        let _ = catch_unwind(AssertUnwindSafe(|| service_drain(hwnd)));
        return 0;
    }
    if msg == WM_TIMER {
        let _ = catch_unwind(AssertUnwindSafe(|| service_drain(hwnd)));
        // ...and then the system default anyway. WG3 owns exactly one timer and
        // treats every `WM_TIMER` as its heartbeat, which is affordable only
        // because a heartbeat with nothing to do costs no VM entry (see
        // `service_drain`). Falling through to `DefWindowProcW` keeps any OTHER
        // timer — a `TIMERPROC` passed in `lParam`, which WG4+ may want — working
        // exactly as it would have.
        return default_proc(msg, hwnd, wparam, lparam);
    }
    // 1. Off the list (or the door is switched off): the overwhelmingly common
    //    case, and the cheapest one. `DefWindowProcW` and nothing else.
    if !is_allowlisted(msg) {
        bump(|c| c.not_listed += 1);
        return default_proc(msg, hwnd, wparam, lparam);
    }
    // 2. A panic must not unwind into Win32's dispatcher (UB). Everything from
    //    here down is inside the net; a caught panic answers the default, like
    //    every other failure this door knows how to have.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        cross_into_smalltalk(hwnd, msg, wparam, lparam)
    }));
    match outcome {
        Ok(Some(lresult)) => lresult,
        Ok(None) => {
            bump(|c| c.defaulted += 1);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        Err(_) => {
            bump(|c| {
                c.panics += 1;
                c.defaulted += 1;
            });
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// `DefWindowProcW`, timed when the message is the one D5 samples and the door
/// is OFF — i.e. exactly the baseline the spec asks for ("the same message
/// `DefWindowProcW`'d, allowlist off").
#[inline]
fn default_proc(msg: u32, hwnd: isize, wparam: usize, lparam: isize) -> isize {
    if msg == WM_SIZE {
        let t0 = qpc();
        let r = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        let dt = (qpc() - t0).max(0) as u64;
        bump(|c| {
            c.base_ticks += dt;
            c.base_samples += 1;
        });
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// `Some(lresult)` when Smalltalk answered an LRESULT; `None` for every "let
/// `DefWindowProcW` handle it" — the Symbol `#defwindowproc`, a nested entry, a
/// missing VM, a raise, a recovered fault, or an out-of-range argument.
fn cross_into_smalltalk(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    // D3. Read the depth BEFORE anything else touches the VM, and answer the
    // default if the door is already open on this thread. Nesting is therefore
    // always safe and always degrades to default behaviour rather than to
    // corruption. NOTE the guard's scope: it ends when THIS function returns,
    // which is before `macvm_wndproc` calls `DefWindowProcW` — see the module
    // doc for why `WM_DESTROY` depends on that.
    if depth() > 0 {
        bump(|c| c.nested += 1);
        return None;
    }
    // The second re-entrancy source, and the one the spec does not name: a
    // Win32 call made from inside `eval`/`exec` that SENDS a message
    // (`CreateWindowExW`, `SetWindowPos`, `DestroyWindow` all do). `depth` is
    // legitimately 0 there and the entry would still be nested. See `vm_busy`.
    if vm_busy() {
        bump(|c| c.busy += 1);
        return None;
    }
    let vmp = crate::embed::ui_vm();
    if vmp.is_null() {
        // No UI VM published on this thread. Ordinary before `openMain` and in
        // any process that is not the UI host; not an error, just the default.
        bump(|c| c.no_vm += 1);
        return None;
    }
    // Win32's four words as Smalltalk Integers. `wParam` is a `UINT_PTR` and
    // `lParam` an `LONG_PTR`, i.e. unsigned and signed; SmallInt is 61-bit, so
    // a `wParam` with the top bits set does not fit and CANNOT be passed —
    // `SmallInt::new` would PANIC (it is documented as a VM-bug assertion, not
    // a guest error). `try_new` and a default answer instead: refusing a message
    // we cannot represent is right, and panicking inside a wndproc is not.
    let (Some(a_hwnd), Some(a_msg), Some(a_w), Some(a_l)) = (
        SmallInt::try_new(hwnd as i64),
        SmallInt::try_new(msg as i64),
        SmallInt::try_new(wparam as i64),
        SmallInt::try_new(lparam as i64),
    ) else {
        return None;
    };

    let _guard = DepthGuard::enter();
    bump(|c| c.vm_entries += 1);

    // Test-only, and placed HERE rather than at the top on purpose: the one
    // failure the trampoline cannot be allowed to have is a panic, and the
    // interesting question is whether the guard is still released when one
    // happens WHILE THE GUARD IS HELD. A panic before the guard would prove
    // nothing. "We never panic" is not a property anyone can check by reading.
    #[cfg(test)]
    if PANIC_NEXT.with(|c| c.replace(false)) {
        panic!("deliberate panic inside the door (see never_panics_across_the_ffi_boundary)");
    }

    // SAFETY: the UI VM's `VmHandle` is boxed and published by the host on THIS
    // thread and outlives the pump (`win_gui`'s `run`), and the depth guard
    // above establishes that no other `&mut VmState` is live — a wndproc call
    // arrives with the VM quiescent between messages, which is precisely what
    // makes this a top-level entry rather than a re-entrant borrow.
    let handle: &mut crate::embed::VmHandle = unsafe { &mut *vmp };

    // `dispatch_callback` is the SAME per-entry `sigsetjmp` recovery door
    // `eval` uses (and the one the Cocoa delegates use). A handler that raises
    // or faults unwinds to inside it, it rewinds the VM to its clean idle
    // baseline, and it answers the `default` we hand it. `NOT_HANDLED` is a
    // value no LRESULT can be, so "the handler blew up" and "the handler said
    // #defwindowproc" arrive here as the same, safe answer.
    let sample = msg == WM_SIZE;
    let t0 = if sample { qpc() } else { 0 };
    let raw = handle.dispatch_callback(NOT_HANDLED, |vm| {
        if msg == WM_DRAWITEM {
            // WG4 D3. The SAME payload-lifetime constraint as WM_NOTIFY —
            // `lParam` is a `DRAWITEMSTRUCT*` alive only for this call — but
            // with one difference stated in the sprint design: this handler
            // DRAWS before it returns, because a paint cannot be deferred to
            // the drain without leaving the control blank. It draws from
            // copied-out values into a DC Windows just handed us; it stashes
            // nothing and calls nothing that sends a message back.
            return perform_drawitem(vm, lparam);
        }
        if msg == WM_NOTIFY {
            // D3's payload-lifetime constraint. `lParam` is an `NMHDR*` that is
            // valid ONLY for the duration of this call, so the three fields are
            // read HERE — inside the recovery window, so a bad pointer is a
            // recovered fault rather than a process death — and handed to
            // Smalltalk as values. Nothing stashes the pointer, because a
            // stashed one is invisible until it corrupts something.
            perform_notify(vm, a_hwnd, lparam)
        } else {
            perform_window_message(vm, a_hwnd, a_msg, a_w, a_l)
        }
    });
    if sample {
        let dt = (qpc() - t0).max(0) as u64;
        bump(|c| {
            c.door_ticks += dt;
            c.door_samples += 1;
        });
    }
    // WG3 D1: the door's OTHER half. The message has been recorded (whatever
    // the handler answered, including a raise — the flag is Smalltalk's and it
    // was set before any work could fail), so note that deferred work exists
    // and wake the loop. The real work happens in `service_drain`, on a fresh
    // top-level entry, with this frame long gone.
    if DRAIN_REQUESTING.contains(&msg) {
        request_drain(hwnd);
    }
    if raw == NOT_HANDLED {
        return None;
    }
    if msg == WM_DESTROY {
        GUEST_HANDLED_DESTROY.with(|c| c.set(true));
    }
    Some(raw as i64 as isize)
}

/// The three `NMHDR` fields, read out of a pointer that dies when this call
/// returns: `hwndFrom` (offset 0), `idFrom` (8), `code` (16) on 64-bit Windows.
///
/// Transcribed offsets, and this is the same exception D2 makes for the
/// allowlist and for the same reason — a Rust read cannot query SQLite, and the
/// core crate must build on a machine with no `windows_api.db`. The
/// compensating control is data: `testNotifyOffsetsMatchWinkb`
/// (`63_winui_controls_tests.mst`) asks winkb for all three by name and asserts
/// these literals, so a wrong one fails in-language rather than silently
/// reading the wrong word.
///
/// # Safety
/// `p` must be the `NMHDR*` Windows passed in `WM_NOTIFY`'s `lParam`, read
/// during the call. Called only from inside `dispatch_callback`, so a bad
/// pointer is a recovered fault rather than a dead process.
unsafe fn read_nmhdr(p: isize) -> (isize, usize, u32) {
    let base = p as *const u8;
    let hwnd_from = std::ptr::read_unaligned(base as *const isize);
    let id_from = std::ptr::read_unaligned(base.add(8) as *const usize);
    let code = std::ptr::read_unaligned(base.add(16) as *const u32);
    (hwnd_from, id_from, code)
}

/// The four `DRAWITEMSTRUCT` fields WG4 D3 needs, read out of a pointer that
/// dies when this call returns: `CtlID` (offset 4), `itemState` (16), `hDC`
/// (32) and `rcItem` (40, four `LONG`s).
///
/// Transcribed offsets, same exception and same compensating control as
/// [`read_nmhdr`]: `testDrawItemOffsetsMatchWinkb` asks winkb for each by name
/// and asserts these literals, so a wrong one fails in-language rather than
/// painting into the wrong place.
///
/// # Safety
/// `p` must be the `DRAWITEMSTRUCT*` Windows passed in `WM_DRAWITEM`'s
/// `lParam`, read during the call. Called only from inside
/// `dispatch_callback`, so a bad pointer is a recovered fault.
unsafe fn read_drawitem(p: isize) -> (u32, u32, isize, [i32; 4]) {
    let base = p as *const u8;
    let ctl_id = std::ptr::read_unaligned(base.add(4) as *const u32);
    let item_state = std::ptr::read_unaligned(base.add(16) as *const u32);
    let hdc = std::ptr::read_unaligned(base.add(32) as *const isize);
    let left = std::ptr::read_unaligned(base.add(40) as *const i32);
    let top = std::ptr::read_unaligned(base.add(44) as *const i32);
    let right = std::ptr::read_unaligned(base.add(48) as *const i32);
    let bottom = std::ptr::read_unaligned(base.add(52) as *const i32);
    (ctl_id, item_state, hdc, [left, top, right, bottom])
}

/// `WM_DRAWITEM` -> `WinShell class>>drawItem:state:dc:x:y:w:h:`, every argument
/// a `SmallInteger` and no pointer among them — D3's constraint, unchanged by
/// the fact that this one draws inline.
fn perform_drawitem(vm: &mut VmState, lparam: isize) -> u64 {
    let (ctl_id, item_state, hdc, r) = unsafe { read_drawitem(lparam) };
    let (Some(a_id), Some(a_state), Some(a_dc)) = (
        SmallInt::try_new(ctl_id as i64),
        SmallInt::try_new(item_state as i64),
        SmallInt::try_new(hdc as i64),
    ) else {
        return NOT_HANDLED;
    };
    let (Some(a_x), Some(a_y), Some(a_w), Some(a_h)) = (
        SmallInt::try_new(r[0] as i64),
        SmallInt::try_new(r[1] as i64),
        SmallInt::try_new((r[2] - r[0]) as i64),
        SmallInt::try_new((r[3] - r[1]) as i64),
    ) else {
        return NOT_HANDLED;
    };
    let Some(cls) = win_shell_class(vm) else {
        return NOT_HANDLED;
    };
    let sel = vm.universe.intern(b"drawItem:state:dc:x:y:w:h:");
    let k = crate::runtime::primitives::klass_of(vm, cls);
    let Some(m) = crate::runtime::lookup::lookup(vm, k, sel) else {
        return NOT_HANDLED;
    };
    let argv = [
        a_id.oop(),
        a_state.oop(),
        a_dc.oop(),
        a_x.oop(),
        a_y.oop(),
        a_w.oop(),
        a_h.oop(),
    ];
    let result = crate::interpreter::run_method_reentrant(vm, m, cls, &argv);
    match SmallInt::try_from(result) {
        Some(n) => n.value() as u64,
        None => NOT_HANDLED,
    }
}

/// The three `NMHDR` fields as `SmallInteger`s, or `None` if any of them will
/// not fit one.
///
/// Split out of [`perform_notify`] precisely so it can be tested without a VM:
/// Δ9 says a control id is a `u32` (and `idFrom` is in fact a `UINT_PTR`) that
/// must be range-checked before `SmallInt::new` sees it, because that function
/// PANICS out of range and a panic inside a wndproc is undefined behaviour.
/// `control_id_range_checked` drives a fabricated `NMHDR` through this.
fn notify_smis(lparam: isize) -> Option<[SmallInt; 3]> {
    if lparam == 0 {
        return None;
    }
    let (hwnd_from, id_from, code) = unsafe { read_nmhdr(lparam) };
    Some([
        // `hwndFrom` is an HWND — a pointer, reinterpreted signed exactly as
        // the four words of `window:message:wParam:lParam:` are.
        SmallInt::try_new(hwnd_from as i64)?,
        // `idFrom` is a `UINT_PTR` and therefore UNSIGNED: `as i64` would turn
        // an out-of-range id into a plausible negative one instead of refusing
        // it, which is the exact shape of bug Δ9 is about. Two conversions, and
        // both of them can say no.
        SmallInt::try_new(i64::try_from(id_from as u64).ok()?)?,
        // `code` is a `UINT` whose conventional values are negative constants
        // cast unsigned (`LVN_ITEMCHANGED` is 4 294 967 195), which is also how
        // winkb reports them — so it crosses unsigned and compares equal in
        // Smalltalk against `WinApi constant:`.
        SmallInt::try_new(code as i64)?,
    ])
}

/// `WM_NOTIFY`, decoded: `WinShell class>>window:notifyFrom:id:code:`.
///
/// Four `SmallInteger`s and no pointer — the whole of D3's constraint in one
/// signature. A control id is a `u32` and a notification code is a `UINT` whose
/// conventional values are negative constants cast unsigned (`LVN_ITEMCHANGED`
/// is 4 294 967 195), so all of them go through `try_new` (Δ9: `SmallInt::new`
/// PANICS out of range, and a wndproc must never panic).
fn perform_notify(vm: &mut VmState, hwnd: SmallInt, lparam: isize) -> u64 {
    let Some([a_from, a_id, a_code]) = notify_smis(lparam) else {
        return NOT_HANDLED;
    };
    let Some(cls) = win_shell_class(vm) else {
        return NOT_HANDLED;
    };
    let sel = vm.universe.intern(b"window:notifyFrom:id:code:");
    let k = crate::runtime::primitives::klass_of(vm, cls);
    let Some(m) = crate::runtime::lookup::lookup(vm, k, sel) else {
        return NOT_HANDLED;
    };
    let argv = [hwnd.oop(), a_from.oop(), a_id.oop(), a_code.oop()];
    let result = crate::interpreter::run_method_reentrant(vm, m, cls, &argv);
    match SmallInt::try_from(result) {
        Some(n) => n.value() as u64,
        None => NOT_HANDLED,
    }
}

/// The "not an LRESULT" sentinel handed to `dispatch_callback` as its default.
///
/// It has to be a `u64` that no real answer can collide with, because
/// `dispatch_callback`'s contract is a single `u64` return with no out-of-band
/// channel. Every LRESULT this door can produce comes from a `SmallInteger`,
/// whose 61-bit range cannot reach bit 63 — so a value with the top bit set and
/// the rest a recognisable pattern is unreachable from Smalltalk by
/// construction rather than by convention.
const NOT_HANDLED: u64 = 0x8000_0000_DEFB_DEFB;

/// Inside the recovery door, with a live `&mut VmState`: send
/// `WinShell class>>window:message:wParam:lParam:` and classify the answer.
///
/// A `SmallInteger` answer is the LRESULT. Anything else — `#defwindowproc`,
/// `nil`, a `LargePositiveInteger`, a class that does not implement the
/// selector, `WinShell` not declared because the layer is not loaded — is
/// [`NOT_HANDLED`], i.e. `DefWindowProcW`. Failing to the default on every
/// unrecognised answer is what makes the two-sided allowlist safe: the halves
/// may disagree, but only ever in the direction of doing less.
fn perform_window_message(
    vm: &mut VmState,
    hwnd: SmallInt,
    msg: SmallInt,
    wparam: SmallInt,
    lparam: SmallInt,
) -> u64 {
    let Some(cls) = win_shell_class(vm) else {
        return NOT_HANDLED;
    };
    let sel = vm.universe.intern(b"window:message:wParam:lParam:");
    let k = crate::runtime::primitives::klass_of(vm, cls);
    let Some(m) = crate::runtime::lookup::lookup(vm, k, sel) else {
        return NOT_HANDLED;
    };
    // Nothing allocates between here and the re-entrant run's own push, so no
    // HandleScope is needed: all four arguments are immediates and `cls` is a
    // class object, which the globals array already roots.
    let argv = [hwnd.oop(), msg.oop(), wparam.oop(), lparam.oop()];
    let result = crate::interpreter::run_method_reentrant(vm, m, cls, &argv);
    match SmallInt::try_from(result) {
        Some(n) => n.value() as u64,
        None => NOT_HANDLED,
    }
}

/// The world-side `WinShell` class oop, resolved by name from the globals.
/// `None` when `winui.list` was never loaded — which is the ordinary state of
/// every non-UI process in this tree, and must fail closed rather than loudly.
fn win_shell_class(vm: &mut VmState) -> Option<Oop> {
    let name = vm.universe.intern(b"WinShell");
    let assoc = crate::runtime::globals::global_lookup(vm, name)?;
    let value = MemOop::try_from(assoc)?.body_oop(1);
    KlassOop::try_from(value).map(|_| value)
}

/// Test-only: force the depth counter, so the nested-entry arm and the
/// leak-on-early-return arm are reachable without a real window, a real message
/// loop and a real modal drag.
#[cfg(test)]
fn force_depth(n: u32) {
    DEPTH.with(|d| d.set(n));
}

#[cfg(test)]
thread_local! {
    /// Arms one deliberate panic inside the door — see
    /// `never_panics_across_the_ffi_boundary`.
    static PANIC_NEXT: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D2, and `tests_wg2.md`'s `door_address_is_stable_and_nonzero`. A function
    /// address is not a thing that changes; asking twice is how you find out
    /// that someone made it answer a fresh trampoline per call.
    #[test]
    fn door_address_is_stable_and_nonzero() {
        let a = door_address();
        let b = door_address();
        assert_ne!(a, 0, "lpfnWndProc must not be NULL");
        assert_eq!(a, b, "the door's address must be the same one every time");
        // And it must be representable as the SmallInteger the guest writes
        // into WNDCLASSW — an address the guest cannot hold is not a channel.
        assert!(
            SmallInt::try_new(a).is_some(),
            "the door address must fit a SmallInteger"
        );
    }

    /// D3. A nested entry takes the `DefWindowProcW` path without touching the
    /// VM — proven by the probe counter: `vm_entries` must not move.
    #[test]
    fn depth_guard_blocks_reentry() {
        let before = vm_entries();
        force_depth(1);
        let answer = cross_into_smalltalk(0, WM_SIZE, 0, 0);
        force_depth(0);
        assert!(answer.is_none(), "a nested entry must answer the default");
        assert_eq!(
            vm_entries(),
            before,
            "a nested entry must not reach the VM at all"
        );
    }

    /// The leak this sprint most fears, in its cheapest form: the counter is
    /// back to 0 after an entry that returned EARLY (no VM published, so
    /// `cross_into_smalltalk` bails after taking no guard) and after one that
    /// took the guard and then bailed.
    ///
    /// The fault-path version — the one that actually matters — is
    /// `depth_returns_to_zero_after_a_recovered_guest_fault` below, which
    /// drives a real `longjmp` through a real VM.
    #[test]
    fn depth_guard_drops_on_early_return() {
        assert_eq!(depth(), 0);
        // No VM published on this thread: the early-return arm.
        let answer = cross_into_smalltalk(0, WM_CLOSE, 0, 0);
        assert!(answer.is_none());
        assert_eq!(depth(), 0, "the early return must leave the counter honest");
        // And through the trampoline itself, which is where a leak would show.
        let _ = macvm_wndproc(0, WM_CLOSE, 0, 0);
        assert_eq!(depth(), 0);
        let _ = macvm_wndproc(0, WM_MOUSEMOVE_FOR_TEST, 0, 0);
        assert_eq!(depth(), 0);
    }

    /// A message Win32 sends in storms and the door must never forward.
    const WM_MOUSEMOVE_FOR_TEST: u32 = 0x0200;

    /// D1. The allowlist is a CLOSED set: a message not on it never reaches the
    /// VM entry point. Probed with the counter rather than argued.
    #[test]
    fn allowlist_is_a_closed_set() {
        set_door_enabled(true);
        for m in ALLOWLIST {
            assert!(is_allowlisted(m), "{m:#06x} is on the list");
        }
        // The storm messages that STILL do not cross, by name and by number,
        // plus WM_PAINT which is deliberately not routed.
        //
        // `WM_MOUSEMOVE` (0x0200) is no longer among them: WG4 D5 admitted it
        // for the splitter drag, with the three-part argument in this module's
        // own header (allowlist scan first, door records only, `WinShell`
        // declines when no drag is active). It is asserted as PRESENT above,
        // via the ALLOWLIST loop -- so this test still pins the set exactly,
        // it just pins the set WG4 actually ships.
        for m in [
            0x0084u32, // WM_NCHITTEST
            0x0020,    // WM_SETCURSOR
            0x000F,    // WM_PAINT
            0x0014,    // WM_ERASEBKGND
            0x0001,    // WM_CREATE
        ] {
            assert!(!is_allowlisted(m), "{m:#06x} must not cross");
        }
        let before = vm_entries();
        for m in [0x0084u32, 0x0020, 0x000F] {
            let _ = macvm_wndproc(0, m, 0, 0);
        }
        assert_eq!(
            vm_entries(),
            before,
            "an off-list message must not reach the VM"
        );
    }

    /// The master switch makes the allowlist EMPTY, which is implementation
    /// order step 1 ("prove the door is transparent before it does anything")
    /// and D5's baseline in the same lever.
    #[test]
    fn door_off_makes_the_allowlist_empty() {
        set_door_enabled(false);
        for m in ALLOWLIST {
            assert!(!is_allowlisted(m), "door off ⇒ nothing crosses");
        }
        set_door_enabled(true);
        for m in ALLOWLIST {
            assert!(is_allowlisted(m));
        }
    }

    /// The sentinel must be unreachable from Smalltalk. A `SmallInteger`'s
    /// 61-bit value cannot produce it, whatever the guest answers, so a handler
    /// can never accidentally mean "I did not handle this".
    #[test]
    fn not_handled_is_unreachable_from_a_smallinteger() {
        assert!(SmallInt::try_new(NOT_HANDLED as i64).is_none());
        assert!(SmallInt::try_new(SmallInt::MAX).unwrap().value() as u64 != NOT_HANDLED);
    }

    /// The second re-entrancy source, the one the spec does not name: a host
    /// `eval`/`exec` already on this thread. `CreateWindowExW` and
    /// `SetWindowPos` SEND `WM_SIZE`, and `openMain` runs inside `vm.eval`, so
    /// without this the door re-enters the VM on the very first run.
    #[test]
    fn busy_guard_declines_a_message_sent_from_inside_a_doit() {
        assert!(!vm_busy());
        let before = vm_entries();
        {
            let _busy = BusyGuard::enter();
            assert!(vm_busy());
            let answer = cross_into_smalltalk(0, WM_SIZE, 0, 0);
            assert!(
                answer.is_none(),
                "a message sent from inside a doit defaults"
            );
            assert_eq!(depth(), 0, "the busy arm must not take the depth guard");
        }
        assert!(!vm_busy(), "the RAII guard must release on scope exit");
        assert_eq!(vm_entries(), before, "the VM must not have been entered");
        assert!(busy_declined() > 0);
    }

    // ── the test this sprint is actually nervous about ──────────────────────
    //
    // Everything above simulates. This one drives a REAL VM through a REAL
    // guest fault inside a REAL handler, reached through the REAL trampoline —
    // because D4's third row is a claim about `longjmp`, and a claim about
    // `longjmp` cannot be tested by anything that does not `longjmp`.

    /// Boot a VM with the base world and a **stand-in `WinShell`** — no
    /// window, no winkb, no `winui.list`. The door only ever needs a class
    /// named `WinShell` that implements `window:message:wParam:lParam:`, and
    /// that is exactly what makes it testable without a desktop, on a `cargo
    /// test` worker thread, in parallel with everything else.
    ///
    /// The base world IS loaded (not `boot_without_world`) because these tests
    /// are about the RECOVERY path, and `error:`, `isNil` and `+` all live in
    /// the world — a handler that cannot raise on purpose cannot test a
    /// `longjmp`.
    fn boot_with_shell(handler_body: &str) -> Box<crate::embed::VmHandle> {
        let mut vm = Box::new(
            crate::embed::VmHandle::boot(
                crate::runtime::VmOptions {
                    heap_mib: 64,
                    ..Default::default()
                },
                std::path::Path::new("world"),
            )
            .expect("the base world must boot"),
        );
        vm.exec(&format!(
            "Object subclass: WinShell [ \
                 <classVars: Seen> \
                 WinShell class >> seen [ ^Seen isNil ifTrue: [ 0 ] ifFalse: [ Seen ] ] \
                 WinShell class >> window: h message: m wParam: w lParam: l [ \
                     Seen := self seen + 1. \
                     {handler_body} \
                 ] \
             ]"
        ))
        .expect("the stand-in WinShell must compile");
        vm
    }

    /// Run `f` with `vm` published as this thread's UI VM, and unpublish after
    /// — `publish_ui_vm` is thread-local, so a test that left it set would
    /// hand the next test on this worker thread a dangling pointer.
    fn with_published<R>(vm: &mut crate::embed::VmHandle, f: impl FnOnce() -> R) -> R {
        crate::embed::publish_ui_vm(vm as *mut _);
        let r = f();
        crate::embed::publish_ui_vm(std::ptr::null_mut());
        r
    }

    /// **D4 row 3, and `depth_guard_drops_on_early_return`'s real form.** A
    /// handler that raises unwinds by a NON-UNWINDING `longjmp` that skips
    /// ordinary returns; if the depth guard were released by a decrement after
    /// the VM entry it would leak here, and every later message would be
    /// declined as "nested" forever — "Smalltalk stopped receiving messages",
    /// with nothing in any log.
    ///
    /// So: fault, then check the counter, then send ANOTHER message and check
    /// it still reaches the VM. The second half is what a leak would break.
    #[test]
    fn depth_returns_to_zero_after_a_recovered_guest_fault() {
        set_door_enabled(true);
        let mut vm = boot_with_shell("^self error: 'deliberate raise inside the handler'");
        with_published(&mut vm, || {
            let before = vm_entries();
            let r = macvm_wndproc(0, WM_SIZE, 0, 0);
            assert_eq!(
                vm_entries(),
                before + 1,
                "the raising handler must still have been ENTERED"
            );
            assert_eq!(
                depth(),
                0,
                "the RAII guard must survive the longjmp — a leak here disables \
                 the door forever after"
            );
            // The raise answered DefWindowProcW, which for a WM_SIZE on a null
            // HWND is 0 — the point is that it answered at all.
            let _ = r;
            // And the door still works, which is the half a leak would break.
            let before = vm_entries();
            let _ = macvm_wndproc(0, WM_SIZE, 0, 0);
            assert_eq!(
                vm_entries(),
                before + 1,
                "the NEXT message must still dispatch into Smalltalk"
            );
            assert_eq!(depth(), 0);
        });
        // The error was recovered, not fatal: the VM is still usable.
        assert_eq!(vm.eval("3 + 4.").unwrap().trim(), "7");
        assert_eq!(
            vm.eval("WinShell seen.").unwrap().trim(),
            "2",
            "both entries ran the handler body before it raised"
        );
    }

    /// The same property for a genuine ACCESS_VIOLATION rather than a guest
    /// `error:` — a different arm of `dispatch_callback`, a different recovery
    /// path, the same requirement of the guard.
    #[test]
    fn depth_returns_to_zero_after_a_recovered_native_fault() {
        set_door_enabled(true);
        let mut vm = boot_with_shell("^(Alien forAddress: 1 size: 8) signedLongAt: 1");
        with_published(&mut vm, || {
            let _ = macvm_wndproc(0, WM_SIZE, 0, 0);
            assert_eq!(depth(), 0, "a native fault must not leak the depth guard");
            let before = vm_entries();
            let _ = macvm_wndproc(0, WM_SIZE, 0, 0);
            assert_eq!(vm_entries(), before + 1);
            assert_eq!(depth(), 0);
        });
        assert_eq!(vm.eval("3 + 4.").unwrap().trim(), "7");
    }

    /// The happy path, end to end: a handler that answers an Integer gets that
    /// Integer back to Win32 as the LRESULT, and one that answers
    /// `#defwindowproc` (or anything else that is not an Integer) gets the
    /// system default. The two-sided allowlist's safe direction, measured.
    #[test]
    fn an_integer_answer_becomes_the_lresult_and_a_symbol_becomes_the_default() {
        set_door_enabled(true);
        let mut vm = boot_with_shell("^4242");
        with_published(&mut vm, || {
            assert_eq!(macvm_wndproc(0, WM_SIZE, 0, 0), 4242);
        });

        let mut vm = boot_with_shell("^#defwindowproc");
        with_published(&mut vm, || {
            let before = defaulted();
            let _ = macvm_wndproc(0, WM_SIZE, 0, 0);
            assert_eq!(
                defaulted(),
                before + 1,
                "#defwindowproc must take the default path"
            );
        });

        // nil, a String, a class — anything that is not a SmallInteger — is
        // the default too. Failing closed on an unrecognised answer is what
        // lets the two halves of the allowlist disagree safely.
        let mut vm = boot_with_shell("^'not an lresult'");
        with_published(&mut vm, || {
            let before = defaulted();
            let _ = macvm_wndproc(0, WM_SIZE, 0, 0);
            assert_eq!(defaulted(), before + 1);
        });
    }

    /// A negative LRESULT survives the round trip. `WM_NCHITTEST` and several
    /// others answer negative values, and the sentinel dance must not turn one
    /// into a huge positive.
    #[test]
    fn a_negative_lresult_survives_the_round_trip() {
        set_door_enabled(true);
        let mut vm = boot_with_shell("^-3");
        with_published(&mut vm, || {
            assert_eq!(macvm_wndproc(0, WM_SIZE, 0, 0), -3);
        });
    }

    /// **The pitfall, made a test.** A Rust `panic!` unwinding into Win32's
    /// dispatcher is undefined behaviour, and "this code never panics" is not
    /// a property a reader can check. So force one, and assert the trampoline
    /// answers `DefWindowProcW`, keeps the depth counter honest, and records
    /// the catch — all three, because a `catch_unwind` that swallowed the
    /// panic while leaking the guard would look like success.
    #[test]
    fn never_panics_across_the_ffi_boundary() {
        set_door_enabled(true);
        let mut vm = boot_with_shell("^0");
        with_published(&mut vm, || {
            let before = panics_caught();
            let entries = vm_entries();
            PANIC_NEXT.with(|c| c.set(true));
            // Silence libtest's panic hook for this one: the message is
            // expected, and printing its backtrace would make a passing run
            // look like a failing one.
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let answer = macvm_wndproc(0, WM_SIZE, 0, 0);
            std::panic::set_hook(prev);
            assert_eq!(answer, 0, "a caught panic answers the system default");
            assert_eq!(panics_caught(), before + 1, "the catch must be recorded");
            assert_eq!(
                vm_entries(),
                entries + 1,
                "the panic must have happened INSIDE the guarded region"
            );
            assert_eq!(depth(), 0, "a caught panic must not leak the depth guard");
            // And the door still works afterwards, which a leak would break.
            assert_eq!(macvm_wndproc(0, WM_SIZE, 0, 0), 0);
            assert_eq!(depth(), 0);
        });
    }

    // ── WG3: the drain (D1), tracking (D2) and the guard's new home (D5) ────

    /// `tests_wg3.md`: **the wake is `PostMessageW`; a `SendMessageW` would
    /// re-enter synchronously.** Not an opinion that can be checked by reading
    /// — a send and a post have the same call shape — so this reads the source
    /// of the one function that wakes the loop.
    ///
    /// The evidence is stronger than the grep suggests: `request_drain` is the
    /// only place the wake is issued, `service_drain` is the only consumer, and
    /// a `SendMessageW` there would call this very trampoline from inside
    /// itself, which the depth guard would decline — so the drain would simply
    /// never run and every WG3 gate item would fail at once. The grep catches
    /// the change at the moment it is made instead.
    #[test]
    fn drain_wake_is_post_not_send() {
        let src = include_str!("win_wndproc.rs");
        let send = concat!("Send", "MessageW");
        for line in src.lines() {
            let l = line.trim();
            if l.starts_with("//") || l.starts_with("///") || l.starts_with("//!") {
                continue;
            }
            assert!(
                !l.contains(send),
                "the drain's wake must be a POST; a send re-enters this trampoline \
                 synchronously — {l}"
            );
        }
        assert!(
            src.contains(concat!("Post", "MessageW(hwnd, WM_APP_DRAIN")),
            "request_drain must post WM_APP_DRAIN"
        );
        // And the message really is a private application message, not
        // something Windows already means by. (The `>= WM_APP` half is
        // `_WM_APP_DRAIN_IS_PRIVATE` at the top of this file — a compile-time
        // assertion, because a runtime one over two constants is a warning and
        // a build error is a better answer anyway.)
        assert!(!ALLOWLIST.contains(&WM_APP_DRAIN));
        assert!(!ALLOWLIST.contains(&WM_TIMER));
    }

    /// D2, and `tests_wg3.md`'s unit row: **flag set ⇒ drain requests are
    /// recorded but not serviced; cleared ⇒ exactly one pass.**
    ///
    /// Runs with no VM published, so "serviced" is measured by whether the pass
    /// got as far as trying — `drain_suppressed` moves while tracking and
    /// `drain_declined` moves after, and `drain_passes` moves in neither
    /// because there is no VM. The three-way distinction is the point: a
    /// suppressed drain and a declined one are different states and only one of
    /// them means D2 is working.
    #[test]
    fn tracking_flag_suppresses_drain() {
        reset_stats();
        DRAIN_REQUESTED.with(|c| c.set(false));
        DRAIN_POSTED.with(|c| c.set(false));
        assert!(!tracking(), "no modal loop at rest");

        // Tracking on: requests accumulate, nothing is posted, nothing runs.
        let _ = macvm_wndproc(0, WM_ENTERSIZEMOVE, 0, 0);
        assert!(tracking());
        for _ in 0..10 {
            request_drain(0);
        }
        assert_eq!(drain_requests(), 10);
        assert_eq!(
            drain_posts(),
            0,
            "a post while tracking would be dispatched by the modal loop's own pump"
        );
        let before = drain_suppressed();
        // A wake that WAS already in the queue when the drag started still
        // arrives — that is the case D2 exists for.
        let _ = macvm_wndproc(0, WM_APP_DRAIN, 0, 0);
        assert_eq!(drain_suppressed(), before + 1);
        assert_eq!(drain_passes(), 0, "ZERO passes while tracking");
        assert!(
            drain_requested(),
            "the flags accumulate, they are not dropped"
        );

        // Tracking off: the accumulated work gets exactly one attempt.
        let _ = macvm_wndproc(0, WM_EXITSIZEMOVE, 0, 0);
        assert!(!tracking());
        let declined = drain_declined();
        let _ = macvm_wndproc(0, WM_APP_DRAIN, 0, 0);
        assert_eq!(
            drain_declined(),
            declined + 1,
            "one attempt after tracking ends (declined here only because no VM \
             is published on this test thread)"
        );
        assert_eq!(drain_suppressed(), before + 1, "and no further suppression");
        DRAIN_REQUESTED.with(|c| c.set(false));
    }

    /// Nesting: a menu loop can open inside a size loop, and a plain bool would
    /// be cleared by the inner one's exit while the outer was still dragging.
    #[test]
    fn tracking_nests() {
        set_tracking(false); // normalise this worker thread
        assert!(!tracking());
        let _ = macvm_wndproc(0, WM_ENTERSIZEMOVE, 0, 0);
        let _ = macvm_wndproc(0, WM_ENTERMENULOOP, 0, 0);
        assert!(tracking());
        let _ = macvm_wndproc(0, WM_EXITMENULOOP, 0, 0);
        assert!(tracking(), "the outer size loop is still running");
        let _ = macvm_wndproc(0, WM_EXITSIZEMOVE, 0, 0);
        assert!(!tracking());
    }

    /// Δ9, and `tests_wg3.md`'s `control_id_range_checked`: an `NMHDR` field is
    /// a `UINT_PTR`/`UINT` that must be refused before `SmallInt::new` sees it,
    /// because that function PANICS out of range and a panic inside a wndproc is
    /// undefined behaviour.
    ///
    /// Drives a FABRICATED `NMHDR` rather than a mocked one: the three offsets
    /// this code reads (0, 8, 16) are exercised by writing at those offsets, so
    /// a transcription error in `read_nmhdr` shows up here as well as in
    /// `testNotifyOffsetsMatchWinkb`.
    #[test]
    fn control_id_range_checked() {
        #[repr(C)]
        struct FakeNmhdr {
            hwnd_from: isize,
            id_from: usize,
            code: u32,
            _pad: u32,
        }
        let ok = FakeNmhdr {
            hwnd_from: 0x1234,
            id_from: 4242,
            code: 0xFFFF_FF9B, // LVN_ITEMCHANGED, i.e. (UINT)-101
            _pad: 0,
        };
        let got = notify_smis(&ok as *const _ as isize).expect("in-range fields must pass");
        assert_eq!(got[0].value(), 0x1234);
        assert_eq!(got[1].value(), 4242);
        assert_eq!(got[2].value(), 0xFFFF_FF9B);

        // A control id that cannot be a SmallInteger: refused, not panicked.
        let bad = FakeNmhdr {
            hwnd_from: 0x1234,
            id_from: usize::MAX,
            code: 0,
            _pad: 0,
        };
        assert!(
            notify_smis(&bad as *const _ as isize).is_none(),
            "an out-of-range idFrom must be refused before SmallInt::new sees it"
        );
        // And a null NMHDR — which a malformed WM_NOTIFY can carry — is not a
        // dereference.
        assert!(notify_smis(0).is_none());
    }

    /// **D5**: `busy_guard_is_inside_eval`. A direct `VmHandle::eval` sets the
    /// busy flag without the caller doing anything, so the door declines a
    /// message sent from inside it — which is the property WG2 achieved by
    /// asking every host to remember, and WG3 makes structural.
    ///
    /// Measured from INSIDE a live `eval`, because that is where the hazard is.
    /// A `TranscriptSink` is a Rust closure the VM calls while the entry is on
    /// the stack — the same position `CreateWindowExW`'s synchronous `WM_SIZE`
    /// arrives from — so the sink drives a real message through the real
    /// trampoline and records what happened.
    #[test]
    fn busy_guard_is_inside_eval() {
        use std::sync::{Arc, Mutex};

        /// What the door looked like from inside the entry.
        #[derive(Default)]
        struct Seen {
            busy: bool,
            entries_moved: bool,
            busy_declined_moved: bool,
        }
        struct Probe(Arc<Mutex<Seen>>);
        impl crate::embed::TranscriptSink for Probe {
            fn show(&mut self, _s: &str) {
                let entries = vm_entries();
                let declined = busy_declined();
                // A message arriving from inside the entry, exactly as
                // `CreateWindowExW`/`SetWindowPos` deliver one.
                let _ = macvm_wndproc(0, WM_SIZE, 0, 0);
                let mut s = self.0.lock().unwrap();
                s.busy = vm_busy();
                s.entries_moved = vm_entries() != entries;
                s.busy_declined_moved = busy_declined() != declined;
            }
        }

        set_door_enabled(true);
        assert!(!vm_busy(), "no host entry live before the test");
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut vm = boot_with_shell("^7");
        vm.set_transcript(Box::new(Probe(seen.clone())));
        crate::embed::publish_ui_vm(&mut *vm as *mut _);
        // NO `BusyGuard` written by this caller — that is the whole point.
        let answer = vm.eval("Transcript showCr: 'inside'. 3 + 4.");
        crate::embed::publish_ui_vm(std::ptr::null_mut());
        answer.expect("the probe must not raise");
        let s = seen.lock().unwrap();
        assert!(
            s.busy,
            "D5: `eval` itself must raise the busy flag — no host cooperation"
        );
        assert!(
            !s.entries_moved,
            "a message sent from inside `eval` must NOT reach the VM"
        );
        assert!(
            s.busy_declined_moved,
            "...and must have been declined for that reason, not another"
        );
        drop(s);
        assert!(!vm_busy(), "the guard must release when `eval` returns");
    }

    /// A VM with no `WinShell` at all — every process in this tree that is not
    /// the UI host — must default, not fail. The layer is conditional by
    /// design and the door has to be a no-op without it.
    #[test]
    fn a_vm_without_winshell_defaults_rather_than_failing() {
        set_door_enabled(true);
        let mut vm = Box::new(crate::embed::VmHandle::boot_without_world(
            crate::runtime::VmOptions::default(),
        ));
        with_published(&mut vm, || {
            let before = defaulted();
            let _ = macvm_wndproc(0, WM_CLOSE, 0, 0);
            assert_eq!(defaulted(), before + 1);
            assert_eq!(depth(), 0);
        });
    }
}
