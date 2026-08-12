//! WINARM (WG6d) — the Editor pane's renderer: a cell grid on DirectWrite.
//!
//! `docs/sprints/sprint_wg6d_detail.md` is the design. The short form: the
//! guest owns MEANING (rope, tokenizer, caret, selection) and writes
//! codepoint+colour cells into a buffer this crate owns; this crate owns
//! PIXELS (one DirectWrite font face, metrics from the font's design tables,
//! Direct2D glyph runs onto a DXGI flip-model swapchain). The guest never
//! computes a pixel; the renderer never sees a document.
//!
//! The GDI pane put two authorities in charge of the same pixels — Smalltalk
//! multiplying `column * charWidth` while GDI laid glyphs out its own way —
//! and shipped four defects in two days because of it. This split does not
//! fix those defects; it makes them unrepresentable.
//!
//! # Everything here is testable with no window
//!
//! Which the GDI path structurally was not. Metrics come from the font's
//! design tables rather than from `DT_CALCRECT` inside a `WM_PAINT`, and the
//! grid draws into any `ID2D1RenderTarget` — a WIC bitmap in the tests, a
//! swapchain back buffer in the app. `grid_renders_ink_where_the_cells_say`
//! asserts actual pixels without opening anything.

#![cfg(windows)]

pub mod gpu;
pub mod pane;

use std::mem::ManuallyDrop;

use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1RenderTarget, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontFace, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_GLYPH_RUN, DWRITE_MEASURING_MODE_NATURAL,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

/// One cell, and the whole of what crosses the seam.
///
/// 12 bytes, little-endian, `cols * rows` of them. `fg`/`bg` are COLORREF —
/// `0x00BBGGRR`, the byte order `WinSyntax`'s palette already speaks — so the
/// guest's existing colour table needs no conversion on the way in.
///
/// A per-cell BACKGROUND is what lets the renderer stay ignorant on purpose:
/// the selection highlight, a find highlight and the ghost line are all just
/// cells the guest coloured differently. None of those concepts crosses here.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub cp: u32,
    pub fg: u32,
    pub bg: u32,
}

/// A background that means **do not fill** — leave whatever is underneath.
///
/// WG8/SM0. Without it the two planes cannot compose: `draw_grid` fills every
/// cell's background, so a pixel plane under a full-width grid is covered
/// everywhere except the margin the grid does not reach. That is exactly what
/// the first plasma frame looked like — bands of colour down the right edge
/// and along the bottom, and white everywhere the grid was.
///
/// Chosen ABOVE the COLORREF range (which is `0x00BBGGRR`, top byte zero) so
/// it cannot collide with a colour anyone means. A cell with this background
/// still draws its GLYPH — which is what makes a HUD over live pixels one
/// value rather than a mode.
pub const BG_TRANSPARENT: u32 = 0x0100_0000;

impl Cell {
    pub fn blank() -> Self {
        Cell {
            cp: ' ' as u32,
            fg: 0x0000_0000,
            bg: 0x00FF_FFFF,
        }
    }
}

/// The face and the numbers the whole grid is built from, in PHYSICAL pixels.
///
/// From the font's DESIGN TABLES — `GetDesignGlyphMetrics` for the advance,
/// `IDWriteFontFace::GetMetrics` for ascent/descent/lineGap — never from
/// drawing anything. That is what makes them exist before any pane does, and
/// it retires the defect where `editorCharWidth` answered a default 8 until
/// the first paint measured the real 7, so a click encoded before that
/// transition and decoded after it landed at the end of the document.
pub struct CellMetrics {
    pub cell_w: u32,
    pub cell_h: u32,
    /// Baseline offset within the cell; glyph origins sit here, not at the top.
    pub ascent: f32,
    /// Em size in physical pixels, for `DWRITE_GLYPH_RUN::fontEmSize`.
    pub em_px: f32,
}

