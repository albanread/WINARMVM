//! WG10a — the pixel pipe, made of shaders. Ported from upstream's Metal
//! renderer (`MacGamePane/graphics`), whose two working parts this reproduces
//! on D3D11:
//!
//! * `indexed_pane.rs`: an `R8Uint` texture of palette indices whose colour
//!   lookup happens **in the fragment shader** — `palette[index]`, per pixel,
//!   on the GPU. That placement is the whole design: a palette cycle uploads
//!   1KB and the GPU re-colours every pixel on screen.
//! * `text_overlay.rs`: text as a texture composited last by a shader, alpha
//!   blended over whatever is underneath.
//!
//! The D2D path this replaces was a CPU pipe wearing a GPU device: a
//! `CreateBitmap` + `DrawBitmap` per present for the plane (a fresh D2D bitmap
//! every frame), per-glyph `DrawGlyphRun` calls for the text, and SM4's
//! palette expanded BY THE CPU into BGRA before any of it. Named by the
//! author: "it is meant to be displayed by a shader into the pane... the text
//! overlay should also be a shader."
//!
//! # The Snapdragon posture
//!
//! This machine is a unified-memory SOC — one LPDDR5x pool, Adreno on die,
//! and the device reports `UnifiedMemoryArchitecture: TRUE` (asked via
//! `CheckFeatureSupport`, answered to the guest through `MacvmRenderIsUma`).
//! Every per-frame upload here is `USAGE_DYNAMIC` + `Map(WRITE_DISCARD)`,
//! which on a UMA driver is a buffer RENAME in that shared pool — no staging
//! copy, no bus. What crosses per frame is what changed and nothing else:
//!
//! * direct plane: `w*h*4` bytes of BGRA the guest wrote;
//! * indexed plane: `w*h` BYTES of slot indices — a quarter of the direct
//!   plane — plus 1KB of palette when it moved;
//! * text: `cols*rows*16` bytes of cell records (~64KB for a full pane);
//! * and the glyph atlas uploads once per NEW glyph, then never again.
//!
//! The GPU does the multiplication: palette expansion, glyph compositing,
//! nearest-neighbour stretch. The CPU's whole job is arithmetic the guest
//! chose to do and one rename-copy of its result.

use std::collections::HashMap;

use windows::core::PCSTR;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11BlendState, ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader,
    ID3D11RasterizerState, ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView,
    ID3D11Texture2D, ID3D11VertexShader, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_OP_ADD, D3D11_BLEND_SRC_ALPHA,
    D3D11_BUFFER_DESC, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_CPU_ACCESS_WRITE,
    D3D11_FEATURE_D3D11_OPTIONS2, D3D11_FEATURE_DATA_D3D11_OPTIONS2, D3D11_FILTER_MIN_MAG_MIP_POINT,
    D3D11_MAP_WRITE_DISCARD, D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC,
    D3D11_TEXTURE2D_DESC, D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC,
    D3D11_VIEWPORT, D3D11_CULL_NONE, D3D11_FILL_SOLID, D3D11_RASTERIZER_DESC,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteFontFace, DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_RENDERING_MODE_NATURAL, DWRITE_TEXTURE_CLEARTYPE_3x1,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R32G32B32A32_UINT, DXGI_FORMAT_R8_UINT,
    DXGI_FORMAT_R8_UNORM, DXGI_SAMPLE_DESC,
};

use crate::{Cell, CellMetrics, PixelPlane, BG_TRANSPARENT};

/// The whole pipeline's shader source. HLSL compiled at attach with Fxc — the
/// shaders stay readable source in this crate, exactly as upstream keeps its
/// Metal inline.
///
/// One vertex shader — the classic fullscreen triangle from `SV_VertexID`,
/// three vertices and no vertex buffer, byte-for-byte the trick upstream's
/// `vmain` uses — and three pixel shaders, one per pass.
const HLSL: &str = r#"
cbuffer C : register(b0) {
    float2 pane;      // pane size, px
    float2 cell;      // cell size, px
    uint cols;        // grid width, cells
    uint rows;        // grid height, cells
    uint atlasCols;   // atlas slots per row
    uint bgTrans;     // the BG_TRANSPARENT sentinel
    int caretCol;     // caret cell, -1 when hidden
    int caretRow;
    uint pad0;
    uint pad1;
};

struct VOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; };

