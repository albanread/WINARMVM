// ── WG11-W3: the input driver + the frame clock's helpers ───────────────────
//
// win_gui/Cargo.toml, [target.'cfg(windows)'.dependencies.windows] features —
// ONE addition; GetAsyncKeyState is the only call not already reachable:
//
//     "Win32_UI_Input_KeyboardAndMouse",
//
// Everything else is in features this crate already has: Win32_Foundation
// (HWND/POINT/RECT), Win32_Graphics_Gdi (ScreenToClient), and
// Win32_UI_WindowsAndMessaging (GetCursorPos, GetClientRect,
// GetForegroundWindow, GetAncestor, GA_ROOT).

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

// Authored as part of `game.rs`; kept a separate module because the input
// driver and the frame sink share nothing but these three facts.
use crate::game::{frame_rate, is_running, pane_size};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, GA_ROOT,
};

/// The Windows virtual keys in GAMEPANE BIT ORDER — bit 0 `keyLeft` … bit 5
/// `keyB` (`world/43_gamepane.mst`, `GamePane class>>keyLeft`…`keyB` = 0…5,
/// and `GameFrame>>keyHeld:` is `(keys bitAnd: (1 bitShift: keyCode)) ~= 0`,
/// so the bit index IS the key code).
///
/// This is the transcription of the Mac's
/// `const CODES: [u16; 6] = [123, 124, 126, 125, 49, 6]`
/// (`upstream/main:cocoa_gui/src/game.rs`, `game_tick`) and it keeps that
/// array's one trap: UP COMES BEFORE DOWN. The bit order is
/// Left/Right/Up/Down/A/B while the macOS codes for them are 123/124/126/125
/// — a port that "tidied" the list into numeric order would swap up and down
/// in every game at once, and only a human playing one would notice.
/// (`MacGamePane/graphics/src/input.rs` names them: KEY_DOWN = 125,
/// KEY_UP = 126.)
///
/// SPACE is keyA and Z is keyB. Life's `frame keyHeld: GamePane keyA` is the
/// SPACE that pauses it, so bit 4 is the one bit this demo reads.
const GAME_VKS: [i32; 6] = [
    0x25, // VK_LEFT   bit 0  keyLeft   (mac 123)
    0x27, // VK_RIGHT  bit 1  keyRight  (mac 124)
    0x26, // VK_UP     bit 2  keyUp     (mac 126)
    0x28, // VK_DOWN   bit 3  keyDown   (mac 125)
    0x20, // VK_SPACE  bit 4  keyA      (mac 49)
    0x5A, // 'Z'       bit 5  keyB      (mac 6 = kVK_ANSI_Z)
];
const VK_LBUTTON: i32 = 0x01;
const VK_RBUTTON: i32 = 0x02;
const VK_ESCAPE: i32 = 0x1B;

/// The Canvas pane's hwnd, published ONCE by the pump (which already learns it
/// for `upload_and_present`) and cleared by [`stop`]. Kept here because the
/// input read runs on the PRIMARY's thread — where the pump's local variable is
/// not reachable — and because every Win32 call below is thread-agnostic, so
/// there is no reason to bounce the read back to the UI thread.
static PANE_HWND: AtomicI64 = AtomicI64::new(0);

/// Escape's up-edge latch. The Mac reads Escape LEVEL-triggered and protects
/// itself by clearing the whole held-key table when a pane is created
/// (`cocoa_gui/src/game.rs`, "the second demo exits immediately" bug — an
/// Escape whose `keyUp:` had nowhere to go stayed stuck "held" and killed the
/// NEXT demo on its first tick). `GetAsyncKeyState` has no table to clear, so
/// the protection has to be the edge itself: fire once when Escape goes down,
/// and do not re-arm until it is physically seen up again.
static ESC_WAS_DOWN: AtomicBool = AtomicBool::new(false);

/// Tell the driver where the game's pixels are. Called once from the pump,
/// right where `WinShell canvasMode: #game` is sent.
pub fn set_pane_hwnd(hwnd: i64) {
    PANE_HWND.store(hwnd, Ordering::Relaxed);
}