/// Cascadia Mono's face plus its cell metrics, or a String naming what failed.
///
/// Cascadia Mono and not Cascadia Code, for §2.1's own reason: a code grid
/// deliberately has no ligatures — columns should be columns.
pub fn mono_face_metrics(pt: f32, dpi: f32) -> Result<(IDWriteFontFace, CellMetrics), String> {
    unsafe {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
            .map_err(|e| format!("DWriteCreateFactory: {e}"))?;
        let fonts = {
            let mut c = None;
            factory
                .GetSystemFontCollection(&mut c, false)
                .map_err(|e| format!("GetSystemFontCollection: {e}"))?;
            c.ok_or("GetSystemFontCollection answered no collection")?
        };
        let mut index = 0u32;
        let mut exists = windows::core::BOOL::default();
        fonts
            .FindFamilyName(w!("Cascadia Mono"), &mut index, &mut exists)
            .map_err(|e| format!("FindFamilyName: {e}"))?;
        if !exists.as_bool() {
            return Err("Cascadia Mono is not installed".into());
        }
        let family = fonts
            .GetFontFamily(index)
            .map_err(|e| format!("GetFontFamily: {e}"))?;
        let font = family
            .GetFirstMatchingFont(
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
            .map_err(|e| format!("GetFirstMatchingFont: {e}"))?;
        let face = font
            .CreateFontFace()
            .map_err(|e| format!("CreateFontFace: {e}"))?;

        let mut fm = Default::default();
        face.GetMetrics(&mut fm);
        let em_px = pt / 72.0 * dpi;
        let scale = em_px / fm.designUnitsPerEm as f32;

        // GetDesignGlyphMetrics wants a GLYPH INDEX. Feeding it the codepoint
        // compiles and quietly answers the metrics of whatever glyph 77 is,
        // which for most faces is wrong in a way nothing would report.
        let cp = ['M' as u32];
        let mut gi = [0u16];
        face.GetGlyphIndices(cp.as_ptr(), 1, gi.as_mut_ptr())
            .map_err(|e| format!("GetGlyphIndices: {e}"))?;
        if gi[0] == 0 {
            return Err("Cascadia Mono has no glyph for 'M', which is absurd".into());
        }
        let mut gm = [Default::default(); 1];
        face.GetDesignGlyphMetrics(gi.as_ptr(), 1, gm.as_mut_ptr(), false)
            .map_err(|e| format!("GetDesignGlyphMetrics: {e}"))?;

        let cell_w = (gm[0].advanceWidth as f32 * scale).ceil() as u32;
        let cell_h =
            ((fm.ascent as i32 + fm.descent as i32 + fm.lineGap as i32) as f32 * scale).ceil() as u32;
        if cell_w == 0 || cell_h == 0 {
            return Err(format!("degenerate cell {cell_w}x{cell_h}"));
        }
        Ok((
            face,
            CellMetrics {
                cell_w,
                cell_h,
                ascent: fm.ascent as f32 * scale,
                em_px,
            },
        ))
    }
}

pub(crate) fn colorref_to_d2d(c: u32) -> D2D1_COLOR_F {
    // COLORREF is 0x00BBGGRR — blue high, which is the packing WG5c's own
    // `colorRefPacksBlueHigh` test pins on the guest side.
    D2D1_COLOR_F {
        r: (c & 0xFF) as f32 / 255.0,
        g: ((c >> 8) & 0xFF) as f32 / 255.0,
        b: ((c >> 16) & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

/// Draw the whole grid. **The entire renderer is this function** — everything
/// else is plumbing that decides which target it draws into.
///
/// Two passes per row, both batched by colour: backgrounds as filled
/// rectangles, then glyphs as runs with EXPLICIT per-cell advances. The
/// advances are what make this a grid rather than a string laid out by the
/// font: every glyph origin is `col * cell_w` by construction, so no amount
/// of per-glyph rounding can accumulate and no second layout engine exists to
/// disagree with.
///
/// A blank cell (space) contributes no glyph, so an empty document costs
/// nothing but the clear.
#[allow(clippy::too_many_arguments)]
pub fn draw_grid(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    face: &IDWriteFontFace,
    m: &CellMetrics,
    cols: u32,
    rows: u32,
    cells: &[Cell],
    caret: Option<(u32, u32)>,
) -> Result<(), String> {
    if cells.len() < (cols as usize) * (rows as usize) {
        return Err(format!(
            "grid is {cols}x{rows} but only {} cells were given",
            cells.len()
        ));
    }
    unsafe {
        // ALIASED, deliberately: cell edges are pixel boundaries, and
        // antialiasing a rectangle that lands exactly on one only blurs it.
        rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);

        let cw = m.cell_w as f32;
        let ch = m.cell_h as f32;

        for row in 0..rows {
            let base = (row * cols) as usize;
            let y = row as f32 * ch;

            // Pass 1: background runs.
            let mut c = 0u32;
            while c < cols {
                let bg = cells[base + c as usize].bg;
                let start = c;
                while c < cols && cells[base + c as usize].bg == bg {
                    c += 1;
                }
                if bg == BG_TRANSPARENT {
                    continue;
                }
                brush.SetColor(&colorref_to_d2d(bg));
                rt.FillRectangle(
                    &D2D_RECT_F {
                        left: start as f32 * cw,
                        top: y,
                        right: c as f32 * cw,
                        bottom: y + ch,
                    },
                    brush,
                );
            }

            // Pass 2: glyph runs, batched by foreground colour.
            let mut c = 0u32;
            while c < cols {
                let fg = cells[base + c as usize].fg;
                let start = c;
                let mut cps: Vec<u32> = Vec::new();
                while c < cols && cells[base + c as usize].fg == fg {
                    cps.push(cells[base + c as usize].cp);
                    c += 1;
                }
                // Trailing blanks carry no ink; drop them so a mostly-empty
                // row costs one short run instead of one full-width one.
                while cps.last() == Some(&(' ' as u32)) {
                    cps.pop();
                }
                if cps.is_empty() {
                    continue;
                }
                let n = cps.len();
                let mut indices = vec![0u16; n];
                face.GetGlyphIndices(cps.as_ptr(), n as u32, indices.as_mut_ptr())
                    .map_err(|e| format!("GetGlyphIndices: {e}"))?;
                let advances = vec![cw; n];

                brush.SetColor(&colorref_to_d2d(fg));
                let mut run = DWRITE_GLYPH_RUN {
                    fontFace: ManuallyDrop::new(Some(face.clone())),
                    fontEmSize: m.em_px,
                    glyphCount: n as u32,
                    glyphIndices: indices.as_ptr(),
                    glyphAdvances: advances.as_ptr(),
                    glyphOffsets: std::ptr::null(),
                    isSideways: false.into(),
                    bidiLevel: 0,
                };
                rt.DrawGlyphRun(
                    windows_numerics::Vector2 {
                        X: start as f32 * cw,
                        Y: y + m.ascent,
                    },
                    &run,
                    // The BRUSH. `ID2D1RenderTarget` has a four-argument
                    // overload without the glyph-run description, so passing
                    // `None` here compiles — `Option` satisfies
                    // `Param<ID2D1Brush>` — and hands D2D a null brush, which
                    // is an access violation inside the driver rather than an
                    // error anyone can read.
                    brush,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                // The clone above addref'd the face; give it back. Letting
                // ManuallyDrop leak would hold the font alive forever, and
                // dropping the struct without this would not release it.
                let _ = ManuallyDrop::take(&mut run.fontFace);
            }
        }

        // The caret, which is a CELL and not a thread-global Win32 resource.
        // That is the whole of what retires the "two cursors" pitfall: there
        // is nothing to create, nothing to destroy, and nothing to reconcile
        // when focus moves.
        if let Some((col, row)) = caret {
            if col < cols && row < rows {
                brush.SetColor(&colorref_to_d2d(0x0000_0000));
                rt.FillRectangle(
                    &D2D_RECT_F {
                        left: col as f32 * cw,
                        top: row as f32 * ch,
                        right: col as f32 * cw + 2.0,
                        bottom: (row + 1) as f32 * ch,
                    },
                    brush,
                );
            }
        }
    }
    Ok(())
}

/// A D2D factory, shared by the app path and the tests.
pub fn d2d_factory() -> Result<ID2D1Factory, String> {
    unsafe {
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .map_err(|e| format!("D2D1CreateFactory: {e}"))
    }
}

/// Render-target properties with DPI PINNED TO 96, so a DIP is a pixel.
///
/// Deliberate: every number in the cell grid is already in physical pixels
/// (the metrics are ceil'd there), and letting D2D apply a second DPI scale
/// on top would be exactly the two-authorities mistake this design exists to
/// remove — in a place where it would be much harder to see.
pub fn rt_props() -> D2D1_RENDER_TARGET_PROPERTIES {
    D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: Default::default(),
    }
}

/// Unused today; present so the hwnd path's rect handling has one home.
pub fn client_size(hwnd: HWND) -> Result<(u32, u32), String> {
    let mut r = RECT::default();
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut r)
            .map_err(|e| format!("GetClientRect: {e}"))?;
    }
    Ok(((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
        WICBitmapCacheOnLoad, WICBitmapLockRead,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };

    /// WG6d-1's opening claim, proven the way GDI never allowed: the font's
    /// metrics, HEADLESS — no window, no DC, no paint. If this passes, the
    /// grid's numbers exist before any pane does, and nothing about them can
    /// depend on when the first WM_PAINT arrived.
    /// SM4's claim, checked without a window: a palette write re-colours pixels
    /// the guest never touched again.
    ///
    /// The assertion that matters is the SECOND one. Writing indices then
    /// resolving proves only that a lookup table works; writing the SAME
    /// indices, changing one palette slot, and resolving again is the actual
    /// design — the screen changes because 4 bytes changed, and no pixel was
    /// stored twice. If `resolve` ever cached, or the palette were copied
    /// rather than shared, this is the line that would fail.
    #[test]
    fn a_palette_write_recolours_pixels_the_guest_never_touched() {
        let mut p = PixelPlane::new(4, 2);
        p.make_indexed();
        assert_eq!(p.index_stride(), 4, "one byte per pixel, no padding");
        // Slot 1 across the top row, slot 2 across the bottom. Written ONCE.
        for x in 0..4 {
            p.indices[x] = 1;
            p.indices[4 + x] = 2;
        }
        p.palette[1] = 0xFFFF_0000; // red
        p.palette[2] = 0xFF00_00FF; // blue
        p.resolve();
        // A free function, not a closure: the closure would hold `p` borrowed
        // across the palette write below, which is the very thing under test.
        fn px(p: &PixelPlane, x: usize, y: usize) -> u32 {
            let o = y * p.stride as usize + x * 4;
            u32::from_le_bytes([p.bytes[o], p.bytes[o + 1], p.bytes[o + 2], p.bytes[o + 3]])
        }
        assert_eq!(px(&p, 0, 0), 0xFFFF_0000);
        assert_eq!(px(&p, 3, 1), 0xFF00_00FF);

        // ONE palette store, no index store at all — and the whole top row
        // turns green. This is the frame a cycling demo actually costs.
        p.palette[1] = 0xFF00_FF00;
        p.resolve();
        assert_eq!(px(&p, 0, 0), 0xFF00_FF00, "the top row follows its palette slot");
        assert_eq!(px(&p, 2, 0), 0xFF00_FF00);
        assert_eq!(px(&p, 3, 1), 0xFF00_00FF, "and the bottom row did not move");

        // An unwritten palette is BLACK and opaque, not whatever was in the
        // allocator: an indexed plane with no palette yet must look empty
        // rather than look like a bug that happens to be colourful.
        let mut q = PixelPlane::new(2, 1);
        q.make_indexed();
        q.resolve();
        assert_eq!(
            u32::from_le_bytes([q.bytes[0], q.bytes[1], q.bytes[2], q.bytes[3]]),
            0xFF00_0000
        );
    }

    #[test]
    fn metrics_exist_with_no_window_and_scale_with_dpi() {
        let (face, m96) = mono_face_metrics(9.0, 96.0).expect("Cascadia Mono at 96dpi");
        assert!(m96.cell_w > 0 && m96.cell_h > 0);
        assert!(
            m96.cell_h > m96.cell_w,
            "a text cell is taller than wide; {}x{} says the axes are swapped",
            m96.cell_w,
            m96.cell_h
        );
        unsafe {
            let cps = ['M' as u32, 'i' as u32];
            let mut gis = [0u16; 2];
            face.GetGlyphIndices(cps.as_ptr(), 2, gis.as_mut_ptr()).unwrap();
            let mut gms = [Default::default(); 2];
            face.GetDesignGlyphMetrics(gis.as_ptr(), 2, gms.as_mut_ptr(), false)
                .unwrap();
            assert_eq!(
                gms[0].advanceWidth, gms[1].advanceWidth,
                "Cascadia Mono must be monospaced"
            );
        }
        let (_f, m144) = mono_face_metrics(9.0, 144.0).expect("Cascadia Mono at 144dpi");
        assert!(m144.cell_w > m96.cell_w);
        assert!(m144.cell_h > m96.cell_h);
    }

    /// Render a known grid into a WIC bitmap and READ THE PIXELS.
    ///
    /// This is the assertion the GDI pane could never carry, and its absence
    /// is exactly why four rendering defects shipped: every gate could see
    /// state (`paintCalls`, `paintError`, a caret offset) and none could see
    /// INK. Here a cell with 'M' has ink, a blank cell has none, a
    /// red-background cell is red, and — the one that would have caught the
    /// reported smearing — the ink for column 1 lies inside column 1's
    /// pixels and nowhere near column 0's.
    #[test]
    fn grid_renders_ink_where_the_cells_say() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let (face, m) = mono_face_metrics(9.0, 96.0).expect("face");
        let (cols, rows) = (4u32, 1u32);
        let (w, h) = (cols * m.cell_w, rows * m.cell_h);

        // cell 0: blank on white. cell 1: 'M' black on white.
        // cell 2: blank on RED. cell 3: blank on white.
        let mut cells = vec![Cell::blank(); (cols * rows) as usize];
        cells[1].cp = 'M' as u32;
        cells[2].bg = 0x0000_00FF; // COLORREF: red

        let factory = d2d_factory().expect("d2d factory");
        let wic: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                .expect("WIC factory");
        let bitmap = unsafe {
            wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)
        }
        .expect("wic bitmap");
        let rt = unsafe { factory.CreateWicBitmapRenderTarget(&bitmap, &rt_props()) }
            .expect("wic render target");
        let brush = unsafe {
            rt.CreateSolidColorBrush(&colorref_to_d2d(0x0000_0000), None)
                .expect("brush")
        };

        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&colorref_to_d2d(0x00FF_FFFF)));
        }
        draw_grid(&rt, &brush, &face, &m, cols, rows, &cells, None).expect("draw");
        unsafe { rt.EndDraw(None, None).expect("EndDraw") };

        // Read it back. BGRA, premultiplied, `stride` bytes per row.
        let lock = unsafe {
            bitmap.Lock(
                &windows::Win32::Graphics::Imaging::WICRect {
                    X: 0,
                    Y: 0,
                    Width: w as i32,
                    Height: h as i32,
                },
                WICBitmapLockRead.0 as u32,
            )
        }
        .expect("lock");
        let stride = unsafe { lock.GetStride() }.expect("stride") as usize;
        let mut ptr = std::ptr::null_mut();
        let mut size = 0u32;
        unsafe { lock.GetDataPointer(&mut size, &mut ptr) }.expect("data");
        let px = unsafe { std::slice::from_raw_parts(ptr, size as usize) };
        let at = |x: u32, y: u32| -> (u8, u8, u8) {
            let o = y as usize * stride + x as usize * 4;
            (px[o + 2], px[o + 1], px[o]) // r, g, b
        };
        let cell_has_ink = |col: u32| -> bool {
            let x0 = col * m.cell_w;
            (x0..x0 + m.cell_w).any(|x| (0..h).any(|y| at(x, y) != (255, 255, 255)))
        };

        assert!(!cell_has_ink(0), "a blank cell must be blank");
        assert!(cell_has_ink(1), "the 'M' cell must have ink");
        assert!(
            cell_has_ink(3) == false,
            "the trailing blank cell must be blank"
        );

        // The red background landed in cell 2 and ONLY cell 2 — a background
        // run that bled by one cell is the shape the selection highlight
        // would fail in.
        let x2 = 2 * m.cell_w + m.cell_w / 2;
        assert_eq!(at(x2, h / 2), (255, 0, 0), "cell 2's background is red");
        let x1 = 1 * m.cell_w + m.cell_w / 2;
        assert_ne!(at(x1, 0), (255, 0, 0), "and it did not bleed into cell 1");

        // THE ONE THAT MATTERS. The glyph in column 1 must be inside column
        // 1. The reported defect was glyphs drifting out of their cells and
        // overlapping their neighbours; asserted here as ink containment.
        let col0_range = 0..m.cell_w;
        assert!(
            !col0_range.clone().any(|x| (0..h).any(|y| at(x, y) != (255, 255, 255))),
            "column 0 is blank, so ANY ink there is column 1's glyph having escaped its cell"
        );
    }
}

