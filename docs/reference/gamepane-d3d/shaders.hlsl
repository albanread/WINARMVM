// ============================================================================
// WINDART game pane — HLSL (Shader Model 5.0) translation of the Metal (MSL)
// shaders inlined in gp_engine.mm:17-148 and :1121-1139.
//
// Target: D3D11 via D3DCompile, profiles vs_5_0 / ps_5_0 / cs_5_0.
// Source of truth: e:\windart\MACDARTV1\macdart\cocoa\gamepane\gp_engine.mm
//                  (READ-ONLY; every kXxxMsl string is cited by line below).
//
// This file is the *reference*; gp_engine_d3d.cpp compiles these bodies at
// runtime with D3DCompile (mirroring Metal's newLibraryWithSource:). The
// per-scanline "copper" palette, the compute blitter, and the runtime shader
// background are all preserved verbatim in behaviour — see the delta notes.
//
// ---------------------------------------------------------------------------
// GLOBAL MSL -> HLSL DELTAS (apply to every shader below)
// ---------------------------------------------------------------------------
//  MSL construct                       -> HLSL (SM5.0)
//  ----------------------------------     ------------------------------------
//  [[vertex_id]]                       -> uint vid : SV_VertexID
//  [[position]] (VOut.pos)             -> float4 pos : SV_Position
//  fragment returns float4             -> float4 ... : SV_Target
//  [[stage_in]]                        -> plain struct arg (interpolated)
//  [[texture(n)]] read-only            -> Texture2D<...> : register(t#)   (SRV)
//  [[texture(n)]] access::write/rw     -> RWTexture2D<...> : register(u#)  (UAV)
//  [[buffer(n)]] constant Uniforms&    -> cbuffer ... : register(b#)
//  [[buffer(n)]] constant float4*      -> StructuredBuffer<float4> : register(t#)
//  tex.read(uint2(x,y)).r              -> tex.Load(int3(x,y,0))   (returns uint)
//  tex.get_width()/get_height()        -> tex.GetDimensions(w,h)  (out-params)
//  tex.sample(s, uv)                   -> tex.Sample(smp, uv)  + SamplerState s#
//  constexpr sampler s(...)            -> host-created SamplerState at s0
//                                          (SM5.0 has no inline/static samplers)
//  discard_fragment()                  -> discard;
//  [[thread_position_in_grid]] (uint2) -> uint3 gid : SV_DispatchThreadID
//  dispatchThreadgroups: (in groups)   -> Dispatch(gx,gy,1); [numthreads(16,16,1)]
//
// ---------------------------------------------------------------------------
// CLIP-SPACE Y / TEXEL ORIGIN  (the one every porter gets wrong — flagged)
// ---------------------------------------------------------------------------
//  Metal and D3D11 AGREE: NDC is Y-up ([-1,+1], +1 = TOP of the render target),
//  depth is [0,1], and the texel origin is TOP-LEFT (V=0 at top). Metal's NDC
//  was designed to match Direct3D; the famous "flip" is OpenGL/Vulkan (Y-down
//  NDC / bottom-left texel origin), NOT this port.
//
//  Consequence: the verbatim UV math
//      o.uv = float2((pos.x+1)*0.5, 1.0 - (pos.y+1)*0.5)
//  is convention-neutral and ports AS-IS. It only encodes "uv(0,0) at screen
//  top-left", which is true in both APIs. The CPU index buffers are uploaded
//  row-0-first (= top), sampled top-first, and the offscreen->backbuffer copy
//  is top-left aligned. No Y flip anywhere.
//
//  THE ACTUAL RISK IS OVER-CORRECTING: adding a "Metal->D3D flip" that isn't
//  needed vertically mirrors the whole game. Verify with the raster-bars demo:
//  gpsnap and confirm the copper bar that is defined for screen line 0 renders
//  at the TOP row. (See RISKS.md.)
//
// ---------------------------------------------------------------------------
// COLUMN- vs ROW-MAJOR: NOT APPLICABLE.
//  This engine has ZERO matrix uniforms. Every transform (the fullscreen
//  triangle, the sprite quad rotation/scale/scroll) is precomputed on the CPU
//  into clip-space float2s (sprites: gp_engine.mm:606-623) or is the hardcoded
//  big triangle. HLSL's default column-major float4x4 packing never bites here.
//
// ---------------------------------------------------------------------------
// CHANNEL ORDER: shaders return LOGICAL RGBA float4; the render-target FORMAT
//  (B8G8R8A8_UNORM offscreen/backbuffer, matching CAMetalLayer's BGRA default)
//  decides storage. The hardware swizzles on write, exactly as in Metal — no
//  shader change for BGRA. The BGRA->RGBA swap is done only in the gpsnap
//  readback on the CPU (gp_engine.mm:1409-1413).
// ============================================================================