/// Is `vk` down RIGHT NOW? The high bit is the live state; bit 0 ("went down
/// since the last call") is deliberately ignored — this is a LEVEL read, the
/// exact contract of the Mac's `key_held`, and every consumer in the corpus
/// depends on it (Life derives its SPACE edge itself against `spaceWas`, and
/// paints a stroke precisely because the buttons are levels).
#[allow(unsafe_code)]
fn vk_down(vk: i32) -> bool {
    // SAFETY: a pure query on a scalar. No handle, no buffer, no thread
    // affinity — GetAsyncKeyState reads the system's physical key state.
    unsafe { (GetAsyncKeyState(vk) as u16) & 0x8000 != 0 }
}

/// Is the MACVM window the ACTIVE window? The Mac gets this for free: its
/// `HELD_KEYS` table is only written by `keyDown:`/`keyUp:` delivered to the
/// game's own key-capable view, so an unfocused game sees no keys at all.
/// `GetAsyncKeyState` is global, so without this gate a MACVM sitting behind a
/// browser would still eat every arrow key the user typed anywhere.
///
/// `GetForegroundWindow`, not `GetFocus`: focus is a per-thread question and
/// this runs on the primary's thread, where `GetFocus` answers null forever.
#[allow(unsafe_code)]
fn ours_is_foreground(pane: HWND) -> bool {
    // SAFETY: both calls take/return an HWND by value and dereference nothing.
    unsafe {
        let root = GetAncestor(pane, GA_ROOT);
        !root.0.is_null() && GetForegroundWindow().0 == root.0
    }
}

/// This frame's input, in exactly the shape the step message wants:
/// `(mask, mouse_x, mouse_y, buttons)` — the Windows twin of `game_tick` +
/// `read_mouse_into_pane_pixels`.
///
/// * `mask` — bit 0 `keyLeft` … bit 5 `keyB`, level-triggered.
/// * `mouse_x`/`mouse_y` — PANE PIXELS from the top-left, the same space
///   `point:y:color:` and `blit:` use; `-1` for "the pointer is not on the
///   pane", which is the sentinel `GameFrame>>mouseInPane` (`^mouseX >= 0`)
///   tests and `initFrame` starts at.
/// * `buttons` — bit 0 left, bit 1 right, level-triggered.
///
/// A quiet answer (`(0, -1, -1, 0)`) is returned rather than an error whenever
/// there is nothing sensible to report: no pane yet, another app in front, or
/// the pointer outside the pane. A game must never see a stale frame's input.
pub fn read_input() -> (i64, i64, i64, i64) {
    const NOTHING: (i64, i64, i64, i64) = (0, -1, -1, 0);
    let raw = PANE_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return NOTHING;
    }
    let pane = HWND(raw as *mut core::ffi::c_void);
    if !ours_is_foreground(pane) {
        return NOTHING;
    }
    let mut mask = 0i64;
    for (bit, vk) in GAME_VKS.iter().enumerate() {
        if vk_down(*vk) {
            mask |= 1 << bit;
        }
    }
    let (mx, my, buttons) = read_pointer(pane);
    (mask, mx, my, buttons)
}