// ── WG8 / SM0: the PIXEL plane ──────────────────────────────────────────────

/// One pane's pixel plane: BGRA bytes the GUEST writes and the renderer draws.
///
/// **The Windows shape of upstream's SM0**, and the one difference is worth
/// stating precisely, because the imprecise version blames the wrong thing.
///
/// It is an API difference, NOT a silicon one. Snapdragon X is a unified-memory
/// part exactly as Apple silicon is: one LPDDR5x pool on package, Adreno on
/// die, no discrete VRAM and no bus to cross — D3D12 reports both `UMA` and
/// `CacheCoherentUMA` TRUE on it. What Metal has that D3D11 lacks is the API
/// that ADMITS this: `MTLStorageModeShared` hands out a persistent CPU pointer
/// whose bytes the GPU samples. D3D11's texture contract is `Map`/`Unmap` with
/// `WRITE_DISCARD`, and the pointer may not be held across frames — so it
/// cannot be given to the guest, whatever the memory underneath is doing.
///
/// So this is ONE copy per presented frame AT THE CONTRACT LEVEL — a
/// `CopyFromMemory` into a D2D bitmap — where the Mac has none. On a UMA part
/// the driver is likely renaming a buffer rather than physically staging it,
/// which makes that count pessimistic in practice; it is quoted as one copy
/// because one copy is what the API guarantees, and code should be built
/// against the guarantee.
///
/// **The gap is closable and the route is known**: D3D12 on a UMA adapter
/// permits a `CUSTOM` heap (CPU write-combine, GPU readable) mapped once and
/// never unmapped, which is the Metal shared-storage shape exactly. That is a
/// device change under this same seam, not a redesign — the guest-facing
/// contract here is already "ask for an address and a stride".
///
/// Even at one copy it deletes what SM0's design actually indicts:
///
/// * the marshalling (`ByteArray` → `copy_bytes_out` → a fresh `Vec`),
/// * the per-frame ALLOCATION,
/// * the hop through a `Mutex<VecDeque>` to another thread,
///
/// leaving only the upload the GPU needs anyway. Three copies and an
/// allocation become one copy and none.
pub struct PixelPlane {
    pub w: u32,
    pub h: u32,
    /// `w * 4`, BGRA. Handed to the guest rather than assumed BY it — see the
    /// standing caution in the WG8 review: a pixel plane is the one place the
    /// guest computes an address, so the SHAPE must come from this side.
    pub stride: u32,
    pub bytes: Vec<u8>,
    /// **SM4: the palette as memory.** When present, `bytes` is not what the
    /// guest writes — `indices` is, one byte per pixel, and each byte names a
    /// slot in `palette`. `resolve` expands the two into `bytes` before a draw.
    ///
    /// The point is not compression, it is the COST OF A FRAME. Cycling a
    /// palette re-colours every pixel on screen for 256 stores; the direct BGRA
    /// path needs `w * h`. At 160x120 that is 256 words against 19,200 — the
    /// difference between an effect a Smalltalk loop can drive at frame rate
    /// and one it cannot.
    pub indexed: bool,
    pub indices: Vec<u8>,
    /// BGRA words. **Two layouts**, and `copper` says which.
    ///
    /// FLAT (`copper == false`): 256 entries, index 0 is an ordinary colour.
    /// This is WG8/SM4's own model and what `WinPixels` writes.
    ///
    /// COPPER (`copper == true`): upstream's model, `h * 16 + 240` entries —
    /// `h` groups of 16 PER-SCANLINE colours first, then 240 globals. Index 0
    /// is TRANSPARENT and never assignable; 1..15 are per-line; 16..255 are
    /// global. That is what lets `45f_copper.mst` fill the screen once and
    /// then change only what colour index 1 MEANS on each of 240 scanlines.
    pub palette: Vec<u32>,
    /// Upstream's per-scanline palette semantics, and index 0 as a hole.
    pub copper: bool,
    /// Viewport pan within a world buffer bigger than the viewport
    /// (`overscan:`/`scrollTo:`). Zero for a plane the size of its viewport.
    pub scroll_x: u32,
    pub scroll_y: u32,
}

