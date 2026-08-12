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

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use macvm::embed::{GameCommand, GameSink};

use crate::text_overlay::TextOverlay;

// WG11-W3: the input driver lives in its own module; re-exported here so
// the pump and the primary's loop have one name for "the game host".
pub use crate::game_input::{escape_pressed, frame_period, set_pane_hwnd, step_doit};

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
    /// **W4.** The retained 5x7 RGB text layer, drawn ABOVE the indexed pane.
    /// It lives INSIDE the frame, under the same lock, so a game's HUD update
    /// cannot tear across a frame the pump is presenting.
    pub text: TextOverlay,
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
            text: TextOverlay::new(w, h),
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
        // A real resize LOSES the ink, matching upstream: rebuilding the pane
        // there drops the whole NativeGame and builds a fresh TextOverlay.
        self.text.resize(w, h);
    }
}

/// **WG11-W7: the direct framebuffer's rotating set.**
///
/// Three host-owned buffers the GUEST writes pixels into through an `Alien`,
/// published to `embed`'s registry so `screenMemory` can hand out the one for
/// the frame being drawn. The ring is what decouples a demo that has already
/// started frame N+1 from a host still uploading frame N — upstream's own
/// comment records the tearing that taught it.
///
/// HOST MEMORY, NOT A MAPPED TEXTURE, and that is the Windows difference. D3D11
/// hands out a `Map`ped pointer only between Map and Unmap and it may not be
/// held across frames, so this is a staging region copied into the renderer's
/// plane on Present — one copy where Metal has none. The guest contract is
/// identical either way, which is why it was written as an address and a
/// stride rather than as a texture.
///
/// LEAKED DELIBERATELY (`Box::leak`): the registry publishes raw pointers that
/// the guest may hold for the life of a frame, and upstream's contract is that
/// they stay valid until `clear_screen_memory`. A demo re-opening at a new size
/// leaks one set of buffers, which is bounded by how many times a user opens a
/// demo and is the safe direction — freeing under a writing VM is not.
struct DirectRing {
    w: u32,
    h: u32,
    stride: usize,
    bufs: Vec<&'static mut [u8]>,
}

static DIRECT: Mutex<Option<DirectRing>> = Mutex::new(None);
/// The HOST's own count of rendered frames — the twin of the VM's present
/// count. Both sides pick `frame % nbuf`, so they agree on which buffer a frame
/// belongs to without either waiting on the other.
static DIRECT_FRAME: AtomicU64 = AtomicU64::new(0);

/// How many buffers rotate. Three: one being written, one being uploaded, one
/// spare — the same depth upstream settled on.
const DIRECT_NBUF: usize = 3;

/// Build (or rebuild) the ring at `w`x`h` and publish it. Answers the stride.
fn open_direct(w: u32, h: u32) -> usize {
    let stride = w as usize;
    let mut guard = match DIRECT.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    if let Some(r) = guard.as_ref() {
        if r.w == w && r.h == h {
            return r.stride;
        }
    }
    let mut bufs: Vec<&'static mut [u8]> = Vec::with_capacity(DIRECT_NBUF);
    let mut ptrs: Vec<*mut u8> = Vec::with_capacity(DIRECT_NBUF);
    for _ in 0..DIRECT_NBUF {
        let b: &'static mut [u8] =
            Box::leak(vec![0u8; stride * h as usize].into_boxed_slice());
        ptrs.push(b.as_mut_ptr());
        bufs.push(b);
    }
    macvm::embed::publish_screen_buffers(&ptrs, stride, h as usize);
    DIRECT_FRAME.store(0, Ordering::Relaxed);
    *guard = Some(DirectRing { w, h, stride, bufs });
    stride
}

/// The buffer the HOST should read for the frame it is about to upload, copied
/// out. `None` when no direct pane is open.
fn direct_frame() -> Option<(u32, u32, Vec<u8>)> {
    let guard = DIRECT.lock().ok()?;
    let r = guard.as_ref()?;
    let slot = (DIRECT_FRAME.load(Ordering::Relaxed) as usize) % r.bufs.len();
    Some((r.w, r.h, r.bufs[slot].to_vec()))
}