/// The pointer, converted once — here, the only place that knows both the
/// pane's client rectangle and this session's logical size.
///
/// The arithmetic is the Mac's, with the fraction folded out. The Mac has a
/// normalized `(fx, fy)` from its view and does `((fx * w) as i64).min(w - 1)`;
/// we have client pixels and a client size, so the same value is
/// `(x * w / cw).min(w - 1)` in integers — no float, and the truncating divide
/// IS the Mac's `as i64` floor because both operands are non-negative by the
/// time we divide.
///
/// Four things the Mac does not have to say and we do:
///
/// 1. NO Y FLIP. Client coordinates already count down from the top-left, the
///    same way pane pixels do. The Mac flips because an unflipped `NSView`
///    counts up from the bottom.
/// 2. THE DENOMINATOR IS EXACTLY RIGHT. `winui_render` takes its viewport from
///    `GetClientRect` on this same hwnd and draws the plane as a full-viewport
///    POINT-sampled stretch — `ps_indexed` does `uint2(i.uv * float2(w, h))`
///    over the fullscreen triangle and `ps_plane` uses
///    `D3D11_FILTER_MIN_MAG_MIP_POINT` — so there is no letterbox and no
///    aspect correction, and client rect ↔ pane pixels really is one plain
///    proportion. It stays right when the splitter resizes the pane, and DPI
///    cancels because both terms come from the same coordinate space.
/// 3. OUTSIDE IS `-1`, NOT A CLAMP. The Mac's view simply stops receiving
///    events when the pointer leaves it, so its last position is harmless.
///    `GetCursorPos` keeps answering, so clamping here would let a drag off the
///    pane paint a stroke down the pane's edge forever — Life holds the left
///    button and draws for as long as `mouseInPane` is true. Note the `.min()`
///    below therefore never fires for in-rect input (for `0 <= x <= cw-1`,
///    `floor(x*w/cw) <= w-1` always); it is a belt-and-braces bound, not the
///    rule. The REJECT above it is the rule.
/// 4. THE BUTTONS GO WITH THE POSITION. The Mac stores `left | (right << 1)`
///    unconditionally, independent of where the pointer is. We zero them off
///    the pane instead: `GetAsyncKeyState` is global, so a click aimed at the
///    Workspace would otherwise arrive as a game button, and
///    `GamePane class >> mouseDown` does not itself gate on `mouseInPane`.
///    Life is unaffected either way — `readMouseFrom:` returns early on
///    `frame mouseInPane ifFalse:` before it ever looks at the buttons.
#[allow(unsafe_code)]
fn read_pointer(pane: HWND) -> (i64, i64, i64) {
    let (pane_w, pane_h) = pane_size();
    let mut pt = POINT::default();
    let mut rc = RECT::default();
    // SAFETY: two out-parameters on the stack, both fully initialized, and a
    // window handle used only as a handle. `GetClientRect`/`ScreenToClient` on
    // a window owned by another thread is a supported read.
    unsafe {
        if GetCursorPos(&mut pt).is_err() {
            return (-1, -1, 0);
        }
        if !ScreenToClient(pane, &mut pt).as_bool() {
            return (-1, -1, 0);
        }
        if GetClientRect(pane, &mut rc).is_err() {
            return (-1, -1, 0);
        }
    }
    let (cw, ch) = ((rc.right - rc.left) as i64, (rc.bottom - rc.top) as i64);
    let (x, y) = (pt.x as i64, pt.y as i64);
    if cw <= 0 || ch <= 0 || x < 0 || y < 0 || x >= cw || y >= ch {
        return (-1, -1, 0); // not on the pane: "nowhere", and no buttons
    }
    let px = (x * pane_w as i64 / cw).min(pane_w as i64 - 1);
    let py = (y * pane_h as i64 / ch).min(pane_h as i64 - 1);
    // Bit 0 left, bit 1 right — `GameFrame>>mouseButtons` says "1 left,
    // 2 right" and the Mac packs `left | (right << 1)`.
    //
    // Read with GetAsyncKeyState rather than from messages ON PURPOSE:
    // WM_RBUTTONDOWN/UP are NOT on the door's allowlist (`ALLOWLIST` in
    // src/runtime/win_wndproc.rs carries WM_LBUTTONDOWN/UP and WM_MOUSEMOVE but
    // no right-button message, and no WM_KEYUP either — so a Mac-style
    // keyDown/keyUp held table is unreachable through the door at all).
    // Polling asks the system instead, and the allowlist stays as it is.
    //
    // No SM_SWAPBUTTON correction: VK_LBUTTON is the LOGICAL primary button,
    // just as NSEvent's leftMouseDown is, so a left-handed mouse already agrees
    // with the user on both platforms.
    let buttons = i64::from(vk_down(VK_LBUTTON)) | (i64::from(vk_down(VK_RBUTTON)) << 1);
    (px, py, buttons)
}

/// Did Escape just go down WHILE A GAME IS RUNNING? True exactly once per
/// physical press.
///
/// The `is_running()` gate is not decoration. `PANE_HWND` survives a session,
/// so without it every Escape pressed anywhere in a foreground MACVM — the one
/// that dismisses a shell dialog included — would run `GamePane reset`
/// top-level on the primary for the rest of the process.
///
/// The Mac tests the level every tick and lets `request_stop` be idempotent; we
/// test the EDGE, which is strictly safer here — an Escape still held while the
/// next demo launches cannot close it before its first frame is seen, which is
/// the bug `clear_all()` exists to prevent on the Mac and which we have no
/// table to clear.
pub fn escape_pressed() -> bool {
    if !is_running() {
        ESC_WAS_DOWN.store(false, Ordering::Relaxed);
        return false; // Escape belongs to the shell when no game owns the pane
    }
    let raw = PANE_HWND.load(Ordering::Relaxed);
    let held =
        raw != 0 && ours_is_foreground(HWND(raw as *mut core::ffi::c_void)) && vk_down(VK_ESCAPE);
    if !held {
        ESC_WAS_DOWN.store(false, Ordering::Relaxed);
        return false;
    }
    !ESC_WAS_DOWN.swap(true, Ordering::Relaxed)
}

