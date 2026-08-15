//! WG11-W4: the 5x7 text overlay — upstream primitives 254
//! (`GamePane>>text:x:y:rgb:scale:`) and 255 (`GamePane>>textClear`).
//!
//! Transcribed from the vendored Metal reference,
//! `MacGamePane/graphics/src/text_overlay.rs`, glyph table for glyph table.
//! This is NOT the cell grid: it is a pixel-positioned RGB layer that sits
//! ABOVE the indexed game pane, and it is **retained between frames** — only
//! `TextClear` erases it (see [`TextOverlay::clear`]).
//!
//! **Font**: a real 5x7 dot-matrix font (5 columns wide, 7 rows tall per
//! glyph). Covered characters: uppercase `A`-`Z`, digits `0`-`9`, space, and
//! the punctuation `. , : - ' ! ? ( ) / + < > &`. Lowercase `a`-`z` render via
//! their uppercase glyphs (retro titles are conventionally all-caps). Any
//! character with no glyph renders as a hollow placeholder box — visibly
//! present and correctly spaced, never silently dropped.
//!
//! Each glyph is a `[u8; 7]`: one byte per row, top to bottom, where bits
//! 4..0 are the five columns left to right (bit 4 = leftmost). [`draw_text`]
//! takes a `scale` (>= 1) that blocks each font pixel into a `scale`x`scale`
//! square, so one font serves both a scale-1 HUD and a scale-3+ title.
//!
//! ## Byte order: BGRA, not the Mac's RGBA
//!
//! The Mac overlay keeps an `RGBA8Unorm` byte buffer because that is the
//! texture format Metal samples. WINARM has no such texture: this overlay is
//! composited on the CPU into `winui_render`'s pixel plane, which is
//! `DXGI_FORMAT_B8G8R8A8_UNORM` (`winui_render/src/gpu.rs`) fed from
//! `PixelPlane::bytes`. So a pixel here is a **`u32` in `0xAARRGGBB` order**,
//! which little-endian stores as the bytes `B, G, R, A` — exactly the word
//! `PixelPlane::resolve` writes with `c.to_le_bytes()`, and exactly the word
//! `win_gui/src/game.rs` already builds for `PaletteAt`
//! (`0xFF00_0000 | (r << 16) | (g << 8) | b`). One packing rule for the whole
//! host: no per-frame channel swizzle anywhere.
//!
//! Alpha is the retention mechanism, not a blend factor. The Mac composites
//! the overlay with `SourceAlpha`/`OneMinusSourceAlpha`, but the font is
//! **binary** — every written pixel has alpha 255 and every other pixel has
//! alpha 0 — so that blend is bit-for-bit "replace where alpha != 0", which is
//! what [`composite_over`] does. Untouched pixels are [`TRANSPARENT`] (all
//! zero bytes), so a cleared overlay costs one `fill(0)`.

// ── geometry ────────────────────────────────────────────────────────────────

/// Glyph geometry, in un-scaled font pixels.
pub const GLYPH_W: u32 = 5; // columns per glyph
pub const GLYPH_H: u32 = 7; // rows per glyph
/// Horizontal advance per character = glyph width + one spacing column.
pub const GLYPH_ADVANCE: u32 = GLYPH_W + 1;

/// An untouched overlay pixel: alpha 0, so [`composite_over`] skips it.
pub const TRANSPARENT: u32 = 0x0000_0000;

/// Pack an RGB triple into the renderer's plane word: `0xAARRGGBB`, opaque,
/// which little-endian stores as `B, G, R, A` for `B8G8R8A8_UNORM`.
#[inline]
pub const fn bgra(r: u8, g: u8, b: u8) -> u32 {
    0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// ── the font ────────────────────────────────────────────────────────────────

// Font tables. Each glyph is `GLYPH_H` rows top-to-bottom; each row's bits
// 4..0 are the `GLYPH_W` columns left-to-right (bit 4 = leftmost, bit 0 =
// rightmost). Read a row in binary to see the glyph, e.g. 0x0E == 0b01110 ==
// ".###.".
pub type Glyph = [u8; GLYPH_H as usize];

/// `0`-`9`, indexed by `digit - '0'`.
pub const DIGIT_GLYPHS: [Glyph; 10] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0  (ring with a diagonal)
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
    [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F], // 2
    [0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E], // 3
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], // 6
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
];