VOut vs_main(uint vid : SV_VertexID) {
    float2 p = float2(vid == 1 ? 3.0 : -1.0, vid == 2 ? 3.0 : -1.0);
    VOut o;
    o.pos = float4(p, 0.0, 1.0);
    o.uv = float2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return o;
}

// ── pass 1a: the DIRECT plane ───────────────────────────────────────────
// The guest's BGRA bytes, point-sampled to fill the pane. POINT, not linear:
// the plane is pixels the guest computed, and interpolation would invent
// values it never wrote.
Texture2D<float4> planeTex : register(t0);
SamplerState pointSamp : register(s0);

float4 ps_plane(VOut i) : SV_Target {
    return float4(planeTex.Sample(pointSamp, i.uv).rgb, 1.0);
}

// ── pass 1b: the INDEXED plane (SM4, on the GPU) ────────────────────────
// One byte per pixel names a palette slot; the lookup happens HERE, per
// pixel, which is upstream's `fmain` verbatim: `palette[k]`. The CPU never
// expands anything — a palette cycle is a 1KB upload and this shader.
Texture2D<uint> idxTex : register(t0);
Texture2D<float4> palTex : register(t1);

float4 ps_indexed(VOut i) : SV_Target {
    uint w, h;
    idxTex.GetDimensions(w, h);
    uint2 p = uint2(i.uv * float2(w, h));
    uint ci = idxTex.Load(int3(min(p.x, w - 1), min(p.y, h - 1), 0));
    return float4(palTex.Load(int3(ci, 0, 0)).rgb, 1.0);
}

// ── pass 2: the TEXT plane ──────────────────────────────────────────────
// The cell grid as a cols x rows uint4 texture (atlas slot, fg, bg, unused)
// and a glyph atlas rasterised by DirectWrite. Per pixel: which cell, which
// glyph, mix fg over bg by the atlas's coverage. A cell whose bg is the
// BG_TRANSPARENT sentinel emits only its INK, alpha-blended, so the plane
// shows through everywhere the glyph is not — upstream's text_overlay
// compositing, with the colour decision moved into the shader.
Texture2D<uint4> cellsTex : register(t0);
Texture2D<float> atlasTex : register(t1);

float3 colorref(uint c) {
    // COLORREF is 0x00BBGGRR: R is the LOW byte.
    return float3(c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF) / 255.0;
}

float4 ps_text(VOut i) : SV_Target {
    float2 px = i.uv * pane;
    uint cx = (uint)(px.x / cell.x);
    uint cy = (uint)(px.y / cell.y);
    if (cx >= cols || cy >= rows) {
        return float4(0, 0, 0, 0);   // the margin keeps whatever is below
    }
    uint4 c = cellsTex.Load(int3(cx, cy, 0));
    float2 inCell = px - float2(cx, cy) * cell;
    uint2 slotOrigin = uint2(c.x % atlasCols, c.x / atlasCols)
        * uint2((uint)cell.x, (uint)cell.y);
    float a = atlasTex.Load(int3(slotOrigin + uint2(inCell), 0));

    // The caret: a 2px bar at the cell's left edge, exactly the D2D one.
    bool caret = (int(cx) == caretCol) && (int(cy) == caretRow) && (inCell.x < 2.0);

    float3 fg = colorref(c.y);
    if (c.z == bgTrans) {
        // Ink only, over the plane. The caret still draws — a HUD cursor
        // over live pixels is one value, not a mode.
        if (caret) { return float4(0, 0, 0, 1); }
        return float4(fg, a);
    }
    float3 bg = colorref(c.z);
    float3 col = caret ? float3(0, 0, 0) : lerp(bg, fg, a);
    return float4(col, 1.0);
}
"#;

/// The cbuffer, mirrored exactly. 48 bytes, three 16-byte registers.
#[repr(C)]
#[derive(Clone, Copy)]
struct Uniforms {
    pane: [f32; 2],
    cell: [f32; 2],
    cols: u32,
    rows: u32,
    atlas_cols: u32,
    bg_trans: u32,
    caret_col: i32,
    caret_row: i32,
    pad0: u32,
    pad1: u32,
}

/// Atlas geometry: 32 slots per row, grown DOWN as glyphs arrive. 2048 slots
/// of one cell each — a full ASCII shell uses under 100.
const ATLAS_COLS: u32 = 32;
const ATLAS_ROWS: u32 = 64;