impl PixelPlane {
    /// The palette length this plane's layout implies. Answered to the guest
    /// rather than assumed by it, exactly as `stride` is — a copper plane's
    /// table is a function of its HEIGHT, so a guest that hard-coded 256 would
    /// write globals into scanline 16's per-line entries.
    pub fn palette_len(&self) -> usize {
        if self.copper {
            self.h as usize * 16 + 240
        } else {
            PALETTE_LEN as usize
        }
    }

    /// Switch this plane to upstream's copper layout, resizing the table.
    pub fn make_copper(&mut self) {
        if self.copper {
            return;
        }
        self.copper = true;
        self.palette = vec![0xFF00_0000; self.palette_len()];
    }

    /// The table index a `(screen_row, colour_index)` pair resolves to — the
    /// shader's arithmetic, in Rust, so a host writing the table and a shader
    /// reading it cannot disagree about where a colour lives.
    pub fn palette_slot(&self, screen_row: u32, index: u8) -> usize {
        if !self.copper {
            return index as usize;
        }
        if index < 16 {
            screen_row as usize * 16 + index as usize
        } else {
            self.h as usize * 16 + (index as usize - 16)
        }
    }
}

/// Slots in a plane's palette. Answered to the guest, never assumed by it.
pub const PALETTE_LEN: u32 = 256;