/// `A`-`Z`, indexed by `letter - 'A'`.
pub const LETTER_GLYPHS: [Glyph; 26] = [
    [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11], // A
    [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E], // B
    [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E], // C
    [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C], // D
    [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F], // E
    [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10], // F
    [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E], // G
    [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11], // H
    [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E], // I
    [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C], // J
    [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11], // K
    [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F], // L
    [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11], // M
    [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11], // N
    [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // O
    [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10], // P
    [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D], // Q
    [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11], // R
    [0x0E, 0x11, 0x10, 0x0E, 0x01, 0x11, 0x0E], // S
    [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04], // T
    [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // U
    [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04], // V
    [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11], // W
    [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11], // X
    [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04], // Y
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F], // Z
];

/// Rendered for any character without a real glyph: a hollow box, so an
/// unsupported character is visibly present (right position, right width)
/// rather than silently missing.
pub const PLACEHOLDER_GLYPH: Glyph = [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F];

/// The 5x7 bitmask for `ch`. Lowercase folds to uppercase; anything without a
/// dedicated glyph falls back to [`PLACEHOLDER_GLYPH`]. Space is blank.
pub fn glyph_for(ch: char) -> Glyph {
    let up = ch.to_ascii_uppercase();
    match up {
        ' ' => [0; 7],
        '0'..='9' => DIGIT_GLYPHS[up as usize - '0' as usize],
        'A'..='Z' => LETTER_GLYPHS[up as usize - 'A' as usize],
        '.' => [0, 0, 0, 0, 0, 0x0C, 0x0C],
        ',' => [0, 0, 0, 0, 0x06, 0x04, 0x08],
        ':' => [0, 0x0C, 0x0C, 0, 0x0C, 0x0C, 0],
        '-' => [0, 0, 0, 0x0E, 0, 0, 0],
        '\'' => [0x04, 0x04, 0x08, 0, 0, 0, 0],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0, 0x04],
        '?' => [0x0E, 0x11, 0x01, 0x06, 0x04, 0, 0x04],
        '(' => [0x04, 0x08, 0x10, 0x10, 0x10, 0x08, 0x04],
        ')' => [0x04, 0x02, 0x01, 0x01, 0x01, 0x02, 0x04],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '+' => [0, 0x04, 0x04, 0x1F, 0x04, 0x04, 0],
        '<' => [0x01, 0x02, 0x04, 0x08, 0x04, 0x02, 0x01],
        '>' => [0x10, 0x08, 0x04, 0x02, 0x04, 0x08, 0x10],
        '&' => [0x0C, 0x12, 0x12, 0x0C, 0x15, 0x12, 0x0D],
        _ => PLACEHOLDER_GLYPH,
    }
}

// ── drawing into a plain BGRA buffer ────────────────────────────────────────

/// One overlay pixel. Out of bounds is DROPPED, never wrapped — a HUD drawn at
/// x = -3 loses its left columns instead of reappearing on the right edge.
#[inline]
fn set_pixel(buf: &mut [u32], w: u32, h: u32, x: i64, y: i64, colour: u32) {
    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
        return;
    }
    let i = y as usize * w as usize + x as usize;
    if i < buf.len() {
        buf[i] = colour;
    }
}

/// One scaled font pixel: a `w`x`h` block, clipped per-pixel by [`set_pixel`].
#[allow(clippy::too_many_arguments)]
fn fill_rect_px(buf: &mut [u32], bw: u32, bh: u32, x: i64, y: i64, w: i64, h: i64, colour: u32) {
    for row in y..y.saturating_add(h) {
        for col in x..x.saturating_add(w) {
            set_pixel(buf, bw, bh, col, row, colour);
        }
    }
}

/// Renders the single glyph for `ch` at `(x, y)` (its top-left corner), each
/// font pixel drawn as a `scale`x`scale` block.
#[allow(clippy::too_many_arguments)]
pub fn draw_char(
    buf: &mut [u32],
    bw: u32,
    bh: u32,
    x: i64,
    y: i64,
    ch: char,
    r: u8,
    g: u8,
    b: u8,
    scale: u32,
) {
    let s = scale.max(1) as i64;
    let colour = bgra(r, g, b);
    let rows = glyph_for(ch);
    for (row, &mask) in rows.iter().enumerate() {
        for col in 0..GLYPH_W {
            // bit (GLYPH_W - 1) is the leftmost column.
            if mask & (1 << (GLYPH_W - 1 - col)) != 0 {
                let px = x.saturating_add(col as i64 * s);
                let py = y.saturating_add(row as i64 * s);
                fill_rect_px(buf, bw, bh, px, py, s, s, colour);
            }
        }
    }
}