// ============================================================================
// 1. INDEXED PANE      MSL: gp_engine.mm:17-43   (indexed_pane.rs:28-58)
//    Fullscreen triangle -> R8_UINT world index sample -> per-SCREEN-scanline
//    palette LUT. Index 0 = transparent (discard). 1..15 = per-line "copper"
//    (raster-locked, does NOT scroll). 16..255 = global.
//    Palette buffer layout (host, gp_engine.mm:230-233): viewport_h*16 float4
//    of per-line entries, then 240 float4 of globals.
// Bindings:  cbuffer b0 (16B) | SRV t0 R8_UINT index | SRV t1 palette
// ============================================================================
struct VOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

cbuffer IndexedUniforms : register(b0) {   // MSL: constant Uniforms& [[buffer(0)]]
    float scroll_x;                         // host uploads float[4] = 16 bytes
    float scroll_y;                         // (gp_engine.mm:431-433)
    float viewport_w;
    float viewport_h;
};

Texture2D<uint>          indexTex : register(t0);   // MSL texture(0), R8_UINT
StructuredBuffer<float4> palette  : register(t1);   // MSL buffer(1) constant float4*

VOut vs_indexed(uint vid : SV_VertexID) {
    float2 positions[3] = { float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0) };
    float2 pos = positions[vid];
    VOut o;
    o.pos = float4(pos, 0.0, 1.0);
    o.uv  = float2((pos.x + 1.0) * 0.5, 1.0 - (pos.y + 1.0) * 0.5);
    return o;
}

float4 ps_indexed(VOut input) : SV_Target {
    uint screenX = uint(input.uv.x * viewport_w);
    uint screenY = uint(input.uv.y * viewport_h);
    uint worldX  = uint(int(screenX) + int(scroll_x));
    uint worldY  = uint(int(screenY) + int(scroll_y));
    uint ci = indexTex.Load(int3(worldX, worldY, 0));   // MSL .read(uint2(...)).r
    if (ci == 0u) { discard; }                          // MSL discard_fragment()
    uint k;
    // per-line palette keys off SCREEN scanline (copper) — NOT world row.
    if (ci < 16u) { k = screenY * 16u + ci; }
    else          { k = uint(viewport_h) * 16u + (ci - 16u); }
    return palette[k];
}


// ============================================================================
// 2. SPRITES           MSL: gp_engine.mm:46-69   (sprites.rs:20-47)
//    Per-instance quad (4 verts, CPU-built strip incl. rotation/scale/scroll,
//    gp_engine.mm:606-623) -> per-FRAME R8_UINT texture -> per-def 16-colour
//    palette -> src-alpha over (blend state, not shader).
// Bindings:  VB {POSITION,TEXCOORD} | cbuffer b0 (alpha) | SRV t0 frame | SRV t1 pal
// Topology:  D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, Draw(4,0)
//
// DELTA — vertex feed: MSL pulls verts from a bound buffer via [[vertex_id]]
// (constant VIn* verts [[buffer(0)]], gp_engine.mm:52). D3D11 idiom = a real
// input-assembler vertex buffer. Primary design: a small DYNAMIC vertex buffer
// (64B), Map(WRITE_DISCARD)+memcpy the 4 verts per instance, IASetVertexBuffers,
// input layout {POSITION:R32G32_FLOAT@0, TEXCOORD:R32G32_FLOAT@8}. (Alt: keep
// the pull model with StructuredBuffer<VIn> indexed by SV_VertexID.)
// ============================================================================
struct SVIn {
    float2 pos : POSITION;      // clip-space xy, already transformed CPU-side
    float2 uv  : TEXCOORD0;
};
struct SVOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