/// The frame period, WITHOUT the integer-divide truncation `from_millis` has:
/// `1000 / 60` is 16 ms, which is 62.5 fps, and `1000 / 120` is 8 ms = 125 fps.
/// `frame_rate()` already clamps to 1..=120, so this cannot divide by zero.
pub fn frame_period() -> std::time::Duration {
    std::time::Duration::from_nanos(1_000_000_000 / frame_rate())
}

/// The step this frame wants, formatted EXACTLY as the Mac formats it
/// (`upstream/main:cocoa_gui/src/game.rs::poll_primary_step`):
///
/// ```text
/// GamePane stepWithKeys: 16 mouseX: 160 y: 120 buttons: 1
/// ```
///
/// No trailing period — the Mac sends none and this path does not need one
/// (`guarded_eval(vm, "WinShell canvasPaneHwnd")` is already periodless).
///
/// ALWAYS the four-keyword form, never the one-keyword `stepWithKeys: mask`:
/// that arity calls `frame keys: mask`, whose own comment is "Leaves the
/// pointer as it was", so a fallback to it would hand the game the PREVIOUS
/// frame's pointer and buttons. `stepWithKeys:mouseX:y:buttons:` fills the
/// frame BEFORE running the step block, so a game sees this frame's input.
pub fn step_doit() -> String {
    let (mask, mx, my, mb) = read_input();
    format!("GamePane stepWithKeys: {mask} mouseX: {mx} y: {my} buttons: {mb}")
}

#[cfg(test)]
mod input_tests {
    use super::*;
    // The public stop verb, used only by these tests.
    use crate::game::stop;

    #[test]
    fn the_vk_table_is_in_gamepane_bit_order_with_up_before_down() {
        assert_eq!(GAME_VKS[0], 0x25, "bit 0 keyLeft  = VK_LEFT");
        assert_eq!(GAME_VKS[1], 0x27, "bit 1 keyRight = VK_RIGHT");
        assert_eq!(GAME_VKS[2], 0x26, "bit 2 keyUp    = VK_UP");
        assert_eq!(GAME_VKS[3], 0x28, "bit 3 keyDown  = VK_DOWN");
        assert_eq!(GAME_VKS[4], 0x20, "bit 4 keyA     = VK_SPACE");
        assert_eq!(GAME_VKS[5], 0x5A, "bit 5 keyB     = 'Z'");
    }

    #[test]
    fn no_pane_means_a_quiet_frame_not_a_stale_one() {
        set_pane_hwnd(0);
        assert_eq!(read_input(), (0, -1, -1, 0));
        assert_eq!(
            step_doit(),
            "GamePane stepWithKeys: 0 mouseX: -1 y: -1 buttons: 0"
        );
    }

    #[test]
    fn escape_is_inert_while_no_game_is_running() {
        // `stop()` is the public verb that clears RUNNING — the same one the
        // Escape path itself calls, so the test drives the real state.
        stop();
        set_pane_hwnd(1234);
        assert!(!escape_pressed(), "Escape belongs to the shell when idle");
        stop();
    }

    #[test]
    fn frame_period_does_not_truncate() {
        // 60 fps is 16_666_666 ns, not the 16 ms `from_millis(1000/60)` gives.
        assert_eq!(frame_period().as_nanos(), 16_666_666);
    }

    #[test]
    fn stop_clears_running_and_unpublishes_the_pane() {
        // RUNNING is set the way a game sets it: `run` emits StartLoop into
        // the sink. No test-only back door into the static.
        use macvm::embed::{GameCommand, GameSink};
        let mut sink = crate::game::WinGameSink;
        sink.emit(GameCommand::StartLoop);
        assert!(is_running(), "StartLoop must set RUNNING");
        set_pane_hwnd(4321);
        stop();
        assert!(!is_running());
        assert_eq!(read_input(), (0, -1, -1, 0));
    }
}