/// Draws `text` at `(x, y)` (top-left, in pixels) into the `w`x`h` BGRA buffer
/// `buf` in the 5x7 dot-matrix font, colour `(r, g, b)`. `scale` (clamped to
/// `>= 1`) blocks each font pixel into a `scale`x`scale` square.
///
/// Layout, for callers positioning text:
/// * horizontal advance per character = `(5 + 1) * scale` pixels
///   (= `GLYPH_ADVANCE * scale`), so a string of `n` chars spans
///   `n * GLYPH_ADVANCE * scale` px (the trailing spacing column included).
/// * glyph height = `7 * scale` pixels (`GLYPH_H * scale`).
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    buf: &mut [u32],
    w: u32,
    h: u32,
    x: i64,
    y: i64,
    text: &str,
    r: u8,
    g: u8,
    b: u8,
    scale: u32,
) {
    let scale = scale.max(1);
    let advance = (GLYPH_ADVANCE * scale) as i64;
    for (i, ch) in text.chars().enumerate() {
        let cell_x = x.saturating_add(i as i64 * advance);
        draw_char(buf, w, h, cell_x, y, ch, r, g, b, scale);
    }
}

// ── compositing ─────────────────────────────────────────────────────────────

/// Composite the overlay over `dst`, both `0xAARRGGBB` and the same length.
///
/// The Mac blends `SourceAlpha`/`OneMinusSourceAlpha`, but this font is binary
/// — alpha is 255 or 0 — so that blend reduces exactly to "replace where the
/// source alpha is non-zero". Bit-identical output, no per-pixel multiply.
pub fn composite_over(dst: &mut [u32], overlay: &[u32]) {
    for (d, &s) in dst.iter_mut().zip(overlay.iter()) {
        if s & 0xFF00_0000 != 0 {
            *d = s;
        }
    }
}

/// Expand `indices` through `palette` into `out` (`0xAARRGGBB` words), the CPU
/// twin of `PixelPlane::resolve`. `out` is resized to `indices.len()`.
pub fn expand_indexed(indices: &[u8], palette: &[u32], out: &mut Vec<u32>) {
    out.clear();
    out.reserve(indices.len());
    out.extend(indices.iter().map(|&i| palette[i as usize & 0xFF]));
}

/// **The pump's one pass.** Resolve `indices` through `palette`, composite
/// `overlay` on top, and write the result into `dst` — a `stride`-BYTE-pitched
/// BGRA buffer, which is what `MacvmRenderPixelPlane` hands back and
/// `MacvmRenderPixelStride` measures.
///
/// Fused deliberately: one pass over `w * h` words rather than an expand, a
/// composite and a memcpy over three. `stride` is honoured rather than assumed
/// to be `w * 4` — `MacvmRenderPixelStride`'s doc comment is explicit that a
/// caller assuming `w * 4` "would be right today and wrong the moment a plane
/// is padded".
///
/// # Safety
/// `dst` must point at `stride * h` writable bytes — exactly the plane
/// `MacvmRenderPixelPlane(hwnd, w, h)` just returned, re-asked this frame
/// because a resize reallocates it.
#[allow(unsafe_code)]
pub unsafe fn resolve_composite_into(
    dst: *mut u8,
    stride: usize,
    w: u32,
    h: u32,
    indices: &[u8],
    palette: &[u32],
    overlay: Option<&[u32]>,
) {
    let (wu, hu) = (w as usize, h as usize);
    for y in 0..hu {
        let row = dst.add(y * stride) as *mut u32;
        for x in 0..wu {
            let i = y * wu + x;
            let mut c = match indices.get(i) {
                Some(&ix) => palette[ix as usize & 0xFF],
                None => 0xFF00_0000,
            };
            if let Some(o) = overlay {
                if let Some(&s) = o.get(i) {
                    if s & 0xFF00_0000 != 0 {
                        c = s;
                    }
                }
            }
            row.add(x).write(c);
        }
    }
}

// ── the retained layer ──────────────────────────────────────────────────────

