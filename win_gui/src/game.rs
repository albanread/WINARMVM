//! WG11-W1 — the GamePane host: a `GameSink` for Windows, and the frame's road
//! to the Canvas.
//!
//! Upstream's games speak `GameCommand` (`src/embed.rs`), a control-plane the
//! Mac consumes in `cocoa_gui/src/game.rs`. WINARM had no consumer at all —
//! `set_game_sink` appeared only inside `#[cfg(test)]`, so `GamePane new … run`
//! emitted into `None` and silently did nothing. This is the consumer.
//!
//! # Two threads, one frame
//!
//! Commands arrive on the PRIMARY's thread (the game's `onStep:` block runs
//! there). Pixels must reach a swapchain that belongs to the UI thread. So the
//! sink owns a CPU-side index buffer, draws into it under a lock, and on
//! `Present` marks a generation; the pump picks the frame up between dispatches
//! and uploads it. That is the same flag-and-drain shape §2.4a uses for
//! everything else crossing this seam, and for the same reason: the drawing
//! side must never touch a COM object owned by another thread.
//!
//! # Reaching the renderer from here
//!
//! `winui_render` is a `cdylib` and nothing links it — deliberately, so its
//! device/swapchain/cell statics exist ONCE, in the copy the guest loaded. The
//! pump reaches that same copy the way any other loaded module is reached:
//! `GetModuleHandleA("winui_render.dll")` answers the handle the guest's FFI
//! already mapped, and `GetProcAddress` gives the identical entry points. Same
//! module, same statics, same thread — the renderer's per-hwnd map is
//! `thread_local` on the UI thread, which is exactly where this runs.
//!
//! # Palette
//!
//! Upstream's model is per-scanline 1..15, global 16..255, index 0
//! transparent. This carries GLOBALS ONLY — which is what `GamePane new`
//! installs (16 colours into slots 16..31) and what Life, FreeCell and
//! Minesweeper use. The per-scanline copper is one demo (`45f_copper.mst`) and
//! arrives with the shader rewrite; until then a copper index renders as a flat
//! colour rather than wrongly, and no game in the target set asks for one.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use macvm::embed::{GameCommand, GameSink};

/// Upstream's default pane. `resizeTo:by:` changes it; every game in the
/// corpus opens at this size.
const DEF_W: u32 = 320;
const DEF_H: u32 = 240;

/// The frame the primary draws into and the pump uploads.
pub struct GameFrame {
    pub w: u32,
    pub h: u32,
    /// One palette index per pixel, row-major, `w * h`.
    pub indices: Vec<u8>,
    /// 256 BGRA words — the renderer's format, so the upload is a memcpy.
    pub palette: Vec<u32>,
    /// Bumped by `Present`; the pump uploads when it moves.
    pub generation: u64,
}

impl GameFrame {
    fn new(w: u32, h: u32) -> Self {
        GameFrame {
            w,
            h,
            indices: vec![0u8; (w * h) as usize],
            // Opaque black, so an index nobody set is visibly empty rather
            // than whatever the allocator left behind.
            palette: vec![0xFF00_0000u32; 256],
            generation: 0,
        }
    }

    fn idx(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return None;
        }
        Some(y as usize * self.w as usize + x as usize)
    }

    fn pset(&mut self, x: i64, y: i64, index: u8) {
        if let Some(i) = self.idx(x, y) {
            self.indices[i] = index;
        }
    }

    fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, index: u8) {
        for row in y..y.saturating_add(h) {
            for col in x..x.saturating_add(w) {
                self.pset(col, row, index);
            }
        }
    }

    /// Bresenham, matching `IndexedPane::line` exactly — a game that draws a
    /// diagonal must get the same pixels here as on the Mac.
    fn line(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, index: u8) {
        let (mut x0, mut y0) = (x0, y0);
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.pset(x0, y0, index);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Midpoint circle drawing horizontal spans — `IndexedPane::disc`.
    fn disc(&mut self, cx: i64, cy: i64, r: i64, index: u8) {
        let mut x = r;
        let mut y = 0;
        let mut err = 0;
        while x >= y {
            self.fill_rect(cx - x, cy + y, 2 * x + 1, 1, index);
            self.fill_rect(cx - x, cy - y, 2 * x + 1, 1, index);
            self.fill_rect(cx - y, cy + x, 2 * y + 1, 1, index);
            self.fill_rect(cx - y, cy - x, 2 * y + 1, 1, index);
            y += 1;
            err += 1 + 2 * y;
            if 2 * (err - x) + 1 > 0 {
                x -= 1;
                err += 1 - 2 * x;
            }
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == self.w && h == self.h {
            return;
        }
        self.w = w;
        self.h = h;
        self.indices = vec![0u8; (w * h) as usize];
    }
}

fn shared() -> &'static Arc<Mutex<GameFrame>> {
    static G: OnceLock<Arc<Mutex<GameFrame>>> = OnceLock::new();
    G.get_or_init(|| Arc::new(Mutex::new(GameFrame::new(DEF_W, DEF_H))))
}