fn compile(src: &str, entry: &str, target: &str) -> Result<ID3DBlob, String> {
    unsafe {
        let mut code: Option<ID3DBlob> = None;
        let mut errs: Option<ID3DBlob> = None;
        let entry_c = format!("{entry}\0");
        let target_c = format!("{target}\0");
        let hr = D3DCompile(
            src.as_ptr() as *const _,
            src.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry_c.as_ptr()),
            PCSTR(target_c.as_ptr()),
            0,
            0,
            &mut code,
            Some(&mut errs),
        );
        if hr.is_err() {
            let msg = errs
                .map(|b| {
                    let p = b.GetBufferPointer() as *const u8;
                    let n = b.GetBufferSize();
                    String::from_utf8_lossy(std::slice::from_raw_parts(p, n)).into_owned()
                })
                .unwrap_or_else(|| format!("{hr:?}"));
            return Err(format!("D3DCompile {entry}: {msg}"));
        }
        code.ok_or_else(|| format!("D3DCompile {entry}: no blob"))
    }
}

fn blob_bytes(b: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}

/// Everything the shader path owns for one pane.
pub struct Pipe {
    pub device: ID3D11Device,
    pub dctx: ID3D11DeviceContext,
    /// `UnifiedMemoryArchitecture` from `CheckFeatureSupport` — the SOC fact,
    /// asked once and reported to the guest via `MacvmRenderIsUma`.
    pub uma: bool,
    vs: ID3D11VertexShader,
    ps_plane: ID3D11PixelShader,
    ps_indexed: ID3D11PixelShader,
    ps_text: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    /// CULL_NONE, and it is LOAD-BEARING: the fullscreen triangle is
    /// counter-clockwise, and D3D11's default rasterizer culls it as a back
    /// face — a silently white pane with every present reporting success.
    /// Metal has no default cull, which is why upstream's identical triangle
    /// never met this.
    raster: ID3D11RasterizerState,
    cbuf: ID3D11Buffer,

    // The atlas: one cell per slot, rasterised by DirectWrite on first use.
    atlas_tex: ID3D11Texture2D,
    atlas_srv: ID3D11ShaderResourceView,
    slots: HashMap<u32, u32>,
    next_slot: u32,

    // Per-frame textures, recreated only when their shape changes.
    cells_tex: Option<(u32, u32, ID3D11Texture2D, ID3D11ShaderResourceView)>,
    plane_tex: Option<(u32, u32, ID3D11Texture2D, ID3D11ShaderResourceView)>,
    idx_tex: Option<(u32, u32, ID3D11Texture2D, ID3D11ShaderResourceView)>,
    pal_tex: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    /// Scratch for the cell translation, kept to avoid a per-frame alloc.
    scratch: Vec<[u32; 4]>,
}

impl Pipe {
    pub fn new(
        device: ID3D11Device,
        dctx: ID3D11DeviceContext,
        metrics: &CellMetrics,
    ) -> Result<Self, String> {
        unsafe {
            // The SOC fact, asked of the device rather than assumed of the
            // machine: TRUE on Snapdragon (one LPDDR5x pool, Adreno on die),
            // FALSE on a discrete GPU, and the design above holds either way —
            // UMA just makes the Map(DISCARD) path a rename instead of a copy.
            let mut opts2 = D3D11_FEATURE_DATA_D3D11_OPTIONS2::default();
            let uma = device
                .CheckFeatureSupport(
                    D3D11_FEATURE_D3D11_OPTIONS2,
                    &mut opts2 as *mut _ as *mut _,
                    std::mem::size_of::<D3D11_FEATURE_DATA_D3D11_OPTIONS2>() as u32,
                )
                .is_ok()
                && opts2.UnifiedMemoryArchitecture.as_bool();

            let vs_blob = compile(HLSL, "vs_main", "vs_5_0")?;
            let ps_plane_blob = compile(HLSL, "ps_plane", "ps_5_0")?;
            let ps_indexed_blob = compile(HLSL, "ps_indexed", "ps_5_0")?;
            let ps_text_blob = compile(HLSL, "ps_text", "ps_5_0")?;

            let mut vs = None;
            device
                .CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs))
                .map_err(|e| format!("CreateVertexShader: {e}"))?;
            let mut ps_plane = None;
            device
                .CreatePixelShader(blob_bytes(&ps_plane_blob), None, Some(&mut ps_plane))
                .map_err(|e| format!("CreatePixelShader plane: {e}"))?;
            let mut ps_indexed = None;
            device
                .CreatePixelShader(blob_bytes(&ps_indexed_blob), None, Some(&mut ps_indexed))
                .map_err(|e| format!("CreatePixelShader indexed: {e}"))?;
            let mut ps_text = None;
            device
                .CreatePixelShader(blob_bytes(&ps_text_blob), None, Some(&mut ps_text))
                .map_err(|e| format!("CreatePixelShader text: {e}"))?;