/// **WG11-W8: the text plane (SM1).**
///
/// `cols * rows` cells of four bytes — `[char, fg, bg, flags]`, fg and bg being
/// PALETTE INDICES — that the guest writes stores into through an `Alien`. A HUD
/// redrawn every frame costs nothing on the command channel, which is the whole
/// of SM1.
///
/// CELLS ARE A FIXED 6x8, not DirectWrite metrics: a 5x7 glyph plus one column
/// of spacing and one row of leading, so a 320x240 pane is 53x30. Upstream's
/// demos hard-code rows against exactly that (`row: 28` for a bottom line), and
/// a grid sized from a font would put their HUDs in the wrong place or clip
/// them. This is emulated hardware; its cell size is part of the spec.
///
/// ONE GRID, IN PLACE — it does not rotate, so an Alien over it may be kept for
/// the life of the pane. Leaked for the same reason the pixel ring is.
const CELL_W: u32 = 6;
const CELL_H: u32 = 8;

struct TextPlane {
    cols: usize,
    rows: usize,
    cells: &'static mut [u8],
}

static TEXTP: Mutex<Option<TextPlane>> = Mutex::new(None);

/// Build (or rebuild) the grid for a `w`x`h` pane and publish it.
fn open_text_plane(w: u32, h: u32) {
    let cols = (w / CELL_W).max(1) as usize;
    let rows = (h / CELL_H).max(1) as usize;
    let mut guard = match TEXTP.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some(t) = guard.as_ref() {
        if t.cols == cols && t.rows == rows {
            return;
        }
    }
    let cells: &'static mut [u8] = Box::leak(vec![0u8; cols * rows * 4].into_boxed_slice());
    macvm::embed::publish_text_memory(cells.as_mut_ptr(), cols, rows);
    *guard = Some(TextPlane { cols, rows, cells });
}

/// Snapshot the grid for the pump. `None` when nothing has been written — a
/// game with no HUD pays nothing.
fn text_cells() -> Option<(usize, usize, Vec<u8>)> {
    let guard = TEXTP.lock().ok()?;
    let t = guard.as_ref()?;
    // Char 0 means "unused cell, draws nothing", so an all-zero grid is empty.
    if t.cells.iter().step_by(4).all(|&c| c == 0) {
        return None;
    }
    Some((t.cols, t.rows, t.cells.to_vec()))
}

/// Draw the cell grid into `dst` (BGRA, `w`x`h`) through `palette`.
///
/// CHAR 0 DRAWS NOTHING — not a black square. That is what lets a HUD occupy
/// one row of a 53x30 grid and leave the game visible everywhere else.
fn draw_text_plane(dst: &mut [u32], w: u32, h: u32, palette: &[u32]) {
    let Some((cols, rows, cells)) = text_cells() else {
        return;
    };
    let rgb = |i: u8| -> (u8, u8, u8) {
        let c = palette.get(i as usize).copied().unwrap_or(0xFF00_0000);
        (((c >> 16) & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, (c & 0xFF) as u8)
    };
    for row in 0..rows {
        for col in 0..cols {
            let o = (row * cols + col) * 4;
            let ch = cells[o];
            if ch == 0 {
                continue;
            }
            let (x, y) = ((col as u32 * CELL_W) as i64, (row as u32 * CELL_H) as i64);
            let bg = cells[o + 2];
            if bg != 0 {
                // A non-zero background paints the whole cell first; zero means
                // transparent, which is how a HUD sits over the picture.
                let (br, bgc, bb) = rgb(bg);
                let word = 0xFF00_0000 | ((br as u32) << 16) | ((bgc as u32) << 8) | bb as u32;
                for dy in 0..CELL_H as i64 {
                    for dx in 0..CELL_W as i64 {
                        let (px, py) = (x + dx, y + dy);
                        if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                            dst[py as usize * w as usize + px as usize] = word;
                        }
                    }
                }
            }
            let (fr, fg, fb) = rgb(cells[o + 1]);
            crate::text_overlay::draw_char(dst, w, h, x, y, ch as char, fr, fg, fb, 1);
        }
    }
}

/// **WG11-W9: the palette as memory (SM4).**
///
/// `h * 16 + 240` RGBA quads the guest stores into — `h` groups of sixteen
/// PER-SCANLINE colours, then 240 globals. Re-colouring the whole screen costs
/// the palette's size rather than the screen's, which is the whole trick behind
/// Copper: the picture is drawn once, never redrawn, and only what colour
/// index 1 MEANS on each scanline changes.
///
/// RGBA HERE, BGRA THERE. The guest writes RGBA bytes (upstream's layout, and
/// what its demos assume); the renderer's palette texture is BGRA words. The
/// swizzle happens once per frame on upload, in `copper_palette`, rather than
/// asking either side to speak the other's byte order.
struct PalettePlane {
    entries: usize,
    global_base: usize,
    bytes: &'static mut [u8],
}

static PAL: Mutex<Option<PalettePlane>> = Mutex::new(None);