/// The always-topmost text layer, retained between frames.
///
/// **Retention is the contract, not an implementation accident.** Upstream's
/// `Present` never touches this buffer (`cocoa_gui/src/game.rs::present` only
/// uploads and composites it); the ONLY thing that erases it is
/// `GameCommand::TextClear`. `world/45a_life.mst::drawHud` depends on that in
/// both directions — it calls `pane textClear` before every HUD redraw, and its
/// own comment says the generation counter would otherwise "smear solid".
#[derive(Clone)]
pub struct TextOverlay {
    w: u32,
    h: u32,
    /// `w * h` words of `0xAARRGGBB`; [`TRANSPARENT`] where nothing was drawn.
    buffer: Vec<u32>,
    /// Whether anything has been drawn since the last [`clear`](Self::clear).
    /// The pump reads it to choose the cheap indexed upload (no ink) over the
    /// CPU-composited direct upload (ink).
    ink: bool,
}

impl TextOverlay {
    pub fn new(w: u32, h: u32) -> Self {
        TextOverlay {
            w,
            h,
            buffer: vec![TRANSPARENT; (w as usize) * (h as usize)],
            ink: false,
        }
    }

    pub fn width(&self) -> u32 {
        self.w
    }

    pub fn height(&self) -> u32 {
        self.h
    }

    /// The raw BGRA words, for [`composite_over`].
    pub fn pixels(&self) -> &[u32] {
        &self.buffer
    }

    /// Has anything been drawn since the last clear?
    pub fn has_ink(&self) -> bool {
        self.ink
    }

    /// `GameCommand::TextClear` (prim 255). The only thing that erases the
    /// layer — `Present` must NOT call this.
    pub fn clear(&mut self) {
        if self.ink {
            self.buffer.fill(TRANSPARENT);
            self.ink = false;
        }
    }

    /// Follow a `SetPaneSize`. Upstream rebuilds the whole pane and with it a
    /// fresh `TextOverlay::new`, so a resize DOES drop the text — matched here.
    /// A same-size call is a no-op and keeps the ink, as upstream's
    /// `SetPaneSize { w, h }` to the current size never reaches `ensure_pane`.
    pub fn resize(&mut self, w: u32, h: u32) {
        if w == self.w && h == self.h {
            return;
        }
        self.w = w;
        self.h = h;
        self.buffer = vec![TRANSPARENT; (w as usize) * (h as usize)];
        self.ink = false;
    }

    /// `GameCommand::Text` (prim 254).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(&mut self, x: i64, y: i64, text: &str, r: u8, g: u8, b: u8, scale: u32) {
        draw_text(&mut self.buffer, self.w, self.h, x, y, text, r, g, b, scale);
        self.ink = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 320;
    const H: u32 = 240;

    fn overlay() -> TextOverlay {
        TextOverlay::new(W, H)
    }

    /// `(r, g, b, a)` of one overlay pixel, unpacked from the `0xAARRGGBB` word
    /// so these assertions read like the Mac reference's.
    fn pixel(o: &TextOverlay, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let c = o.pixels()[(y * o.width() + x) as usize];
        (
            ((c >> 16) & 0xFF) as u8,
            ((c >> 8) & 0xFF) as u8,
            (c & 0xFF) as u8,
            ((c >> 24) & 0xFF) as u8,
        )
    }

    #[test]
    fn a_word_is_bgra_in_memory() {
        assert_eq!(
            bgra(0x12, 0x34, 0x56).to_le_bytes(),
            [0x56, 0x34, 0x12, 0xFF]
        );
    }

    #[test]
    fn clear_zeroes_the_whole_buffer() {
        let mut o = overlay();
        o.draw_text(0, 0, "0", 255, 0, 0, 1);
        assert!(o.has_ink());
        o.clear();
        assert_eq!(pixel(&o, 0, 0), (0, 0, 0, 0));
        assert!(!o.has_ink());
    }

    #[test]
    fn letter_a_lights_the_expected_pixels() {
        let mut o = overlay();
        o.draw_text(0, 0, "A", 255, 0, 0, 1);
        assert_eq!(pixel(&o, 0, 0).3, 0, "A: top-left corner is dark");
        assert_eq!(pixel(&o, 4, 0).3, 0, "A: top-right corner is dark");
        assert_eq!(pixel(&o, 2, 0), (255, 0, 0, 255), "A: top-center lit");
        assert_eq!(pixel(&o, 0, 3).3, 255, "A: crossbar left lit");
        assert_eq!(pixel(&o, 4, 3).3, 255, "A: crossbar right lit");
    }