            let samp_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ..Default::default()
            };
            let mut sampler = None;
            device
                .CreateSamplerState(&samp_desc, Some(&mut sampler))
                .map_err(|e| format!("CreateSamplerState: {e}"))?;

            // Alpha blend for the text pass — upstream text_overlay's exact
            // factors. The plane passes draw opaque and never bind it.
            let blend_desc = D3D11_BLEND_DESC {
                RenderTarget: [D3D11_RENDER_TARGET_BLEND_DESC {
                    BlendEnable: true.into(),
                    SrcBlend: D3D11_BLEND_SRC_ALPHA,
                    DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOp: D3D11_BLEND_OP_ADD,
                    SrcBlendAlpha: D3D11_BLEND_SRC_ALPHA,
                    DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOpAlpha: D3D11_BLEND_OP_ADD,
                    RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
                }; 8],
                ..Default::default()
            };
            let mut blend = None;
            device
                .CreateBlendState(&blend_desc, Some(&mut blend))
                .map_err(|e| format!("CreateBlendState: {e}"))?;

            let raster_desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                ..Default::default()
            };
            let mut raster = None;
            device
                .CreateRasterizerState(&raster_desc, Some(&mut raster))
                .map_err(|e| format!("CreateRasterizerState: {e}"))?;

            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<Uniforms>() as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut cbuf = None;
            device
                .CreateBuffer(&cb_desc, None, Some(&mut cbuf))
                .map_err(|e| format!("CreateBuffer cbuf: {e}"))?;

            // The atlas: R8 coverage, DEFAULT usage — written once per NEW
            // glyph via UpdateSubresource, sampled forever after. Slot 0 is
            // reserved blank so an unmapped codepoint is a space, not garbage.
            let atlas_w = ATLAS_COLS * metrics.cell_w;
            let atlas_h = ATLAS_ROWS * metrics.cell_h;
            let atlas_desc = D3D11_TEXTURE2D_DESC {
                Width: atlas_w,
                Height: atlas_h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let mut atlas_tex = None;
            device
                .CreateTexture2D(&atlas_desc, None, Some(&mut atlas_tex))
                .map_err(|e| format!("CreateTexture2D atlas: {e}"))?;
            let atlas_tex = atlas_tex.unwrap();
            let mut atlas_srv = None;
            device
                .CreateShaderResourceView(&atlas_tex, None, Some(&mut atlas_srv))
                .map_err(|e| format!("CreateShaderResourceView atlas: {e}"))?;

            Ok(Pipe {
                device,
                dctx,
                uma,
                vs: vs.unwrap(),
                ps_plane: ps_plane.unwrap(),
                ps_indexed: ps_indexed.unwrap(),
                ps_text: ps_text.unwrap(),
                sampler: sampler.unwrap(),
                blend: blend.unwrap(),
                raster: raster.unwrap(),
                cbuf: cbuf.unwrap(),
                atlas_tex,
                atlas_srv: atlas_srv.unwrap(),
                slots: HashMap::new(),
                next_slot: 1,
                cells_tex: None,
                plane_tex: None,
                idx_tex: None,
                pal_tex: None,
                scratch: Vec::new(),
            })
        }
    }

    /// The slot holding `cp`'s glyph, rasterising it into the atlas on first
    /// use. DirectWrite's `CreateGlyphRunAnalysis` + `CreateAlphaTexture` —
    /// the same rasteriser the D2D path used, one glyph at a time, once.
    fn slot_for(
        &mut self,
        factory: &IDWriteFactory,
        face: &IDWriteFontFace,
        metrics: &CellMetrics,
        cp: u32,
    ) -> u32 {
        if cp == b' ' as u32 {
            return 0;
        }
        if let Some(&s) = self.slots.get(&cp) {
            return s;
        }
        if self.next_slot >= ATLAS_COLS * ATLAS_ROWS {
            // Full: evict everything and refill on demand. Crude, honest, and
            // unreachable short of thousands of distinct codepoints.
            self.slots.clear();
            self.next_slot = 1;
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        self.slots.insert(cp, slot);

        // Rasterise: coverage bytes for this one glyph, baseline at ascent.
        unsafe {
            let mut gi = [0u16];
            if face.GetGlyphIndices(&cp, 1, gi.as_mut_ptr()).is_err() || gi[0] == 0 {
                return 0;
            }
            let advances = [0.0f32];
            let offsets = [DWRITE_GLYPH_OFFSET::default()];
            let run = DWRITE_GLYPH_RUN {
                fontFace: std::mem::ManuallyDrop::new(Some(face.clone())),
                fontEmSize: metrics.em_px,
                glyphCount: 1,
                glyphIndices: gi.as_ptr(),
                glyphAdvances: advances.as_ptr(),
                glyphOffsets: offsets.as_ptr(),
                isSideways: false.into(),
                bidiLevel: 0,
            };
            let analysis = factory.CreateGlyphRunAnalysis(
                &run,
                1.0,
                None,
                DWRITE_RENDERING_MODE_NATURAL,
                DWRITE_MEASURING_MODE_NATURAL,
                0.0,
                metrics.ascent,
            );
            let _ = std::mem::ManuallyDrop::into_inner(run.fontFace);
            let Ok(analysis) = analysis else { return 0 };
            let Ok(bounds) = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_CLEARTYPE_3x1) else {
                return 0;
            };
            let bw = (bounds.right - bounds.left).max(0) as usize;
            let bh = (bounds.bottom - bounds.top).max(0) as usize;
            if bw == 0 || bh == 0 {
                return slot; // a mark with no ink (e.g. NBSP): a blank slot
            }
            let mut rgb = vec![0u8; bw * bh * 3];
            if analysis
                .CreateAlphaTexture(DWRITE_TEXTURE_CLEARTYPE_3x1, &bounds, &mut rgb)
                .is_err()
            {
                return 0;
            }

            // ClearType coverage is 3 bytes per pixel; average to one alpha.
            // Then place the glyph's bounds into the slot's cell, clamped —
            // an overhanging glyph is cropped to its cell exactly as the cell
            // grid's contract says it must be.
            let cw = metrics.cell_w as usize;
            let ch = metrics.cell_h as usize;
            let mut cellbuf = vec![0u8; cw * ch];
            for y in 0..bh {
                let dy = bounds.top + y as i32;
                if dy < 0 || dy >= ch as i32 {
                    continue;
                }
                for x in 0..bw {
                    let dx = bounds.left + x as i32;
                    if dx < 0 || dx >= cw as i32 {
                        continue;
                    }
                    let o = (y * bw + x) * 3;
                    let a =
                        (rgb[o] as u32 + rgb[o + 1] as u32 + rgb[o + 2] as u32) / 3;
                    cellbuf[dy as usize * cw + dx as usize] = a as u8;
                }
            }
            let sx = (slot % ATLAS_COLS) * metrics.cell_w;
            let sy = (slot / ATLAS_COLS) * metrics.cell_h;
            let dest = windows::Win32::Graphics::Direct3D11::D3D11_BOX {
                left: sx,
                top: sy,
                front: 0,
                right: sx + metrics.cell_w,
                bottom: sy + metrics.cell_h,
                back: 1,
            };
            self.dctx.UpdateSubresource(
                &self.atlas_tex,
                0,
                Some(&dest),
                cellbuf.as_ptr() as *const _,
                cw as u32,
                0,
            );
        }
        slot
    }

    /// A DYNAMIC texture + SRV of the given shape, recreated only on change.
    fn dynamic_tex(
        device: &ID3D11Device,
        w: u32,
        h: u32,
        format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    ) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView), String> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut tex = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .map_err(|e| format!("CreateTexture2D dyn: {e}"))?;
            let tex = tex.unwrap();
            let mut srv = None;
            device
                .CreateShaderResourceView(&tex, None, Some(&mut srv))
                .map_err(|e| format!("CreateShaderResourceView dyn: {e}"))?;
            Ok((tex, srv.unwrap()))
        }
    }

    /// Map(WRITE_DISCARD) + row-copy `src` into `tex`. On a UMA driver this is
    /// the rename path — the "upload" is a write into the same physical pool
    /// the GPU samples.
    fn upload(
        &self,
        tex: &ID3D11Texture2D,
        src: &[u8],
        src_stride: usize,
        rows: usize,
    ) -> Result<(), String> {
        unsafe {
            let mut mapped = Default::default();
            self.dctx
                .Map(tex, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
                .map_err(|e| format!("Map: {e}"))?;
            let dst = mapped.pData as *mut u8;
            for y in 0..rows {
                std::ptr::copy_nonoverlapping(
                    src.as_ptr().add(y * src_stride),
                    dst.add(y * mapped.RowPitch as usize),
                    src_stride,
                );
            }
            self.dctx.Unmap(tex, 0);
            Ok(())
        }
    }

    /// Draw one frame into `rtv`. This is the whole present path: at most two
    /// fullscreen-triangle passes over the pane — the plane, then the text —
    /// with every colour decision made by a pixel shader.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        rtv: &ID3D11RenderTargetView,
        px_w: u32,
        px_h: u32,
        factory: &IDWriteFactory,
        face: &IDWriteFontFace,
        metrics: &CellMetrics,
        cols: u32,
        rows: u32,
        cells: &[Cell],
        caret: Option<(u32, u32)>,
        plane: Option<&PixelPlane>,
    ) -> Result<(), String> {
        unsafe {
            let white = [1.0f32, 1.0, 1.0, 1.0];
            self.dctx.ClearRenderTargetView(rtv, &white);
            self.dctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            self.dctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: px_w as f32,
                Height: px_h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            self.dctx
                .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.dctx.RSSetState(&self.raster);
            self.dctx.VSSetShader(&self.vs, None);

            // The cbuffer, once per frame.
            let u = Uniforms {
                pane: [px_w as f32, px_h as f32],
                cell: [metrics.cell_w as f32, metrics.cell_h as f32],
                cols,
                rows,
                atlas_cols: ATLAS_COLS,
                bg_trans: BG_TRANSPARENT,
                caret_col: caret.map(|c| c.0 as i32).unwrap_or(-1),
                caret_row: caret.map(|c| c.1 as i32).unwrap_or(-1),
                pad0: 0,
                pad1: 0,
            };
            let mut mapped = Default::default();
            self.dctx
                .Map(&self.cbuf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
                .map_err(|e| format!("Map cbuf: {e}"))?;
            std::ptr::copy_nonoverlapping(
                &u as *const Uniforms as *const u8,
                mapped.pData as *mut u8,
                std::mem::size_of::<Uniforms>(),
            );
            self.dctx.Unmap(&self.cbuf, 0);
            self.dctx.PSSetConstantBuffers(0, Some(&[Some(self.cbuf.clone())]));

            // ── pass 1: the plane, if this pane has one ──────────────────
            if let Some(p) = plane {
                self.dctx.OMSetBlendState(None, None, u32::MAX);
                if p.indexed {
                    let need = match &self.idx_tex {
                        Some((w, h, _, _)) => *w != p.w || *h != p.h,
                        None => true,
                    };
                    if need {
                        let (t, s) = Self::dynamic_tex(&self.device, p.w, p.h, DXGI_FORMAT_R8_UINT)?;
                        self.idx_tex = Some((p.w, p.h, t, s));
                    }
                    if self.pal_tex.is_none() {
                        let (t, s) =
                            Self::dynamic_tex(&self.device, 256, 1, DXGI_FORMAT_B8G8R8A8_UNORM)?;
                        self.pal_tex = Some((t, s));
                    }
                    let (_, _, idx_t, idx_s) = self.idx_tex.as_ref().unwrap();
                    let (pal_t, pal_s) = self.pal_tex.as_ref().unwrap();
                    self.upload(idx_t, &p.indices, p.w as usize, p.h as usize)?;
                    let pal_bytes: &[u8] = std::slice::from_raw_parts(
                        p.palette.as_ptr() as *const u8,
                        p.palette.len() * 4,
                    );
                    self.upload(pal_t, pal_bytes, p.palette.len() * 4, 1)?;
                    self.dctx.PSSetShader(&self.ps_indexed, None);
                    self.dctx
                        .PSSetShaderResources(0, Some(&[Some(idx_s.clone()), Some(pal_s.clone())]));
                } else {
                    let need = match &self.plane_tex {
                        Some((w, h, _, _)) => *w != p.w || *h != p.h,
                        None => true,
                    };
                    if need {
                        let (t, s) =
                            Self::dynamic_tex(&self.device, p.w, p.h, DXGI_FORMAT_B8G8R8A8_UNORM)?;
                        self.plane_tex = Some((p.w, p.h, t, s));
                    }
                    let (_, _, t, s) = self.plane_tex.as_ref().unwrap();
                    self.upload(t, &p.bytes, p.stride as usize, p.h as usize)?;
                    self.dctx.PSSetShader(&self.ps_plane, None);
                    self.dctx.PSSetShaderResources(0, Some(&[Some(s.clone())]));
                    self.dctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
                }
                self.dctx.Draw(3, 0);
            }

            // ── pass 2: the text ─────────────────────────────────────────
            if cols > 0 && rows > 0 && cells.len() >= (cols * rows) as usize {
                // Translate cells to (slot, fg, bg, 0), rasterising new
                // glyphs as they appear. After the first frame this loop is
                // pure table lookups.
                self.scratch.clear();
                self.scratch.reserve((cols * rows) as usize);
                for c in &cells[..(cols * rows) as usize] {
                    let slot = self.slot_for(factory, face, metrics, c.cp);
                    self.scratch.push([slot, c.fg, c.bg, 0]);
                }
                let need = match &self.cells_tex {
                    Some((w, h, _, _)) => *w != cols || *h != rows,
                    None => true,
                };
                if need {
                    let (t, s) =
                        Self::dynamic_tex(&self.device, cols, rows, DXGI_FORMAT_R32G32B32A32_UINT)?;
                    self.cells_tex = Some((cols, rows, t, s));
                }
                let (_, _, t, s) = self.cells_tex.as_ref().unwrap();
                let bytes: &[u8] = std::slice::from_raw_parts(
                    self.scratch.as_ptr() as *const u8,
                    self.scratch.len() * 16,
                );
                self.upload(t, bytes, cols as usize * 16, rows as usize)?;
                self.dctx
                    .OMSetBlendState(&self.blend, None, u32::MAX);
                self.dctx.PSSetShader(&self.ps_text, None);
                self.dctx
                    .PSSetShaderResources(0, Some(&[Some(s.clone()), Some(self.atlas_srv.clone())]));
                self.dctx.Draw(3, 0);
            }

            // Unbind, so the next frame's Map(DISCARD) never races a binding.
            self.dctx.PSSetShaderResources(0, Some(&[None, None]));
            self.dctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::DirectWrite::{DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED};

    /// The REAL pipeline, headless: render into an offscreen target and read
    /// the pixels back. This is the test that would have caught the cull —
    /// the D2D-era WIC test proved a path production no longer takes, and the
    /// gate's counters all passed over a silently white pane.
    fn render_and_read(
        cols: u32,
        rows: u32,
        cells: Vec<Cell>,
        plane: Option<PixelPlane>,
        w: u32,
        h: u32,
    ) -> Vec<u32> {
        unsafe {
            let mut device = None;
            let mut hr = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            );
            if hr.is_err() {
                hr = D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    Default::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                );
            }
            hr.expect("a device, hardware or WARP");
            let device = device.unwrap();
            let dctx = device.GetImmediateContext().unwrap();

            let (face, metrics) = crate::mono_face_metrics(9.0, 96.0).expect("metrics");
            let factory: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("dwrite");

            let target_desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                ..Default::default()
            };
            let mut target = None;
            device
                .CreateTexture2D(&target_desc, None, Some(&mut target))
                .expect("target");
            let target = target.unwrap();
            let mut rtv = None;
            device
                .CreateRenderTargetView(&target, None, Some(&mut rtv))
                .expect("rtv");

            let mut pipe = Pipe::new(device.clone(), dctx.clone(), &metrics).expect("pipe");
            pipe.render(
                &rtv.unwrap(),
                w,
                h,
                &factory,
                &face,
                &metrics,
                cols,
                rows,
                &cells,
                None,
                plane.as_ref(),
            )
            .expect("render");

            let staging_desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..target_desc
            };
            let mut staging = None;
            device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .expect("staging");
            let staging = staging.unwrap();
            dctx.CopyResource(&staging, &target);
            let mut mapped = Default::default();
            dctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .expect("map");
            let mut out = vec![0u32; (w * h) as usize];
            for y in 0..h as usize {
                let row = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                std::ptr::copy_nonoverlapping(
                    row,
                    out.as_mut_ptr().add(y * w as usize) as *mut u8,
                    w as usize * 4,
                );
            }
            dctx.Unmap(&staging, 0);
            out
        }
    }

    #[test]
    fn the_shader_pipe_inks_glyphs_where_the_cells_say() {
        // One 'M', black on white, in a 4x2 grid. Some pixel inside that cell
        // is dark and the far corner is white — the same claim the WIC test
        // makes of the D2D path, now made of the path production takes.
        let (_, m) = crate::mono_face_metrics(9.0, 96.0).expect("metrics");
        let mut cells = vec![Cell::blank(); 8];
        cells[0] = Cell {
            cp: 'M' as u32,
            fg: 0x0000_0000,
            bg: 0x00FF_FFFF,
        };
        let w = m.cell_w * 4;
        let h = m.cell_h * 2;
        let px = render_and_read(4, 2, cells, None, w, h);
        let cell0_has_ink = (0..m.cell_h)
            .any(|y| (0..m.cell_w).any(|x| px[(y * w + x) as usize] & 0xFF_FFFF != 0xFF_FFFF));
        assert!(cell0_has_ink, "an 'M' must put ink in its own cell");
        let far = px[((h - 1) * w + (w - 1)) as usize];
        assert_eq!(
            far & 0xFF_FFFF,
            0xFF_FFFF,
            "a blank cell stays white, got {far:08x}"
        );
    }

    #[test]
    fn the_indexed_plane_is_expanded_by_the_shader_not_the_cpu() {
        // SM4's claim against the REAL pipeline: indices written once, one
        // palette slot changed, the rendered colour follows. `resolve()` is
        // never called — if the shader lookup broke, this is the test.
        let mut p = PixelPlane::new(8, 8);
        p.make_indexed();
        p.indices.iter_mut().for_each(|i| *i = 7);
        p.palette[7] = 0xFFFF_0000; // red
        let px = render_and_read(0, 0, Vec::new(), Some(p), 32, 32);
        assert_eq!(
            px[0] & 0xFF_FFFF,
            0xFF_0000,
            "slot 7 is red, got {:08x}",
            px[0]
        );

        let mut p2 = PixelPlane::new(8, 8);
        p2.make_indexed();
        p2.indices.iter_mut().for_each(|i| *i = 7);
        p2.palette[7] = 0xFF00_FF00; // the SAME indices, one palette word moved
        let px2 = render_and_read(0, 0, Vec::new(), Some(p2), 32, 32);
        assert_eq!(
            px2[0] & 0xFF_FFFF,
            0x00_FF00,
            "the screen follows the palette"
        );
    }

    #[test]
    fn a_transparent_background_lets_the_plane_through() {
        // The composition claim: a direct red plane under a grid whose cells
        // carry BG_TRANSPARENT. A pixel with no ink is the PLANE's red, not
        // the grid's fill — and a cell with an opaque bg still covers it.
        let (_, m) = crate::mono_face_metrics(9.0, 96.0).expect("metrics");
        let mut p = PixelPlane::new(4, 4);
        for px in p.bytes.chunks_exact_mut(4) {
            px.copy_from_slice(&0xFFFF_0000u32.to_le_bytes()); // red, opaque
        }
        let mut cells = vec![
            Cell {
                cp: ' ' as u32,
                fg: 0x00FF_FFFF,
                bg: crate::BG_TRANSPARENT
            };
            4
        ];
        cells[1] = Cell {
            cp: ' ' as u32,
            fg: 0,
            bg: 0x0000_FF00,
        }; // opaque green
        let w = m.cell_w * 2;
        let h = m.cell_h * 2;
        let px = render_and_read(2, 2, cells, Some(p), w, h);
        assert_eq!(
            px[0] & 0xFF_FFFF,
            0xFF_0000,
            "transparent bg shows the plane, got {:08x}",
            px[0]
        );
        let in_cell1 = px[(m.cell_w + 1) as usize] & 0xFF_FFFF;
        assert_eq!(
            in_cell1, 0x00_FF00,
            "an opaque bg still covers it, got {in_cell1:08x}"
        );
    }
}
