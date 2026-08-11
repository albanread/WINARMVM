//! WINARM (WG6d) — the Editor pane's renderer: a cell grid on DirectWrite.
//!
//! `docs/sprints/sprint_wg6d_detail.md` is the design. The short form: the
//! guest owns MEANING (rope, tokenizer, caret, selection) and writes
//! codepoint+colour cells into a buffer this crate owns; this crate owns
//! PIXELS (one DirectWrite font face, metrics from the font's design tables,
//! Direct2D glyph runs onto a DXGI flip-model swapchain). The guest never
//! computes a pixel; the renderer never sees a document. The GDI pane put two
//! authorities in charge of the same pixels and shipped four defects in two
//! days because of it — this split is the fix for the class.
//!
//! This file is WG6d-1's first step: the DirectWrite half only — factory,
//! font face, and metrics from design tables — proven HEADLESSLY, which the
//! GDI path could never be. The device/swapchain/present half lands next,
//! gated by a WIC-bitmap render test (no window there either).

#![cfg(windows)]

use windows::core::w;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontFace, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
};

/// The face and the two numbers the whole grid is built from.
///
/// `cell_w`/`cell_h` are PHYSICAL pixels for one cell at (`pt`, `dpi`),
/// ceil'd so a glyph never paints over its neighbour. They come from the
/// font's DESIGN TABLES — `GetDesignGlyphMetrics` for the advance,
/// `IDWriteFontFace::GetMetrics` for ascent/descent/lineGap — not from
/// drawing anything. That is what makes them testable with no window, no DC
/// and no paint having happened, which is precisely what the GDI
/// `DT_CALCRECT`-in-a-paint scheme was not.
pub struct CellMetrics {
    pub cell_w: u32,
    pub cell_h: u32,
}

/// Cascadia Mono's face plus its cell metrics, or a String naming what
/// failed. `pt` is the type size in points; `dpi` the pane's DPI.
///
/// Cascadia Mono and not Cascadia Code, for §2.1's own reason: a code grid
/// deliberately has no ligatures, columns should be columns.
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
        let face = font.CreateFontFace().map_err(|e| format!("CreateFontFace: {e}"))?;

        // Design-table metrics. The em size in physical pixels is
        // pt/72 * dpi; every design unit scales by em_px / designUnitsPerEm.
        let mut fm = Default::default();
        face.GetMetrics(&mut fm);
        let em_px = pt / 72.0 * dpi;
        let scale = em_px / fm.designUnitsPerEm as f32;

        // The advance of 'M' — via GetGlyphIndices, because
        // GetDesignGlyphMetrics wants a GLYPH INDEX and feeding it the
        // codepoint compiles and quietly answers some other glyph's metrics
        // (the design doc's pitfall list names this one).
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
        let cell_h = ((fm.ascent as i32 + fm.descent as i32 + fm.lineGap as i32) as f32 * scale)
            .ceil() as u32;
        if cell_w == 0 || cell_h == 0 {
            return Err(format!("degenerate cell {cell_w}x{cell_h}"));
        }
        Ok((face, CellMetrics { cell_w, cell_h }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WG6d-1's opening claim, proven the way GDI never allowed: the font's
    /// metrics, HEADLESS — no window, no DC, no paint. If this passes on a
    /// machine, the grid's two numbers exist before any pane does, and
    /// nothing about them can depend on when the first WM_PAINT arrived
    /// (the defect that shipped as "a click lands at the end of the
    /// document").
    #[test]
    fn metrics_exist_with_no_window_and_scale_with_dpi() {
        let (_face, m96) = mono_face_metrics(9.0, 96.0).expect("Cascadia Mono at 96dpi");
        assert!(m96.cell_w > 0 && m96.cell_h > 0);
        assert!(
            m96.cell_h > m96.cell_w,
            "a text cell is taller than wide; {}x{} says the axes are swapped",
            m96.cell_w,
            m96.cell_h
        );
        // Monospace sanity: 'M' and 'i' advance identically, or the whole
        // premise of a cell grid is wrong for this face.
        let (face, _) = mono_face_metrics(9.0, 96.0).unwrap();
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
        // And the grid scales with DPI, which is the other half of the
        // stale-metrics defect: 144dpi cells must be larger than 96dpi ones.
        let (_f, m144) = mono_face_metrics(9.0, 144.0).expect("Cascadia Mono at 144dpi");
        assert!(m144.cell_w > m96.cell_w);
        assert!(m144.cell_h > m96.cell_h);
    }
}