cbuffer SpriteUniforms : register(b0) {     // MSL: struct Uniforms { float alpha; }
    float alpha;                             // host uploads 16B (alpha + 12B pad):
    float3 _pad_sprite;                      // cbuffers round up to 16 bytes.
};

Texture2D<uint>          spriteTex : register(t0);   // per-frame R8_UINT
StructuredBuffer<float4> spritePal : register(t1);   // 16 float4 (gp_engine.mm:513)

SVOut vs_sprite(SVIn v) {
    SVOut o;
    o.pos = float4(v.pos, 0.0, 1.0);
    o.uv  = v.uv;
    return o;
}

float4 ps_sprite(SVOut input) : SV_Target {
    uint w, h;
    spriteTex.GetDimensions(w, h);                     // MSL get_width()/get_height()
    uint2 texel = uint2(input.uv.x * float(w), input.uv.y * float(h));
    uint ci = spriteTex.Load(int3(texel, 0));
    if (ci == 0u) { discard; }
    float4 c = spritePal[ci];
    c.a *= alpha;
    return c;                                          // blend state does src-alpha over
}


// ============================================================================
// 3. TEXT OVERLAY      MSL: gp_engine.mm:118-133   (text_overlay.rs:23-42)
//    Fullscreen triangle sampling a viewport-sized RGBA8 CPU buffer, nearest,
//    src-alpha over everything.
// Bindings:  SRV t0 RGBA8_UNORM | SamplerState s0 (POINT + CLAMP)
//
// DELTA — the sampler: MSL declared it inline (constexpr sampler s(coord::
// normalized, filter::nearest), gp_engine.mm:131). SM5.0 has no static/inline
// samplers, so the host creates a SamplerState (D3D11_FILTER_MIN_MAG_MIP_POINT,
// ADDRESS_CLAMP) and binds it at s0 (PSSetSamplers).
// ============================================================================
struct TVOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

Texture2D    textTex : register(t0);        // RGBA8_UNORM -> Texture2D<float4>
SamplerState textSmp : register(s0);        // point + clamp, host-created

TVOut vs_text(uint vid : SV_VertexID) {
    float2 positions[3] = { float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0) };
    float2 pos = positions[vid];
    TVOut o;
    o.pos = float4(pos, 0.0, 1.0);
    o.uv  = float2((pos.x + 1.0) * 0.5, 1.0 - (pos.y + 1.0) * 0.5);
    return o;
}

float4 ps_text(TVOut input) : SV_Target {
    return textTex.Sample(textSmp, input.uv);   // MSL tex.sample(s, in.uv)
}


// ============================================================================
// 4. SHADER BACKGROUND — runtime template   MSL header: gp_engine.mm:135-148
//    The game supplies a pixel-shader BODY (fmain) compiled at runtime with
//    D3DCompile; this header supplies the fullscreen-triangle VS and the
//    Uniforms cbuffer. A compile error is returned as a string, never aborts
//    (GpShaderPane::compile, gp_engine.mm:880-890 -> capture D3DCompile's
//    ID3DBlob errorMsgs).
//
// DELTA — Uniforms naming: MSL bodies read u.time / u.aspect / u.p[i] via a
//   `constant Uniforms& u` argument (gp_engine.mm:140). HLSL cbuffer members
//   are GLOBAL — game bodies reference `time`, `aspect`, `p[i]` (drop `u.`).
//
// DELTA — array packing (CRITICAL): in an HLSL cbuffer, EACH array element
//   occupies its own 16-byte slot. `float p[8]` = 128 bytes, NOT 32. The host
//   must upload with a 16-byte stride per param (see GP_ENGINE_D3D_DESIGN.md
//   §5.5 for the exact 144-byte layout). The tightly-packed MSL host upload
//   (float[10], gp_engine.mm:907-911) MUST be re-laid-out.
//
// Game shader bodies are HLSL (rewritten from MSL); entry point `fmain`.
// ============================================================================
cbuffer ShaderUniforms : register(b0) {     // MSL: struct Uniforms{float time,aspect,p[8];}
    float  time;
    float  aspect;
    float2 _pad_shader;                      // fills the first 16-byte register
    float  p[8];                             // 16-byte stride per element (128B)
};

struct GVOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