/// The generation the pump last uploaded, and the one the sink last presented.
/// Atomics rather than a flag inside the mutex so the pump's "is there a frame?"
/// check costs a relaxed load and never contends with a drawing primary.
static PRESENTED: AtomicU64 = AtomicU64::new(0);
static UPLOADED: AtomicU64 = AtomicU64::new(0);
/// `GamePane>>run`/`stop`. Read by the pump to decide whether to keep stepping.
static RUNNING: AtomicBool = AtomicBool::new(false);
static FPS: AtomicU64 = AtomicU64::new(60);

pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

pub fn frame_rate() -> u64 {
    FPS.load(Ordering::Relaxed).clamp(1, 120)
}

/// Is there a presented frame the pump has not uploaded yet?
pub fn frame_pending() -> bool {
    PRESENTED.load(Ordering::Acquire) != UPLOADED.load(Ordering::Relaxed)
}

/// The Windows `GameSink`. Every drawing command mutates the CPU buffer; only
/// `Present` costs anything across the seam — one generation bump, which is
/// upstream's whole "a frame emits one command" claim preserved.
pub struct WinGameSink;

impl GameSink for WinGameSink {
    fn emit(&mut self, cmd: GameCommand) {
        let g = shared();
        let mut f = match g.lock() {
            Ok(f) => f,
            // A poisoned lock means a previous holder panicked mid-draw. The
            // frame is garbage but the GAME is not: drop the command rather
            // than propagate a panic into the primary's step block.
            Err(_) => return,
        };
        match cmd {
            GameCommand::PaletteAt { index, r, g: gg, b } => {
                // BGRA, opaque — the renderer's word order, so the upload is a
                // memcpy and nothing converts per frame.
                f.palette[index as usize] =
                    0xFF00_0000 | ((r as u32) << 16) | ((gg as u32) << 8) | b as u32;
            }
            GameCommand::Cls { index } => {
                let n = (f.w * f.h) as usize;
                f.indices[..n].fill(index);
            }
            GameCommand::ClearTo { r, g: gg, b } => {
                // Upstream's convenience: palette 16 takes the colour, then
                // clear to it (`prim_game_clear`).
                f.palette[16] = 0xFF00_0000 | ((r as u32) << 16) | ((gg as u32) << 8) | b as u32;
                let n = (f.w * f.h) as usize;
                f.indices[..n].fill(16);
            }
            GameCommand::Pset { x, y, index } => f.pset(x, y, index),
            GameCommand::Line {
                x0,
                y0,
                x1,
                y1,
                index,
            } => f.line(x0, y0, x1, y1, index),
            GameCommand::FillRect { x, y, w, h, index } => f.fill_rect(x, y, w, h, index),
            GameCommand::Disc { cx, cy, r, index } => f.disc(cx, cy, r, index),
            GameCommand::Blit { data } => {
                // Length-tolerant like `IndexedPane::blit`: a short slice fills
                // the prefix, a long one is truncated, neither panics. A game
                // that resized and blitted an old-sized buffer gets a visibly
                // wrong picture rather than a dead VM.
                let n = data.len().min(f.indices.len());
                f.indices[..n].copy_from_slice(&data[..n]);
            }
            GameCommand::Present => {
                PRESENTED.fetch_add(1, Ordering::Release);
                f.generation += 1;
            }
            GameCommand::StartLoop => RUNNING.store(true, Ordering::Relaxed),
            GameCommand::StopLoop => RUNNING.store(false, Ordering::Relaxed),
            GameCommand::SetFrameRate { fps } => {
                FPS.store(fps.clamp(1, 120) as u64, Ordering::Relaxed)
            }
            GameCommand::SetPaneSize { w, h } => f.resize(w.max(1), h.max(1)),
            // Everything else is a later W: sprites (W10), the legacy 5x7 text
            // overlay (W4), sound (W6), the runtime shader (W11), per-scanline
            // palette and scroll (W9). Swallowed rather than logged — a game
            // calling them must not spam the console sixty times a second.
            _ => {}
        }
    }
}

/// Copy the current frame out for the pump. Answers `(w, h, indices, palette)`.
pub fn take_frame() -> Option<(u32, u32, Vec<u8>, Vec<u32>)> {
    let presented = PRESENTED.load(Ordering::Acquire);
    if presented == UPLOADED.load(Ordering::Relaxed) {
        return None;
    }
    let g = shared();
    let f = g.lock().ok()?;
    UPLOADED.store(presented, Ordering::Relaxed);
    Some((f.w, f.h, f.indices.clone(), f.palette.clone()))
}