fn open_palette(h: u32) {
    let global_base = h as usize * 16;
    let entries = global_base + 240;
    let mut guard = match PAL.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some(pp) = guard.as_ref() {
        if pp.entries == entries {
            return;
        }
    }
    let bytes: &'static mut [u8] = Box::leak(vec![0u8; entries * 4].into_boxed_slice());
    macvm::embed::publish_palette_memory(bytes.as_mut_ptr(), entries, global_base);
    *guard = Some(PalettePlane { entries, global_base, bytes });
}

/// The copper table as the renderer wants it: BGRA words, in the same slot
/// order `PixelPlane::palette_slot` computes. `None` until a guest writes one.
fn copper_palette() -> Option<Vec<u32>> {
    let guard = PAL.lock().ok()?;
    let pp = guard.as_ref()?;
    if pp.bytes.iter().all(|&b| b == 0) {
        return None;
    }
    let mut out = Vec::with_capacity(pp.entries);
    for e in 0..pp.entries {
        let o = e * 4;
        let (r, g, b) = (pp.bytes[o], pp.bytes[o + 1], pp.bytes[o + 2]);
        out.push(0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
    }
    let _ = pp.global_base;
    Some(out)
}

/// **WG11-W10: sprites.**
///
/// A definition is `/`-separated hex rows, FOUR BITS PER PIXEL — each nibble a
/// palette index into the sprite's OWN sixteen colours, not the pane's. Index 0
/// is transparent, which is what lets a sprite be a shape rather than a tile.
///
/// Frames live in one definition (`addFrame:`), so an animation is one id with
/// several pictures and `place:x:y:frame:` selects among them. Composited over
/// the pane in id order at present time — upstream draws them after the indexed
/// plane and before the text overlay, and so does this.
struct SpriteDef {
    frames: Vec<(u32, u32, Vec<u8>)>, // w, h, nibble-per-pixel indices
    palette: [u32; 16],
    x: i64,
    y: i64,
    frame: usize,
    visible: bool,
}

static SPRITES: Mutex<Option<Vec<(i64, SpriteDef)>>> = Mutex::new(None);

/// Parse `/`-separated hex rows into `(w, h, indices)`.
fn parse_sprite(rows: &str) -> (u32, u32, Vec<u8>) {
    let lines: Vec<&str> = rows.split('/').filter(|l| !l.is_empty()).collect();
    let w = lines.iter().map(|l| l.len()).max().unwrap_or(0) as u32;
    let h = lines.len() as u32;
    let mut px = vec![0u8; (w * h) as usize];
    for (y, line) in lines.iter().enumerate() {
        for (x, c) in line.chars().enumerate() {
            px[y * w as usize + x] = c.to_digit(16).unwrap_or(0) as u8;
        }
    }
    (w, h, px)
}

fn with_sprites<T>(f: impl FnOnce(&mut Vec<(i64, SpriteDef)>) -> T) -> Option<T> {
    let mut g = SPRITES.lock().ok()?;
    Some(f(g.get_or_insert_with(Vec::new)))
}

/// Draw every visible sprite into `dst`, in id order. Index 0 is transparent.
fn draw_sprites(dst: &mut [u32], w: u32, h: u32) {
    let _ = with_sprites(|v| {
        for (_, sp) in v.iter() {
            if !sp.visible {
                continue;
            }
            let Some((sw, sh, px)) = sp.frames.get(sp.frame.min(sp.frames.len().saturating_sub(1)))
            else {
                continue;
            };
            for sy in 0..*sh as i64 {
                for sx in 0..*sw as i64 {
                    let ci = px[(sy as usize) * *sw as usize + sx as usize];
                    if ci == 0 {
                        continue;
                    }
                    let (x, y) = (sp.x + sx, sp.y + sy);
                    if x < 0 || y < 0 || x as u32 >= w || y as u32 >= h {
                        continue;
                    }
                    dst[y as usize * w as usize + x as usize] = sp.palette[ci as usize & 15];
                }
            }
        }
    });
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
/// WG11-W9: `overscan:`'s margin, and the viewport's pan within the world.
static OVERSCAN: AtomicUsize = AtomicUsize::new(0);
static SCROLL_X: AtomicUsize = AtomicUsize::new(0);
static SCROLL_Y: AtomicUsize = AtomicUsize::new(0);

// ── WG11-W11: the layer-0 background shader ─────────────────────────────────
//
// The sink runs on the primary's thread; the D3D device lives on the UI
// thread's `thread_local` state. So the arms below only STASH — the pump
// compiles the source and pushes the params at present time, the same hand-off
// the copper palette already makes. One consequence, accepted: upstream's
// Cocoa host compiles synchronously in the command arm, so here a compile
// error surfaces one frame later and on stderr rather than at the send site.
// The GAME sees no difference either way — `shader:` answers self on both
// platforms, and a bad shader leaves the previous one (or none) in force.
static SHADER_SRC: Mutex<Option<String>> = Mutex::new(None);
static SHADER_PARAMS: [AtomicU32; 8] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: AtomicU32 = AtomicU32::new(0);
    [Z; 8]
};
/// True once a source COMPILED — what gates the copper treatment below, so a
/// failed compile leaves the pane's software look entirely untouched.
static SHADER_LIVE: AtomicBool = AtomicBool::new(false);
/// Set by `stop()`: the pump must drop the renderer's shader before the next
/// game's first frame, or that game inherits this one's cosmos.
static SHADER_CLEAR: AtomicBool = AtomicBool::new(false);

/// WG11-W11: `linePaletteAt:` (261) overrides, `(h, h*16 words)`, 0 = unset —
/// safe as a sentinel because every SET word carries FF alpha. Kept apart from
/// the SM4 palette plane (`PAL`): that one is guest memory with its own
/// all-or-nothing copper contract, and a game that only sends commands (as
/// galaxigans does, for its tractor beam) must not be mistaken for a guest
/// that took ownership of the whole table.
static LINE_PAL: Mutex<Option<(u32, Vec<u32>)>> = Mutex::new(None);

fn line_pal_set(h: u32, line: u32, index: u8, word: u32) {
    if index == 0 || index > 15 || line >= h {
        return;
    }
    if let Ok(mut g) = LINE_PAL.lock() {
        let entry = g.get_or_insert_with(|| (h, vec![0u32; h as usize * 16]));
        if entry.0 != h {
            *entry = (h, vec![0u32; h as usize * 16]);
        }
        entry.1[line as usize * 16 + index as usize] = word;
    }
}

/// The copper table this frame renders through, or `None` for the flat 256.
///
/// Three ways in, in priority order: a guest that wrote SM4 palette memory
/// owns the whole table (`copper_palette`); a game that sent `linePaletteAt:`
/// gets its overrides laid over the flat palette's defaults; and a LIVE SHADER
/// forces the copper layout by itself — upstream's pane always has it, and it
/// is precisely what makes index 0 a hole the shader can show through.
fn effective_copper(h: u32, flat: &[u32], shader_live: bool) -> Option<Vec<u32>> {
    if let Some(cp) = copper_palette() {
        return Some(cp);
    }
    let lp = match LINE_PAL.lock().ok().and_then(|g| g.clone()) {
        Some((lh, v)) if lh == h => Some(v),
        _ => None,
    };
    if lp.is_none() && !shader_live {
        return None;
    }
    let gb = h as usize * 16;
    let mut out = vec![0u32; gb + 240];
    for y in 0..h as usize {
        for i in 1..16usize {
            let over = lp.as_ref().map(|v| v[y * 16 + i]).unwrap_or(0);
            out[y * 16 + i] = if over != 0 {
                over
            } else {
                flat.get(i).copied().unwrap_or(0xFF00_0000)
            };
        }
    }
    for j in 0..240usize {
        out[gb + j] = flat.get(16 + j).copied().unwrap_or(0xFF00_0000);
    }
    Some(out)
}

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
            // ── W4: the retained 5x7 text overlay ────────────────────────
            // Primitive 254 packs its colour as one 0xRRGGBB SmallInteger (to
            // stay inside MAX_PRIMITIVE_ARGS) and unpacks it before emitting,
            // so the command already carries separate bytes.
            GameCommand::Text {
                x,
                y,
                text,
                r,
                g: gg,
                b,
                scale,
            } => f.text.draw_text(x, y, &text, r, gg, b, scale),
            // The ONLY eraser. `Present` must never clear it: retention is the
            // contract `45a_life.mst`'s HUD depends on.
            GameCommand::TextClear => f.text.clear(),
            GameCommand::Present => {
                // WG11-W7: the pixels a demo wrote STRAIGHT into the ring are
                // the frame. Copied into the sink's own buffer here, so the
                // upload path downstream is identical whether the game drew
                // through commands or through memory.
                if let Some((dw, dh, px)) = direct_frame() {
                    if dw == f.w && dh == f.h {
                        let n = px.len().min(f.indices.len());
                        f.indices[..n].copy_from_slice(&px[..n]);
                    }
                    DIRECT_FRAME.fetch_add(1, Ordering::AcqRel);
                }
                PRESENTED.fetch_add(1, Ordering::Release);
                f.generation += 1;
                // Never post while holding the frame the pump is about to take.
                drop(f);
                // WAKE THE PUMP. Without this it sits in `GetMessageW` and the
                // upload is paced by the shell's 50 ms heartbeat — 20 fps by
                // accident, with up to 50 ms of latency. The Mac wakes main on
                // every emit; waking on Present is the right adaptation here,
                // because this host buffers the whole frame on the CPU and only
                // Present makes it showable.
                crate::app::wake_ui_thread();
            }
            GameCommand::SetOverscan { margin } => {
                // The WORLD grows; the viewport does not. The plane is
                // reallocated at world size and the shader pans within it.
                let (vw, vh) = (f.w, f.h);
                let _ = (vw, vh);
                OVERSCAN.store(margin as usize, Ordering::Relaxed);
            }
            GameCommand::Scroll { x, y } => {
                SCROLL_X.store(x.max(0) as usize, Ordering::Relaxed);
                SCROLL_Y.store(y.max(0) as usize, Ordering::Relaxed);
            }
            GameCommand::DefineSprite { id, rows } => {
                let f = parse_sprite(&rows);
                let _ = with_sprites(|v| {
                    v.retain(|(i, _)| *i != id);
                    v.push((
                        id,
                        SpriteDef {
                            frames: vec![f],
                            // Opaque black until the game says otherwise; slot 0
                            // is never drawn, so its value cannot show.
                            palette: [0xFF00_0000; 16],
                            x: 0,
                            y: 0,
                            frame: 0,
                            visible: true,
                        },
                    ));
                    v.sort_by_key(|(i, _)| *i);
                });
            }
            GameCommand::AddFrame { id, rows } => {
                let f = parse_sprite(&rows);
                let _ = with_sprites(|v| {
                    if let Some((_, sp)) = v.iter_mut().find(|(i, _)| *i == id) {
                        sp.frames.push(f);
                    }
                });
            }
            GameCommand::SpriteColor { id, index, r, g: gg, b } => {
                let _ = with_sprites(|v| {
                    if let Some((_, sp)) = v.iter_mut().find(|(i, _)| *i == id) {
                        sp.palette[index as usize & 15] =
                            0xFF00_0000 | ((r as u32) << 16) | ((gg as u32) << 8) | b as u32;
                    }
                });
            }
            GameCommand::MoveSprite { id, x, y } => {
                let _ = with_sprites(|v| {
                    if let Some((_, sp)) = v.iter_mut().find(|(i, _)| *i == id) {
                        sp.x = x;
                        sp.y = y;
                        sp.visible = true;
                    }
                });
            }
            GameCommand::PlaceSprite { id, x, y, frame } => {
                let _ = with_sprites(|v| {
                    if let Some((_, sp)) = v.iter_mut().find(|(i, _)| *i == id) {
                        sp.x = x;
                        sp.y = y;
                        sp.frame = frame as usize;
                        sp.visible = true;
                    }
                });
            }
            GameCommand::HideSprite { id } => {
                let _ = with_sprites(|v| {
                    if let Some((_, sp)) = v.iter_mut().find(|(i, _)| *i == id) {
                        sp.visible = false;
                    }
                });
            }
            GameCommand::Shader { src } => {
                // Fresh params, like upstream's new ShaderPane: they are
                // driven by `shaderParam:value:` AFTER the compile, and a
                // recompile must not inherit the last game's scene number.
                for p in SHADER_PARAMS.iter() {
                    p.store(0, Ordering::Relaxed);
                }
                if let Ok(mut g) = SHADER_SRC.lock() {
                    *g = Some(src);
                }
            }
            GameCommand::ShaderParam { index, value } => {
                SHADER_PARAMS[index.min(7)].store(value.to_bits(), Ordering::Relaxed);
            }
            GameCommand::LinePalette {
                line,
                index,
                r,
                g: gg,
                b,
            } => {
                let h = f.h;
                line_pal_set(
                    h,
                    line,
                    index,
                    0xFF00_0000 | ((r as u32) << 16) | ((gg as u32) << 8) | b as u32,
                );
            }
            GameCommand::StartLoop => RUNNING.store(true, Ordering::Relaxed),
            GameCommand::StopLoop => RUNNING.store(false, Ordering::Relaxed),
            GameCommand::SetFrameRate { fps } => {
                FPS.store(fps.clamp(1, 120) as u64, Ordering::Relaxed)
            }
            GameCommand::SetPaneSize { w, h } => {
                f.resize(w.max(1), h.max(1));
                open_text_plane(w.max(1), h.max(1));
                open_palette(h.max(1));
            }
            GameCommand::OpenDirect { w, h } => {
                // The pane takes the framebuffer's shape, so a demo that opens
                // 320x240 direct gets a 320x240 pane without also sending
                // resizeTo:by: — which upstream's demos rely on.
                f.resize(w.max(1), h.max(1));
                drop(f);
                open_direct(w.max(1), h.max(1));
                open_text_plane(w.max(1), h.max(1));
                open_palette(h.max(1));
                return;
            }
            // Everything else is a later W: sprites (W10), the legacy 5x7 text
            // overlay (W4), sound (W6), the runtime shader (W11), per-scanline
            // palette and scroll (W9). Swallowed rather than logged — a game
            // calling them must not spam the console sixty times a second.
            _ => {}
        }
    }
}