GVOut vs_shader(uint vid : SV_VertexID) {    // the header's vmain (gp_engine.mm:141)
    float2 positions[3] = { float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0) };
    float2 pos = positions[vid];
    GVOut o;
    o.pos = float4(pos, 0.0, 1.0);
    o.uv  = float2((pos.x + 1.0) * 0.5, 1.0 - (pos.y + 1.0) * 0.5);
    return o;
}

// ---- EXAMPLE game body (appended after the header at runtime) --------------
// Demonstrates the HLSL body contract: reads `time`/`aspect`/`p[i]`, `input.uv`.
float4 fmain(GVOut input) : SV_Target {
    float2 uv = input.uv * 2.0 - 1.0;
    uv.x *= aspect;
    float d = length(uv) - (0.3 + 0.1 * sin(time + p[0]));
    float g = smoothstep(0.02, 0.0, abs(d));
    return float4(g, g * 0.6, 1.0 - g, 1.0);
}


// ============================================================================
// 5. DIRECT PANE       MSL: gp_engine.mm:1121-1139   (GAMEPANE_PLAN.md §6b)
//    256-colour palette shader over the game-written linear index framebuffer.
//    Opaque (the framebuffer is the whole picture).
// Bindings:  cbuffer b0 (w,h) | SRV t0 R8_UINT | SRV t1 palette(256)
// ============================================================================
struct DVOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

cbuffer DirectUniforms : register(b0) {     // MSL: struct U { float w; float h; }
    float dw;                                // host uploads 16B (w,h + 8B pad)
    float dh;
    float2 _pad_direct;
};

Texture2D<uint>          directTex : register(t0);
StructuredBuffer<float4> directPal : register(t1);   // 256 float4 (gp_engine.mm:1166)

DVOut vs_direct(uint vid : SV_VertexID) {
    float2 positions[3] = { float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0) };
    float2 pos = positions[vid];
    DVOut o;
    o.pos = float4(pos, 0.0, 1.0);
    o.uv  = float2((pos.x + 1.0) * 0.5, 1.0 - (pos.y + 1.0) * 0.5);
    return o;
}

float4 ps_direct(DVOut input) : SV_Target {
    uint x = uint(input.uv.x * dw);
    uint y = uint(input.uv.y * dh);
    if (x >= uint(dw)) x = uint(dw) - 1u;
    if (y >= uint(dh)) y = uint(dh) - 1u;
    return directPal[directTex.Load(int3(x, y, 0))];
}


// ============================================================================
// 6. COMPUTE BLITTER   MSL: gp_engine.mm:72-115   (blitter.rs:21-70)
//    Slot-to-slot rectangle ops over R8_UINT index textures. 16x16 threadgroups.
//    Host dispatch: Dispatch((w+15)/16, (h+15)/16, 1)  (matches gp_engine.mm:
//    735-738, which passes THREADGROUP counts to dispatchThreadgroups).
//
// DELTA — SRV/UAV NAMESPACE SPLIT (important): Metal binds src=texture(0),
//   dst=texture(1) in ONE namespace. D3D11 separates SRVs and UAVs: src is an
//   SRV at t0, dst is a UAV at u0 (NOT u1). Host binds:
//       CSSetShaderResources(0,1,&srcSRV);   // t0
//       CSSetUnorderedAccessViews(0,1,&dstUAV,NULL);  // u0
//   For blit_clear (no source) dst is still the UAV at u0.
//
// DELTA — HAZARD: after a blit, the dst texture is bound as a UAV. Before it is
//   sampled as an SRV in ps_indexed, the host MUST unbind u0 (bind NULL UAV) or
//   D3D11 nulls the SRV and warns. (D3D11 auto-inserts the UAV->SRV barrier.)
//
// DELTA — TYPED UAV LOAD (blit_minterm only, the hardest point): copy/
//   transparent/clear only STORE to dst (broadly supported on R8_UINT).
//   blit_minterm READS dst (`d = dst.read(dpos)`) -> a typed UAV LOAD of
//   R8_UINT, which D3D11 gates behind D3D11_FEATURE_D3D11_OPTIONS2::
//   TypedUAVLoadAdditionalFormats. See GP_ENGINE_D3D_DESIGN.md §5.3 for the
//   CPU-mirror fallback (already computed at gp_engine.mm:743-760).
// ============================================================================
cbuffer BlitParams : register(b0) {         // MSL BlitParams (gp_engine.mm:75-77)
    uint src_x, src_y, dst_x, dst_y;         // host packs 8x uint32 = 32 bytes,
    uint w, h, op, value;                    // identical to gp_engine.mm:722-725
};