impl PixelPlane {
    pub fn new(w: u32, h: u32) -> Self {
        let stride = w * 4;
        PixelPlane {
            w,
            h,
            stride,
            bytes: vec![0u8; (stride as usize) * (h as usize)],
            indexed: false,
            indices: Vec::new(),
            copper: false,
            scroll_x: 0,
            scroll_y: 0,
            // A palette that starts BLACK, not undefined. An indexed plane
            // whose palette was never written should be a black rectangle —
            // visibly empty — rather than whatever the allocator left behind.
            palette: vec![0xFF00_0000; PALETTE_LEN as usize],
        }
    }

    /// Turn this plane indexed, allocating the index buffer. Idempotent.
    pub fn make_indexed(&mut self) {
        if !self.indexed {
            self.indices = vec![0u8; (self.w as usize) * (self.h as usize)];
            self.indexed = true;
        }
    }

    /// The index buffer's stride. `w` today — handed out anyway, because the
    /// guest asking is the discipline, not the number being interesting.
    pub fn index_stride(&self) -> u32 {
        self.w
    }

    /// Expand `indices` through `palette` into `bytes`.
    ///
    /// Runs on the RENDERER's side of the seam, once per presented frame, so a
    /// guest that cycles its palette pays 256 stores and this pass — never
    /// `w * h` stores of its own. A plane that is not indexed is already in its
    /// final form and this does nothing.
    pub fn resolve(&mut self) {
        if !self.indexed {
            return;
        }
        for y in 0..self.h as usize {
            let src = y * self.w as usize;
            let dst = y * self.stride as usize;
            for x in 0..self.w as usize {
                // `& 0xFF` is free here (u8 index into a 256-entry table) and
                // is what makes the palette length a fact rather than a
                // promise: there is no index the guest can write that reads
                // off the end.
                let c = self.palette[self.indices[src + x] as usize];
                self.bytes[dst + x * 4..dst + x * 4 + 4].copy_from_slice(&c.to_le_bytes());
            }
        }
    }
}