/// Copy the current frame out for the pump. Answers `(w, h, indices, palette)`.
pub fn take_frame() -> Option<(u32, u32, Vec<u8>, Vec<u32>, Option<Vec<u32>>)> {
    let presented = PRESENTED.load(Ordering::Acquire);
    if presented == UPLOADED.load(Ordering::Relaxed) {
        return None;
    }
    let g = shared();
    let f = g.lock().ok()?;
    UPLOADED.store(presented, Ordering::Relaxed);
    // The overlay is snapshotted under the SAME lock as the pixels, and only
    // when it has ink — a game that never draws text pays nothing for it.
    let overlay = if f.text.has_ink() {
        Some(f.text.pixels().to_vec())
    } else {
        None
    };
    Some((f.w, f.h, f.indices.clone(), f.palette.clone(), overlay))
}

/// End the session host-side, the way the Mac's `close_window` does.
///
/// LOAD-BEARING. `GamePane class >> reset` runs the reset block and nils the
/// step block, but it sends no primitive — so it emits no `StopLoop`, and
/// `StopLoop` comes only from INSTANCE-side `GamePane>>stop`, which the class
/// does not understand. Without this, Escape leaves `RUNNING` true and the
/// primary steps a nil block at 60 Hz forever on the fast beat.
pub fn stop() {
    RUNNING.store(false, Ordering::Relaxed);
    TEXT_MODE.store(false, Ordering::Relaxed);
    // W11: the NEXT game must not inherit this one's cosmos. The renderer's
    // copy is dropped by the pump (which owns the D3D thread) on the next
    // frame it uploads — SHADER_CLEAR is that instruction.
    SHADER_LIVE.store(false, Ordering::Relaxed);
    SHADER_CLEAR.store(true, Ordering::Relaxed);
    if let Ok(mut g) = SHADER_SRC.lock() {
        *g = None;
    }
    for p in SHADER_PARAMS.iter() {
        p.store(0, Ordering::Relaxed);
    }
    if let Ok(mut g) = LINE_PAL.lock() {
        *g = None;
    }
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
    // W4: the three the text path needs.
    drop_plane: unsafe extern "C" fn(i64) -> i64,
    stride: unsafe extern "C" fn(i64) -> i64,
    palette_len: unsafe extern "C" fn(i64) -> i64,
    // W9: the copper contract and the viewport's pan.
    make_copper: unsafe extern "C" fn(i64) -> i64,
    scroll: unsafe extern "C" fn(i64, i64, i64) -> i64,
    // W11: the layer-0 background shader, and the compiler's reason when it
    // refuses one — read immediately, printed once, never per frame.
    shader: unsafe extern "C" fn(i64, *const u8, i64) -> i64,
    shader_param: unsafe extern "C" fn(i64, i64, f64) -> i64,
    last_error: unsafe extern "C" fn() -> *const u16,
    last_error_len: unsafe extern "C" fn() -> i64,
}