Texture2D<uint>   blitSrc : register(t0);   // MSL access::read  -> SRV
RWTexture2D<uint> blitDst : register(u0);   // MSL access::write / read_write -> UAV

// --- 6a. blit_copy: dst = src  (dst STORE only) ----------------------------
[numthreads(16, 16, 1)]
void cs_blit_copy(uint3 gid : SV_DispatchThreadID) {
    if (gid.x >= w || gid.y >= h) return;
    uint v = blitSrc.Load(int3(src_x + gid.x, src_y + gid.y, 0));
    blitDst[uint2(dst_x + gid.x, dst_y + gid.y)] = v;
}

// --- 6b. blit_transparent: dst = src where src!=0  (dst STORE only) ---------
[numthreads(16, 16, 1)]
void cs_blit_transparent(uint3 gid : SV_DispatchThreadID) {
    if (gid.x >= w || gid.y >= h) return;
    uint v = blitSrc.Load(int3(src_x + gid.x, src_y + gid.y, 0));
    if (v == 0u) return;
    blitDst[uint2(dst_x + gid.x, dst_y + gid.y)] = v;
}

// --- 6c. blit_minterm: dst = src (AND|OR|XOR) dst  (dst LOAD+STORE) ---------
//   op==0 AND, op==1 OR, op==2 XOR, else copy (matches gp_engine.mm:104-107).
//   REQUIRES R8_UINT typed UAV load — gate at device init or use CPU fallback.
[numthreads(16, 16, 1)]
void cs_blit_minterm(uint3 gid : SV_DispatchThreadID) {
    if (gid.x >= w || gid.y >= h) return;
    uint s = blitSrc.Load(int3(src_x + gid.x, src_y + gid.y, 0));
    uint2 dpos = uint2(dst_x + gid.x, dst_y + gid.y);
    uint d = blitDst[dpos];                 // <-- typed UAV LOAD of R8_UINT
    uint result;
    if      (op == 0u) { result = s & d; }
    else if (op == 1u) { result = s | d; }
    else if (op == 2u) { result = s ^ d; }
    else               { result = s; }
    blitDst[dpos] = result;
}

// --- 6d. blit_clear: dst = value  (dst STORE only) -------------------------
//   MSL bound dst at texture(0); in D3D it is still the UAV at u0.
[numthreads(16, 16, 1)]
void cs_blit_clear(uint3 gid : SV_DispatchThreadID) {
    if (gid.x >= w || gid.y >= h) return;
    blitDst[uint2(dst_x + gid.x, dst_y + gid.y)] = value;
}


// ============================================================================
// 7. PRESENT BLIT (NEW — no MSL counterpart)
//    Metal renders offscreen (logical WxH) then MTLBlitEncoder-copies to the
//    CAMetalLayer drawable, and the LAYER does the nearest upscale + letterbox
//    (kCAFilterNearest, gp_engine.mm:1241,1360-1374). D3D11/DXGI has no layer
//    filter guarantee, so the crisp retro upscale is done explicitly: a
//    fullscreen point-sampled blit of the offscreen SRV into an aspect-correct
//    letterboxed VIEWPORT of the backbuffer (clear the bars to black).
//    gpsnap still reads the offscreen (logical) directly and is unaffected.
//    (If the swapchain is created AT logical size, a plain CopyResource works
//    but DWM upscaling is linear, not nearest — this shader is the crisp path.)
// Bindings:  SRV t0 offscreen BGRA | SamplerState s0 (POINT + CLAMP)
// ============================================================================
struct PVOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

Texture2D    presentTex : register(t0);
SamplerState presentSmp : register(s0);     // POINT + CLAMP -> crisp nearest

PVOut vs_present(uint vid : SV_VertexID) {
    float2 positions[3] = { float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0) };
    float2 pos = positions[vid];
    PVOut o;
    o.pos = float4(pos, 0.0, 1.0);
    o.uv  = float2((pos.x + 1.0) * 0.5, 1.0 - (pos.y + 1.0) * 0.5);
    return o;
}

float4 ps_present(PVOut input) : SV_Target {
    return presentTex.Sample(presentSmp, input.uv);
}