/// Draw a pixel plane to fill `dest`, nearest-neighbour.
///
/// NEAREST, not linear, and it is a correctness choice rather than a taste
/// one: a plane is pixels the guest computed, and interpolating them invents
/// values it did not write. A Julia set at 320x240 blown up to a pane should
/// look like big pixels, because that is what it is.
pub fn draw_pixel_plane(
    rt: &ID2D1RenderTarget,
    plane: &PixelPlane,
    dest_w: f32,
    dest_h: f32,
) -> Result<(), String> {
    unsafe {
        let props = windows::Win32::Graphics::Direct2D::D2D1_BITMAP_PROPERTIES {
            pixelFormat: rt_props().pixelFormat,
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let bmp = rt
            .CreateBitmap(
                windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U {
                    width: plane.w,
                    height: plane.h,
                },
                Some(plane.bytes.as_ptr() as *const _),
                plane.stride,
                &props,
            )
            .map_err(|e| format!("CreateBitmap: {e}"))?;
        // `ID2D1RenderTarget`'s own DrawBitmap: FIVE arguments (no perspective
        // transform) and no Result. The six-argument one belongs to
        // `ID2D1DeviceContext`, and picking the wrong overload is how the
        // caret's brush went missing in WG6d-1 — so the arity is named here.
        rt.DrawBitmap(
            &bmp,
            Some(&D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: dest_w,
                bottom: dest_h,
            }),
            1.0,
            windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            None,
        );
    }
    Ok(())
}