/// The size the pane currently is, for the input driver's pixel scaling.
pub fn pane_size() -> (u32, u32) {
    shared()
        .lock()
        .map(|f| (f.w, f.h))
        .unwrap_or((DEF_W, DEF_H))
}

// ── the renderer, reached by handle ─────────────────────────────────────────

/// The four `winui_render` entry points the pump needs, resolved once from the
/// module the GUEST already loaded.
///
/// Resolved rather than linked: `winui_render` is a `cdylib` precisely so its
/// device, swapchain and per-hwnd map exist once. `GetModuleHandleA` on an
/// already-mapped DLL answers that same copy, so these are the identical
/// functions the guest calls, operating on the identical state — and the pump
/// runs on the UI thread, which is the thread that state is local to.
struct RenderApi {
    plane: unsafe extern "C" fn(i64, i64, i64) -> i64,
    index_plane: unsafe extern "C" fn(i64) -> i64,
    palette: unsafe extern "C" fn(i64) -> i64,
    present: unsafe extern "C" fn(i64) -> i64,
    clear: unsafe extern "C" fn(i64, i64, i64) -> i64,
}

#[allow(unsafe_code)]
fn render_api() -> Option<&'static RenderApi> {
    static API: OnceLock<Option<RenderApi>> = OnceLock::new();
    API.get_or_init(|| {
        let sym = |name: &str| macvm::runtime::winkb::resolve_export(Some("winui_render.dll"), name);
        // SAFETY: every address comes from GetProcAddress on the loaded
        // renderer, and each signature is transcribed from that function's
        // own `#[no_mangle] extern "C"` declaration in `winui_render/src/pane.rs`.
        unsafe {
            Some(RenderApi {
                plane: std::mem::transmute::<u64, unsafe extern "C" fn(i64, i64, i64) -> i64>(
                    sym("MacvmRenderPixelPlane")?,
                ),
                index_plane: std::mem::transmute::<u64, unsafe extern "C" fn(i64) -> i64>(sym(
                    "MacvmRenderIndexPlane",
                )?),
                palette: std::mem::transmute::<u64, unsafe extern "C" fn(i64) -> i64>(sym(
                    "MacvmRenderPalette",
                )?),
                present: std::mem::transmute::<u64, unsafe extern "C" fn(i64) -> i64>(sym(
                    "MacvmRenderPresent",
                )?),
                clear: std::mem::transmute::<u64, unsafe extern "C" fn(i64, i64, i64) -> i64>(
                    sym("MacvmRenderClear")?,
                ),
            })
        }
    })
    .as_ref()
}

/// Upload the pending frame to `hwnd`'s pane and present it. Answers whether a
/// frame was shown.
///
/// Called from the pump between dispatches, never from a wndproc.
#[allow(unsafe_code)]
pub fn upload_and_present(hwnd: i64) -> bool {
    if hwnd == 0 {
        return false;
    }
    let Some(api) = render_api() else { return false };
    let Some((w, h, indices, palette)) = take_frame() else {
        return false;
    };
    // SAFETY: the renderer's own contract — ask for the plane (which creates or
    // resizes it), then for the index buffer and the palette, and write within
    // the sizes just requested. Addresses are re-asked every frame because a
    // resize reallocates them; caching one across a resize is the dangling
    // pointer this contract exists to prevent.
    unsafe {
        if (api.plane)(hwnd, w as i64, h as i64) == 0 {
            return false;
        }
        let ix = (api.index_plane)(hwnd);
        if ix == 0 {
            return false;
        }
        std::ptr::copy_nonoverlapping(indices.as_ptr(), ix as *mut u8, indices.len());
        let pal = (api.palette)(hwnd);
        if pal == 0 {
            return false;
        }
        std::ptr::copy_nonoverlapping(palette.as_ptr(), pal as *mut u32, palette.len());
        // A GAME OWNS THE WHOLE PANE. The Canvas's own demos leave a cell grid
        // behind — the plasma HUD sat over the first game frame ever presented
        // here, which is the shell talking over the guest. Clearing to
        // BG_TRANSPARENT (0x0100_0000) makes every cell skip its background
        // fill and draw a blank glyph, so the grid is present and invisible
        // and the game's pixels are all there is. Cheap: a memset of
        // cols*rows*12 bytes, once a frame.
        let _ = (api.clear)(hwnd, 0x00FF_FFFF, 0x0100_0000);
        (api.present)(hwnd) == 0
    }
}

/// Install the sink on `vm` — the primary, where a game's step block runs.
pub fn install(vm: &mut macvm::embed::VmHandle) {
    vm.set_game_sink(Box::new(WinGameSink));
}