    #[test]
    fn digit_zero_is_a_ring_with_a_hollow_interior() {
        let mut o = overlay();
        o.draw_text(0, 0, "0", 200, 100, 50, 1);
        assert_eq!(pixel(&o, 2, 0).3, 255, "0: top edge lit");
        assert_eq!(pixel(&o, 0, 1).3, 255, "0: left edge lit");
        assert_eq!(pixel(&o, 2, 1).3, 0, "0: interior is hollow");
    }

    #[test]
    fn space_leaves_its_cell_untouched() {
        let mut o = overlay();
        o.draw_text(0, 0, " ", 255, 255, 255, 1);
        for y in 0..GLYPH_H {
            for x in 0..GLYPH_ADVANCE {
                assert_eq!(pixel(&o, x, y).3, 0);
            }
        }
    }

    #[test]
    fn characters_advance_by_glyph_advance_pixels_at_scale_one() {
        let mut o = overlay();
        o.draw_text(0, 0, "AA", 255, 0, 0, 1);
        assert_eq!(GLYPH_ADVANCE, 6);
        assert_eq!(pixel(&o, GLYPH_ADVANCE + 2, 0).3, 255);
    }

    #[test]
    fn scale_blocks_each_font_pixel_into_a_square() {
        let mut o = overlay();
        o.draw_text(0, 0, "A", 0, 255, 0, 3);
        for yy in 0..3 {
            for xx in 6..9 {
                assert_eq!(pixel(&o, xx, yy), (0, 255, 0, 255));
            }
        }
        for yy in 0..3 {
            for xx in 0..3 {
                assert_eq!(pixel(&o, xx, yy).3, 0);
            }
        }
    }

    #[test]
    fn unknown_character_renders_the_hollow_placeholder_box() {
        let mut o = overlay();
        o.draw_text(0, 0, "%", 255, 255, 255, 1);
        assert_eq!(pixel(&o, 0, 0).3, 255, "placeholder: top-left lit");
        assert_eq!(pixel(&o, 4, 0).3, 255, "placeholder: top-right lit");
        assert_eq!(pixel(&o, 2, 1).3, 0, "placeholder: hollow interior");
    }

    #[test]
    fn lowercase_folds_to_the_uppercase_glyph() {
        let mut upper = overlay();
        let mut lower = overlay();
        upper.draw_text(0, 0, "A", 255, 255, 255, 1);
        lower.draw_text(0, 0, "a", 255, 255, 255, 1);
        assert_eq!(upper.pixels(), lower.pixels(), "'a' should render as 'A'");
    }

    #[test]
    fn out_of_bounds_is_dropped_not_wrapped() {
        let mut o = overlay();
        o.draw_text(-3, 0, "H", 255, 255, 255, 1);
        for y in 0..GLYPH_H {
            assert_eq!(pixel(&o, W - 1, y).3, 0, "no wrap onto the right edge");
        }
        let mut o2 = overlay();
        o2.draw_text(0, H as i64 + 10, "HELLO", 255, 255, 255, 1);
        assert!(o2.pixels().iter().all(|&p| p == TRANSPARENT));
    }

    #[test]
    fn the_overlay_is_retained_until_text_clear() {
        let mut o = overlay();
        o.draw_text(4, 4, "GEN 1", 235, 240, 255, 1);
        let after_first: Vec<u32> = o.pixels().to_vec();
        o.draw_text(4, 4, "GEN 2", 235, 240, 255, 1);
        assert_ne!(o.pixels(), after_first.as_slice(), "second draw accumulated");
        for (i, &p) in after_first.iter().enumerate() {
            if p != TRANSPARENT {
                assert_eq!(o.pixels()[i], p, "retained pixel survived the redraw");
            }
        }
        o.clear();
        assert!(o.pixels().iter().all(|&p| p == TRANSPARENT));
    }

    #[test]
    fn composite_replaces_only_where_the_overlay_has_ink() {
        let mut o = overlay();
        o.draw_text(0, 0, "A", 255, 0, 0, 1);
        let mut dst = vec![bgra(0, 0, 40); (W * H) as usize];
        composite_over(&mut dst, o.pixels());
        assert_eq!(dst[2], bgra(255, 0, 0), "lit font pixel wins");
        assert_eq!(dst[0], bgra(0, 0, 40), "dark font pixel keeps the pane");
    }