/// Which shape this module last left the renderer's plane in: `true` indexed,
/// `false` direct BGRA. A plane is indexed XOR direct and `make_indexed` is
/// one-way, so a change of mind costs a drop first. A stale belief is harmless:
/// dropping a plane that is already gone is a no-op, and `MacvmRenderPixelPlane`
/// always rebuilds non-indexed.
static PLANE_INDEXED: AtomicBool = AtomicBool::new(true);
/// Once a session has drawn text it keeps the direct path for the rest of the
/// session. Sticky because the mode belongs to the GAME, not to a frame — a HUD
/// that blinked would otherwise reallocate the plane twice a blink.
static TEXT_MODE: AtomicBool = AtomicBool::new(false);

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
                drop_plane: std::mem::transmute::<u64, unsafe extern "C" fn(i64) -> i64>(sym(
                    "MacvmRenderDropPixelPlane",
                )?),
                stride: std::mem::transmute::<u64, unsafe extern "C" fn(i64) -> i64>(sym(
                    "MacvmRenderPixelStride",
                )?),
                palette_len: std::mem::transmute::<u64, unsafe extern "C" fn(i64) -> i64>(sym(
                    "MacvmRenderPaletteLen",
                )?),
                make_copper: std::mem::transmute::<u64, unsafe extern "C" fn(i64) -> i64>(sym(
                    "MacvmRenderMakeCopper",
                )?),
                scroll: std::mem::transmute::<u64, unsafe extern "C" fn(i64, i64, i64) -> i64>(
                    sym("MacvmRenderScroll")?,
                ),
                shader: std::mem::transmute::<u64, unsafe extern "C" fn(i64, *const u8, i64) -> i64>(
                    sym("MacvmRenderShader")?,
                ),
                shader_param: std::mem::transmute::<u64, unsafe extern "C" fn(i64, i64, f64) -> i64>(
                    sym("MacvmRenderShaderParam")?,
                ),
                last_error: std::mem::transmute::<u64, unsafe extern "C" fn() -> *const u16>(sym(
                    "MacvmRenderLastError",
                )?),
                last_error_len: std::mem::transmute::<u64, unsafe extern "C" fn() -> i64>(sym(
                    "MacvmRenderLastErrorLen",
                )?),
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
    let Some((w, h, indices, palette, overlay)) = take_frame() else {
        return false;
    };
    let any_sprites = with_sprites(|v| v.iter().any(|(_, sp)| sp.visible)).unwrap_or(false);
    if overlay.is_some() || text_cells().is_some() || any_sprites {
        // Either text layer means RGB over indices, so the direct path.
        TEXT_MODE.store(true, Ordering::Relaxed);
    }
    let want_indexed = !TEXT_MODE.load(Ordering::Relaxed);
    // ── W11: the shader hand-off, on the thread that owns the device ──────
    if SHADER_CLEAR.swap(false, Ordering::Relaxed) {
        let _ = unsafe { (api.shader)(hwnd, std::ptr::null(), 0) };
    }
    if let Some(src) = SHADER_SRC.lock().ok().and_then(|mut g| g.take()) {
        let ok = unsafe { (api.shader)(hwnd, src.as_ptr(), src.len() as i64) } == 0;
        SHADER_LIVE.store(ok, Ordering::Relaxed);
        if !ok {
            // Once, at compile time — never per frame. The pane keeps its
            // software look; the compiler's reason goes to whoever is porting
            // the shader, which is the only party who can act on it.
            let reason = unsafe {
                let n = (api.last_error_len)().max(0) as usize;
                let p = (api.last_error)();
                if p.is_null() || n == 0 {
                    String::new()
                } else {
                    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
                }
            };
            eprintln!("macvm-win: game shader failed to compile: {reason}");
        }
    }
    let shader_live = SHADER_LIVE.load(Ordering::Relaxed);
    if shader_live {
        for (i, p) in SHADER_PARAMS.iter().enumerate() {
            let v = f32::from_bits(p.load(Ordering::Relaxed));
            unsafe { (api.shader_param)(hwnd, i as i64, v as f64) };
        }
    }
    // SAFETY: the renderer's own contract — ask for the plane (which creates or
    // resizes it), then for whichever buffers this mode uses, and write within
    // the sizes just requested. Addresses are re-asked every frame because a
    // resize reallocates them; caching one across a resize is the dangling
    // pointer this contract exists to prevent.
    unsafe {
        if PLANE_INDEXED.swap(want_indexed, Ordering::Relaxed) != want_indexed {
            // A plane cannot change shape in place, so it is dropped and
            // rebuilt. Once per session, not once per frame.
            let _ = (api.drop_plane)(hwnd);
        }
        let base = (api.plane)(hwnd, w as i64, h as i64);
        if base == 0 {
            return false;
        }
        if want_indexed {
            // THE FAST PATH: the GPU expands indices through the palette, so a
            // palette cycle stays a 1KB upload.
            let ix = (api.index_plane)(hwnd);
            if ix == 0 {
                return false;
            }
            std::ptr::copy_nonoverlapping(indices.as_ptr(), ix as *mut u8, indices.len());
            // W9: a guest that wrote a palette gets the COPPER contract —
            // index 0 transparent, 1..15 per-scanline, 16..255 global — and the
            // shader's own scroll. Otherwise the flat 256 path is untouched.
            // W11 widened "gets the copper contract" to line-palette commands
            // and to a live shader, whose holes are the whole point.
            let copper = effective_copper(h, &palette, shader_live);
            if copper.is_some() {
                (api.make_copper)(hwnd);
                (api.scroll)(
                    hwnd,
                    SCROLL_X.load(Ordering::Relaxed) as i64,
                    SCROLL_Y.load(Ordering::Relaxed) as i64,
                );
            }
            let pal = (api.palette)(hwnd);
            if pal == 0 {
                return false;
            }
            let palette = copper.unwrap_or(palette);
            // ASKED, not assumed — W2 made a copper table `h*16+240` entries and
            // added this entry point precisely so a caller cannot write its
            // globals into scanline 16's per-line colours.
            let n = ((api.palette_len)(hwnd).max(0) as usize).min(palette.len());
            std::ptr::copy_nonoverlapping(palette.as_ptr(), pal as *mut u32, n);
        } else {
            // THE TEXT PATH. The overlay is RGB and the plane is indices, and an
            // indexed plane's `bytes` is dead memory — so the resolve the
            // renderer would do happens HERE, with the glyphs composited on top,
            // and the result is handed over as direct BGRA. One pass over w*h
            // words. Rejected alternative: rasterising glyphs into the index
            // buffer against reserved palette slots — collision-free for Life,
            // but it caps text at N colours and diverges from upstream, where
            // Text is true RGB.
            let stride = (api.stride)(hwnd);
            if stride <= 0 {
                return false;
            }
            // W9: the COPPER on the direct path. A game with a text HUD takes
            // this branch, and Copper is exactly that — so without resolving
            // per-scanline here its bars came out black while its HUD read
            // fine. The arithmetic is PixelPlane::palette_slot's, in the one
            // other place that has to agree with it.
            if let Some(cp) = effective_copper(h, &palette, shader_live) {
                let gb = h as usize * 16;
                let dst = std::slice::from_raw_parts_mut(base as *mut u32, w as usize * h as usize);
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        let ci = indices[y * w as usize + x];
                        let word = if ci == 0 {
                            // W11: over a live shader, index 0 is written as
                            // TRANSPARENT and the renderer blends this plane —
                            // the CPU-resolved twin of `ps_indexed`'s discard.
                            // With no shader it stays opaque black, upstream's
                            // own backdrop behind an empty pane.
                            if shader_live { 0x0000_0000 } else { 0xFF00_0000 }
                        } else if ci < 16 {
                            cp.get(y * 16 + ci as usize).copied().unwrap_or(0xFF00_0000)
                        } else {
                            cp.get(gb + ci as usize - 16).copied().unwrap_or(0xFF00_0000)
                        };
                        dst[y * w as usize + x] = word;
                    }
                }
                draw_sprites(dst, w, h);
                draw_text_plane(dst, w, h, &palette);
                // The legacy 5x7 overlay tops the stack, exactly as in the
                // non-copper branch below — upstream's galaxigans writes its
                // whole HUD through it, and W11's copper treatment routed that
                // game HERE, where the overlay had never been composited
                // because the Copper demo (this branch's author) uses the SM1
                // cell grid instead.
                if let Some(ov) = overlay.as_deref() {
                    crate::text_overlay::composite_over(dst, ov);
                }
                let _ = (api.clear)(hwnd, 0x00FF_FFFF, 0x0100_0000);
                return (api.present)(hwnd) == 0;
            }
            crate::text_overlay::resolve_composite_into(
                base as *mut u8,
                stride as usize,
                w,
                h,
                &indices,
                &palette,
                overlay.as_deref(),
            );
            // W8: the CELL grid goes on last, over the pixels and over the
            // legacy overlay — it is the topmost layer, as it is upstream.
            if stride as usize == w as usize * 4 {
                let dst = std::slice::from_raw_parts_mut(
                    base as *mut u32,
                    w as usize * h as usize,
                );
                // Sprites over the pixels, text over the sprites — upstream's
                // layer order, so a HUD is never hidden by a sprite.
                draw_sprites(dst, w, h);
                draw_text_plane(dst, w, h, &palette);
            }
        }
        // A GAME OWNS THE WHOLE PANE.
        let _ = (api.clear)(hwnd, 0x00FF_FFFF, 0x0100_0000);
        (api.present)(hwnd) == 0
    }
}

/// Install the sink on `vm` — the primary, where a game's step block runs.
pub fn install(vm: &mut macvm::embed::VmHandle) {
    // The default pane has a text plane from the start: Minesweeper writes its
    // HUD without ever resizing, and `textMemory` must not fail for it.
    open_text_plane(DEF_W, DEF_H);
    open_palette(DEF_H);
    vm.set_game_sink(Box::new(WinGameSink));
}