    #[test]
    fn expand_indexed_matches_the_renderers_resolve() {
        let mut palette = vec![0xFF00_0000u32; 256];
        palette[7] = bgra(10, 20, 30);
        let indices = [0u8, 7, 7, 0];
        let mut out = Vec::new();
        expand_indexed(&indices, &palette, &mut out);
        assert_eq!(
            out,
            vec![0xFF00_0000, bgra(10, 20, 30), bgra(10, 20, 30), 0xFF00_0000]
        );
    }

    #[test]
    fn every_punctuation_arm_has_a_real_glyph_not_the_placeholder() {
        for ch in ". , : - ' ! ? ( ) / + < > &".chars().filter(|c| *c != ' ') {
            assert_ne!(
                glyph_for(ch),
                PLACEHOLDER_GLYPH,
                "{ch:?} should have a dedicated glyph"
            );
        }
        assert_eq!(glyph_for(' '), [0; 7], "space is blank");
        assert_eq!(glyph_for('%'), PLACEHOLDER_GLYPH);
        assert_eq!(glyph_for('9'), DIGIT_GLYPHS[9]);
        assert_eq!(glyph_for('z'), LETTER_GLYPHS[25]);
    }

    /// The fused pass the pump uses: same answer as expand + composite, and it
    /// honours a PADDED stride rather than assuming `w * 4`.
    #[test]
    fn resolve_composite_into_honours_a_padded_stride() {
        const PW: u32 = 8;
        const PH: u32 = 4;
        let mut palette = vec![0xFF00_0000u32; 256];
        palette[16] = bgra(0, 0, 40);
        palette[27] = bgra(0, 200, 0);
        let mut indices = vec![16u8; (PW * PH) as usize];
        indices[9] = 27;
        let mut o = TextOverlay::new(PW, PH);
        o.draw_text(0, 0, ".", 255, 255, 255, 1); // '.' lights (2,5),(3,5),(2,6),(3,6)

        let stride = (PW as usize + 3) * 4; // deliberately padded
        let mut plane = vec![0u8; stride * PH as usize];
        unsafe {
            resolve_composite_into(
                plane.as_mut_ptr(),
                stride,
                PW,
                PH,
                &indices,
                &palette,
                Some(o.pixels()),
            );
        }
        let word = |x: usize, y: usize| -> u32 {
            let o = y * stride + x * 4;
            u32::from_le_bytes([plane[o], plane[o + 1], plane[o + 2], plane[o + 3]])
        };
        assert_eq!(word(0, 0), bgra(0, 0, 40), "background index resolved");
        assert_eq!(word(1, 1), bgra(0, 200, 0), "index 27 resolved");
        // The padding columns are never written.
        assert_eq!(&plane[stride - 12..stride], &[0u8; 12]);
        // With no overlay the result is the plain resolve.
        let mut bare = vec![0u8; stride * PH as usize];
        unsafe {
            resolve_composite_into(
                bare.as_mut_ptr(),
                stride,
                PW,
                PH,
                &indices,
                &palette,
                None,
            );
        }
        let mut expanded = Vec::new();
        expand_indexed(&indices, &palette, &mut expanded);
        for y in 0..PH as usize {
            for x in 0..PW as usize {
                let o = y * stride + x * 4;
                let got = u32::from_le_bytes([bare[o], bare[o + 1], bare[o + 2], bare[o + 3]]);
                assert_eq!(got, expanded[y * PW as usize + x]);
            }
        }
    }

    /// Life's own HUD, at Life's own coordinates, on Life's own pane size.
    #[test]
    fn lifes_hud_fits_the_pane_and_lands_where_life_asks() {
        let mut o = overlay();
        o.draw_text(4, 4, "GEN 0   POP 10   RUNNING", 235, 240, 255, 1);
        o.draw_text(
            4,
            228,
            "SPACE pause   LEFT draw   RIGHT erase   ESC quit",
            150,
            165,
            200,
            1,
        );
        // 48 chars * 6 px = 288, from x = 4 -> last column 291 < 320.
        let legend = "SPACE pause   LEFT draw   RIGHT erase   ESC quit";
        let legend_end = 4 + legend.chars().count() as u32 * GLYPH_ADVANCE;
        assert!(legend_end <= W, "legend spans {legend_end} px of {W}");
        // Bottom line's last row is 228 + 6 = 234 < 240.
        assert!(228 + GLYPH_H <= o.height());
        // 'S' of SPACE: glyph [0x0E,...] row 0 = ".###." -> (4+1, 228) lit.
        assert_eq!(pixel(&o, 5, 228), (150, 165, 200, 255));
        assert!(o.has_ink());
    }
}
