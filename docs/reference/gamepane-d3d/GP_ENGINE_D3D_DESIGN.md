# GP_ENGINE_D3D_DESIGN — the game pane on Direct3D 11

The design for `gp_engine_d3d.cpp`, the Windows/D3D11 replacement for
`gp_engine.mm` (the Metal engine). This is the S6 spec: it maps every Metal
object to its D3D11 counterpart, preserves the `gpApply` verb semantics exactly,
and keeps the wire (`gp_natives.mm`, the 7-native shape) **unchanged** — only
the engine beneath the boundary changes.

- **Reference (READ-ONLY):** `e:\windart\MACDARTV1\macdart\cocoa\gamepane\gp_engine.{h,mm}`, `gp_natives.mm`, `gp_synth.{h,cc}`.
- **Shaders:** `shaders.hlsl` (this directory) — the MSL→HLSL translation, cited per-shader.
- **Toolchain (SPRINTS.md):** **MSVC 19.50 (VS 2026)** — no clang-cl. D3D11 + `D3DCompile` (HLSL SM5.0). Libs already on the link line (`WINDOWS_PORTING_PLAN.md` §4): `d3d11 dxgi d3dcompiler xaudio2 dcomp`.
- **Host context (S4):** the game pane attaches to a child HWND materialized by the view-server (`e:\windart\gui-design\win_view.h`, `WidgetKind::kCanvas`); `Win_gpOpen/Close/Apply/Snap/Stat/Fullscreen/Backbuffer` is "the same 7-native shape as cocoa" (`S4_GUI_HOST_DESIGN.md` §2.2, lines 309-313).

---

## 1. What is preserved (the invariants S6 must not break)

From `GAMEPANE_PLAN.md` and the code, these are load-bearing and unchanged:

1. **The wire is atomic.** A whole frame crosses in one `gpApply(cmds)` call,
   applied best-effort, **presented once at the end** (`gp_natives.mm:131-402`).
   This kills MACVM's mid-frame-present flicker class. D3D11 preserves it
   naturally: all commands go to the immediate context, `Present` once in
   `render_present()`.
2. **Offscreen-first.** Every layer renders into a persistent offscreen RT, then
   is copied to the swapchain. Two reasons (gp_engine.h:9-13): `gpsnap` reads
   honest pixels from the offscreen at any time, and a lost/occluded swapchain
   never loses the frame.
3. **Layer order:** shader bg → indexed pane → sprites → text overlay
   (`render_present`, gp_engine.mm:1340-1358). Direct mode: direct pane → text.
4. **Range-validate at the boundary, throw not abort** (`gp_natives.mm`). The
   engine setters trust their callers; `gp_natives.mm` is unchanged and keeps
   doing the validation.
5. **Fixed logical resolution, letterboxed, nearest upscale** (crisp retro).
6. **The direct framebuffer is length-bounded external typed data** (§6b) — the
   game physically cannot scribble past its buffer.

---

## 2. Object model map (Metal → D3D11)

Expanded from `WINDOWS_PORTING_PLAN.md` §6, with the concrete D3D11 types.

| Metal (gp_engine.mm) | Direct3D 11 | Notes |
|---|---|---|
| `MTLCreateSystemDefaultDevice()` (:1231) | `D3D11CreateDevice` → `ID3D11Device` + `ID3D11DeviceContext` | request FL 11_0+; `BGRA_SUPPORT` flag |
| `[device newCommandQueue]` (:1236) | (implicit) `ID3D11DeviceContext` (immediate) | D3D11 has no separate queue object |
| `[queue commandBuffer]` (:1330) | (no object) immediate context | `begin_frame()` becomes a near-no-op (§8) |
| `CAMetalLayer` (:1237) | `IDXGISwapChain1` (flip-model) on the child HWND | `DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL`; opt. `IDCompositionDevice` for a composed layer |
| `[layer nextDrawable]` / `presentDrawable` (:1360,1374) | `swapChain->GetBuffer(0)` + `Present(1,0)` | vsync on (1) matches the ~30fps pull cadence |
| offscreen `MTLTexture` BGRA (:1259-1266) | `ID3D11Texture2D` (B8G8R8A8_UNORM) + `ID3D11RenderTargetView` + `ID3D11ShaderResourceView` | `BIND_RENDER_TARGET \| BIND_SHADER_RESOURCE` |
| `MTLBlitCommandEncoder` copy offscreen→drawable (:1362-1373) | present-blit draw (`shaders.hlsl` §7) or `CopyResource` | draw = crisp nearest upscale + letterbox |
| `R8Uint` index texture (:199-210) | `ID3D11Texture2D` (DXGI_FORMAT_R8_UINT) + SRV (+ UAV for blit slots) | `USAGE_DEFAULT`, `BIND_SHADER_RESOURCE \| BIND_UNORDERED_ACCESS` |
| `replaceRegion:withBytes:` (:411-414,846,496) | `UpdateSubresource(tex,0,NULL,bytes,rowPitch,0)` | DEFAULT-usage upload path |
| `MTLBuffer` palette (float4*) (:232,513,1166) | `ID3D11Buffer` (STRUCTURED, stride 16) + SRV `StructuredBuffer<float4>` | see §5.1; cbuffer-array is a small-palette alt |
| `setFragmentBytes:` inline uniforms (:433,628,911) | `ID3D11Buffer` (`BIND_CONSTANT_BUFFER`) → `cbuffer` | 16-byte-multiple sizes; §5.5 packing |
| `newRenderPipelineStateWithDescriptor:` (:188) | `ID3D11VertexShader`+`ID3D11PixelShader`+`ID3D11InputLayout`+`ID3D11BlendState` | pipeline is split into discrete state objects |
| `newComputePipelineStateWithFunction:` (:650) | `ID3D11ComputeShader` | `CSSetShader` + `Dispatch` |
| render pass `loadAction Clear/Load` (:425,597,901) | `ClearRenderTargetView` (Clear) vs no-clear (Load) | Load = just don't clear before drawing |
| MSL `newLibraryWithSource:` (:155) | `D3DCompile(src, ... "vs_5_0"/"ps_5_0"/"cs_5_0", ...)` | runtime compile; errors via `ID3DBlob` |
| blend descriptor src-alpha over (:179-186) | `ID3D11BlendState` (SRC_ALPHA / INV_SRC_ALPHA, ADD) | one shared "alpha-over" blend state |
| `getBytes:` readback (:1389) | `CopyResource`→STAGING `ID3D11Texture2D` + `Map(READ)` | §7 gpsnap |
| `Dart_NewExternalTypedData` over shared `MTLBuffer` (:246,1180) | external typed data over a host buffer ring + upload-on-present | §6; **semantic change flagged** |
| `enterFullScreenMode:` (:1321) | borderless child→top-level style swap (or DXGI fullscreen) | §9 |

---

## 3. Device, swapchain, and the child HWND

`GpEngine::ensure_device` (gp_engine.mm:1229-1246) creates device + queue + a
`CAMetalLayer`-hosting NSView. The D3D11 shape:

1. **`ensure_device()`** — `D3D11CreateDevice(NULL, HARDWARE, ...,
   D3D11_CREATE_DEVICE_BGRA_SUPPORT [| DEBUG], FL{11_1,11_0}, ...,
   &device_, &featureLevel_, &ctx_)`. Store `ID3D11Device* device_`,
   `ID3D11DeviceContext* ctx_` (the immediate context = the "queue"). Query
   `IDXGIFactory2` (via `IDXGIDevice`→adapter→`GetParent`). **Probe the blitter
   format caps here** (§5.3): `CheckFeatureSupport(D3D11_FEATURE_D3D11_OPTIONS2,
   ...)` for `TypedUAVLoadAdditionalFormats`, and `CheckFormatSupport2(R8_UINT)`
   for typed UAV load/store. Store `bool r8_uav_load_ok_`.

2. **The child HWND.** Metal's `open()` returns an `NSView*` (as an int64
   handle, gp_natives.mm:121) that the workspace embeds in the Demos tab. On
   Windows the analogue is a **child HWND** created under the Demos-tab host
   window (a `WS_CHILD | WS_CLIPSIBLINGS` static/owner-draw window, or the
   `kCanvas` widget HWND the view-server already materializes — S4). `Win_gpOpen`
   returns that `HWND` as the int64 handle. The swapchain is created on it.

3. **Swapchain** — `IDXGIFactory2::CreateSwapChainForHwnd(device_, hwnd, &desc,
   ...)` with `DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL`, `BufferCount=2`,
   `Format=DXGI_FORMAT_B8G8R8A8_UNORM` (matches the mac's BGRA default),
   `Scaling=DXGI_SCALING_STRETCH`, `AlphaMode=IGNORE`. Backbuffer size = the
   child HWND client size (the offscreen is logical size; §7 upscales). Call
   `factory->MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)` to keep our own
   fullscreen control (§9). Optionally compose via `IDCompositionDevice` +
   `IDCompositionVisual` for a layered pane (the plan lists `dcomp`); not
   required for MVP.

4. **Resize.** The mac defers live resize (`GAMEPANE_PLAN.md` §0). Windows can
   match: on `WM_SIZE` of the child, `ResizeBuffers` the swapchain to the new
   client size and re-create the backbuffer RTV; the offscreen (logical) is
   untouched, so §7's letterbox just recomputes. Keep it minimal for S6.

---

## 4. Frame lifecycle & the immediate-context model

Metal builds a fresh `MTLCommandBuffer` per frame; blit verbs encode into it
mid-apply; `render_present` composites + `presentDrawable` + `commit`.

D3D11's immediate context is persistent — **there is no per-frame command buffer
to commit.** Translation:

- **`begin_frame()`** (gp_engine.mm:1328-1331) → near-no-op: reset per-frame
  transient state (e.g. rewind the sprite dynamic-VB write cursor). No object.
- **`gpblit` during apply** → issues `CSSetShader`+`Dispatch` on the immediate
  context immediately (see `GpBlitter::blit`, §5.3). Because the dispatch and the
  later render both run on the immediate context in submission order, D3D11's
  automatic hazard tracking inserts the UAV-write → SRV-read barrier for free.
- **`render_present()`** → issues the layered draws, then the present-blit, then
  `swapChain->Present(1,0)`. One present. `frame_cb_ = nil` has no analogue.
- **Atomicity** is preserved: all of a frame's dispatches and draws are queued
  before the single `Present`, exactly as the one-list-one-present wire requires.

> **Optional (perf, not MVP):** a deferred `ID3D11DeviceContext` per frame +
> `FinishCommandList`/`ExecuteCommandList` would reintroduce an explicit
> "command buffer" object if the architect wants closer structural parity, but
> it buys nothing here and adds a threading contract. Recommend the immediate
> context.

---

## 5. Per-class translation

### 5.1 GpIndexedPane (the 8-bit framebuffer + per-scanline palette)

State (gp_engine.h:39-86): 8 world-sized R8_UINT textures + CPU mirrors + dirty
flags; one flat `float4` palette buffer of `viewport_h*16 + 240` entries;
`{scroll_x, scroll_y}` and `active`/front/back identities.

- **Textures** — 8 × `ID3D11Texture2D` R8_UINT, `USAGE_DEFAULT`,
  `BIND_SHADER_RESOURCE | BIND_UNORDERED_ACCESS` (blit slots need the UAV). One
  `ID3D11ShaderResourceView` per texture; one `ID3D11UnorderedAccessView` per
  texture (created lazily / for all, cheap). CPU mirrors stay `std::vector<uint8_t>`
  unchanged (all the CPU drawing — `pset/line/circle/fill_rect/cls/load` — ports
  verbatim; it's pure C++).
- **Upload** (`upload()`, gp_engine.mm:401-417) → for each dirty slot,
  `ctx_->UpdateSubresource(tex[i], 0, NULL, mirror[i].data(), world_w, 0)`
  (rowPitch = world_w for R8). Clear dirty. `UpdateSubresource` is the DEFAULT-
  usage equivalent of `replaceRegion:`. (Do NOT use a DYNAMIC texture — a
  texture cannot be both `USAGE_DYNAMIC` and a UAV; DEFAULT+UpdateSubresource is
  the only shape that serves both the CPU upload and the blit UAV.)
- **Palette buffer** — the per-scanline "copper" mechanism. Host allocates an
  `ID3D11Buffer`, `MISC_BUFFER_STRUCTURED`, `StructureByteStride=16`,
  `ByteWidth=(viewport_h*16 + 240)*16`, `USAGE_DYNAMIC`, `CPU_ACCESS_WRITE`,
  `BIND_SHADER_RESOURCE`; SRV of type `StructuredBuffer<float4>` at t1. On
  `palette_dirty_`, `Map(WRITE_DISCARD)` + memcpy the `std::vector<float>`
  palette (same layout as gp_engine.mm:403). **This is the whole per-scanline
  palette trick, unchanged:** `set_line_rgb` writes entry `line*16 + index`
  (gp_engine.mm:279), `set_rgb` writes `viewport_h*16 + (index-16)`
  (gp_engine.mm:268), and `ps_indexed` keys `k = screenY*16 + ci` off the SCREEN
  scanline (`shaders.hlsl` §1). **Why a StructuredBuffer, not a cbuffer:** the
  entry count scales with viewport height (viewport_h up to 2048 → up to 33008
  float4 = 515 KB), far past the 64 KB cbuffer limit. StructuredBuffer has no
  such cap and is the natural map for `constant float4*`.
- **Render** (`render()`, gp_engine.mm:419-438) → set RTV (offscreen); if Clear,
  `ClearRenderTargetView(black)`, else skip; `IASetPrimitiveTopology(TRIANGLELIST)`,
  no VB/IL; `VSSetShader(vs_indexed)`, `PSSetShader(ps_indexed)`; update+bind the
  16-byte `IndexedUniforms` cbuffer (b0) with `{scroll_x, scroll_y, viewport_w,
  viewport_h}`; `PSSetShaderResources(0,1,&frontIndexSRV)` (t0),
  `PSSetShaderResources(1,1,&paletteSRV)` (t1); `Draw(3,0)`. **Renders the FRONT
  slot** regardless of `active` (gp_engine.mm:434), unchanged.
- **swap_buffers** (gp_engine.mm:248-258) → swap the front/back *identities*
  (texture ptr, SRV ptr, UAV ptr, CPU mirror, dirty flag) — pointer swaps, no
  content copy. Verbatim.
- **HAZARD:** if a slot was just a blit UAV target and is now the front SRV, the
  UAV must be unbound (`CSSetUnorderedAccessViews(0,1,&nullUAV,NULL)`) before the
  render binds it as SRV. `render_present` does this once after the last blit.

### 5.2 GpSprites

State (gp_engine.h:92-128): per-def `frames` (R8_UINT textures), 16-float4
palette + its buffer, instances with transform.

- **Frame textures** — `ID3D11Texture2D` R8_UINT, `USAGE_IMMUTABLE`, initial data
  = the parsed rows (gp_engine.mm:493-501, `MakeFrameTexture`), `BIND_SHADER_RESOURCE`,
  one SRV. Uploaded once at definition, never GPU-written → IMMUTABLE is ideal.
  `ParseSpriteRows` (gp_engine.mm:457-491) is pure C++, verbatim.
- **Per-def palette** — a 16-element `StructuredBuffer<float4>` (256 bytes), or a
  `cbuffer float4[16]` (fits easily under 64 KB — either is fine here; §5.1 keeps
  StructuredBuffer for one code path). Lazy upload at render top
  (gp_engine.mm:588-593) via `Map(WRITE_DISCARD)`.
- **Vertex feed** — the per-instance quad (gp_engine.mm:606-623) is built CPU-side
  into 4 `{pos.xy(clip), uv.xy}` verts. Primary design: one **DYNAMIC vertex
  buffer** (`ByteWidth>= max_instances*4*16`, `USAGE_DYNAMIC`, `BIND_VERTEX_BUFFER`,
  `CPU_ACCESS_WRITE`) + input layout `{POSITION R32G32_FLOAT @0, TEXCOORD
  R32G32_FLOAT @8}`. Per visible instance, `Map(WRITE_NO_OVERWRITE)` appended (or
  `WRITE_DISCARD` for the first of the frame), then `Draw(4, offset)` with
  `TRIANGLESTRIP`. Rotation/scale/scroll math (gp_engine.mm:606-623) is pure C++,
  verbatim. (Alt: keep the MSL "pull" model with `StructuredBuffer<VIn>` +
  `SV_VertexID`; the VB path is more idiomatic D3D11.)
- **Blend** — the shared "alpha-over" `ID3D11BlendState` (SRC_ALPHA/INV_SRC_ALPHA,
  ADD) bound for the sprite and text passes. Matches gp_engine.mm:179-186.
- **Per draw** — set the 16-byte `SpriteUniforms` cbuffer (b0) = `{alpha}`; bind
  frame SRV (t0), palette SRV (t1); `Draw(4, off)`. `tick()`/`hit()`/`place()`
  pure C++, verbatim.

### 5.3 GpBlitter (the compute blitter — hardest single point, §RISKS)

Four kernels → `cs_blit_copy/transparent/minterm/clear` (`shaders.hlsl` §6),
each an `ID3D11ComputeShader` compiled from the same source via `D3DCompile`
with distinct entry points. `blit()` (gp_engine.mm:686-761) ports as:

1. **CPU clipping** (gp_engine.mm:694-703) — verbatim (the plan-mandated fix vs
   the Rust panic). Pure C++.
2. **`pane->upload()`** before the GPU reads (gp_engine.mm:707) — verbatim (both
   sides current).
3. **Dispatch** — pick the CS; bind `BlitParams` cbuffer (b0) = the 8 uint32
   struct (gp_engine.mm:722-725, identical bytes); bind src SRV at **t0** and dst
   UAV at **u0** (namespace split, `shaders.hlsl` §6 delta); `Dispatch((w+15)/16,
   (h+15)/16, 1)`. For clear, no src.
4. **CPU-mirror the op** (gp_engine.mm:743-760) — verbatim; keeps `pget` truthful,
   then `set_dirty(dst,false)`.
5. **Unbind the dst UAV** (`CSSetUnorderedAccessViews(0,1,&null,NULL)`) so the
   later render can bind it as SRV (§5.1 hazard).

**The typed-UAV-load gate.** `cs_blit_minterm` reads dst (AND/OR/XOR) — a typed
UAV load of R8_UINT, gated behind `TypedUAVLoadAdditionalFormats` (§3 probe).
copy/transparent/clear only STORE (broadly supported). **Decision:**

- **Fast path (r8_uav_load_ok_ == true, the modern-Win11 norm):** dispatch all
  four kernels on the GPU as above.
- **Fallback (cap absent OR modes 2/3/4 only):** *skip the compute dispatch for
  min-term and rely on the CPU-mirror result.* Step 4 already computes the exact
  AND/OR/XOR into the CPU mirror; simply mark `dst` dirty (don't clear it) so the
  next `upload()` re-pushes the correct destination via `UpdateSubresource`. This
  is nearly free, is always correct, and reuses code that must exist anyway. copy/
  transparent/clear can still run on the GPU (store-only). **Recommendation:** ship
  the fallback as the baseline (min-term on CPU + reupload) and treat the GPU
  min-term as an optimization enabled only when the cap is present — the blitter is
  not the hot path for the shipping games (Pong/Invaders/Brickout drive the
  indexed pane + copy/transparent blits, per `GAMEPANE_PLAN.md` §5b).

### 5.4 GpTextOverlay

State (gp_engine.h:152-171): viewport-sized RGBA8 CPU buffer, seven-seg digits.

- **Texture** — `ID3D11Texture2D` R8G8B8A8_UNORM, `USAGE_DYNAMIC`,
  `CPU_ACCESS_WRITE`, `BIND_SHADER_RESOURCE`, one SRV. `upload()` (gp_engine.mm:
  844-851) → `Map(WRITE_DISCARD)` + memcpy the whole `rgba_` (row-by-row if the
  mapped `RowPitch != w*4`). All the CPU rasterization (`draw_text`, seven-seg
  segments, placeholder boxes; gp_engine.mm:790-842) is pure C++, verbatim.
- **Sampler** — one `ID3D11SamplerState` (`FILTER_MIN_MAG_MIP_POINT`,
  ADDRESS_CLAMP) at s0 (replaces the MSL inline `constexpr sampler`).
- **Render** (gp_engine.mm:853-866) → Load action (no clear), alpha-over blend,
  `vs_text`/`ps_text`, bind SRV t0 + sampler s0, `Draw(3,0)`.

### 5.5 GpShaderPane (runtime-compiled background)

- **compile()** (gp_engine.mm:880-890) → `D3DCompile(header + "\n" + body, ...,
  "vs_shader"/"fmain", "vs_5_0"/"ps_5_0", flags, &blob, &errBlob)`. On failure,
  return `string((char*)errBlob->GetBufferPointer())` — a logged Dart error,
  never an abort (matches the MSL error-as-string contract; the game-side
  `gpshader` verb surfaces it, gp_natives.mm:309-312). Build the VS from the
  header once; recompile the PS per `gpshader`.
- **Uniforms cbuffer** (`shaders.hlsl` §4) — `{time, aspect, _pad(8), p[8]}`.
  **CRITICAL packing:** HLSL puts each `p[]` element in its own 16-byte slot, so
  the host must upload a **144-byte** layout, NOT the tight 40-byte MSL layout
  (gp_engine.mm:907-911). Exact bytes:
  ```
  offset 0   : time        (float)
  offset 4   : aspect      (float)
  offset 8   : pad         (8 bytes)
  offset 16  : p[0]        (float)  + 12 pad
  offset 32  : p[1]        (float)  + 12 pad
  ... p[i] at offset 16 + i*16 ...
  offset 128 : p[7]        (float)  + 12 pad   (total 144, a 16-byte multiple)
  ```
  `set_param(i,v)` (gp_engine.mm:892-894) writes `params_[i]`; the render packs
  into this layout. `set_aspect`, `start_time_` unchanged (use `QueryPerformance
  Counter` for `CACurrentMediaTime()`).
- **Game bodies are HLSL** (rewritten from MSL): reference `time`/`aspect`/`p[i]`
  as bare cbuffer globals (drop MSL's `u.` prefix), entry `fmain(GVOut input) :
  SV_Target`.

### 5.6 GpDirectPane (the zero-copy direct framebuffer — semantic change flagged)

State (gp_engine.h:242-265): 3 shared `MTLBuffer`s of R8 indices, each with a
linear R8_UINT texture view, sampled by a 256-colour palette shader; `write_`
rotated on thread 0; exposed to Dart as external typed data.

**The Apple design relies on unified memory:** `MTLStorageModeShared`
`contents()` is CPU-writable memory the GPU samples directly (gp_engine.mm:
1156-1163), so `Dart_NewExternalTypedData` over `contents()` is true zero-copy.
**Discrete Windows GPUs have no shared memory**, and D3D11 forbids the exact
"buffer viewed as a texture that the CPU persistently writes and the GPU
samples" shape (a `USAGE_DYNAMIC` resource's Map pointer is valid only between
Map/Unmap; Dart writes at arbitrary times, even off-thread per §6b). **So the
zero-copy becomes host-write-then-upload-on-present:**

- **3 host ring buffers** — plain heap allocations (`stride*h`, stride = `w`
  rounded to a chosen alignment; on Windows *we* own the buffer so alignment is
  our choice — keep the stride reported by `gpstat` truthful, gp_natives.mm:
  420-432). `backbuffer_ptr()` returns `buffers_[write_].data()`; a STABLE
  pointer across frames. `Dart_NewExternalTypedData(kUint8, ptr, stride*h)` wraps
  it, length-bounded, no finalizer — identical wire (gp_natives.mm:440-451,
  unchanged). Several isolates aliasing the same buffer (§6b) works the same.
- **One GPU SRV texture** — `ID3D11Texture2D` R8_UINT, `USAGE_DEFAULT`,
  `BIND_SHADER_RESOURCE`. At `present_render()` (gp_engine.mm:1191-1212):
  `UpdateSubresource(tex, 0, NULL, buffers_[write_].data(), stride, 0)` uploads
  the just-written buffer, then draw `vs_direct`/`ps_direct` with the palette
  StructuredBuffer (256 float4, `Map(WRITE_DISCARD)` on `pal_dirty_`). Then
  `write_ = (write_+1) % 3` (gp_engine.mm:1211).
- **The triple-buffer ring is still meaningful:** it decouples the off-thread
  writers (worker isolates computing bands, §6b) from the present-time upload —
  the writer advances to a fresh buffer while present reads the previous one.
  (On Apple the ring hid GPU read latency; on Windows it hides the CPU→GPU upload
  read. Same 3-deep discipline.)
- **FLAG for the architect:** this is the one place the port is NOT
  byte-identical to Metal — it trades unified-memory zero-copy for one CPU→GPU
  upload per present. The *wire and the Dart-facing contract are unchanged*; the
  cost is a `stride*h` upload per frame (e.g. 424×176 = 75 KB — negligible).
  Optional fast path: on GPUs reporting `UnifiedMemoryArchitecture`
  (`D3D11_FEATURE_D3D11_OPTIONS2`), a persistently `Map`-able resource could skip
  the copy; the portable design does the upload.

---

## 6. render_present() — the composited frame

Port of gp_engine.mm:1333-1380. Immediate context, in order:

**Retained mode** (gp_engine.mm:1346-1358):
1. `sprites->tick(dt)`; `pane->upload()`; `text->upload()` (dt via QPC).
2. Set RTV = offscreen. If `shader->ready()`: `shader->render` (Clear black),
   then `pane->render(Load)`; else `pane->render(Clear)`.
3. `sprites->render(Load)` (alpha-over blend on).
4. `text->render(Load)`.

**Direct mode** (gp_engine.mm:1340-1345):
1. `text->upload()`; `direct_pane->present_render(offscreen)` (Clear);
   `text->render(offscreen, Load)`.

**Present** (gp_engine.mm:1360-1376) → §7. Then `frames_++`; `music->poll()`
(§GP_AUDIO_DESIGN). Before the first SRV bind, **unbind any blit UAVs** (§5.1).

---

## 7. Present + gpsnap (honest readback)

**Present.** Metal blits offscreen→drawable and the layer upscales (nearest,
letterbox). D3D11:
- Get backbuffer: `swapChain->GetBuffer(0, IID_PPV_ARGS(&bb))`; RTV over it.
- Compute the letterbox viewport: fit logical `w×h` into the client size
  preserving aspect (integer-scale-then-center is even crisper); clear the
  backbuffer black; `RSSetViewports(letterbox)`; draw `vs_present`/`ps_present`
  sampling the offscreen SRV with the POINT sampler (`shaders.hlsl` §7).
- `Present(1, 0)` (vsync — matches the pull cadence; the pull pacer is the frame
  driver, `GAMEPANE_PLAN.md` §3, so no CADisplayLink analogue is needed).
- If the swapchain is instead created at logical size, `CopyResource(bb,
  offscreen)` works but DWM upscale is linear — the present-blit is the crisp
  path and is recommended.

**gpsnap** (gp_engine.mm:1382-1423) — reads the OFFSCREEN (logical), so it is
independent of the swapchain and always honest:
1. `CopyResource(staging, offscreen)` where `staging` is a matching
   `USAGE_STAGING`, `CPU_ACCESS_READ`, B8G8R8A8_UNORM texture (create once,
   reuse).
2. `Map(staging, 0, MAP_READ, ...)`, copy row-by-row honoring `RowPitch`.
3. BGRA→RGBA swap (gp_engine.mm:1409-1413), then **WIC** PNG encode
   (`IWICImagingFactory` → `CreateEncoder(GUID_ContainerFormatPng)` → frame →
   `WritePixels` → `WICPixelFormat32bppRGBA`) to `path`. (WIC replaces
   `NSBitmapImageRep`; `windowscodecs` is already on the link line.)
4. The regress suite's headless open→define→place→tick→gpsnap→assert flow
   (`GAMEPANE_PLAN.md` §4) is unchanged.

---

## 8. Shared state objects (create once, reuse)

- **BlendState** "alpha-over": `SRC_ALPHA / INV_SRC_ALPHA`, `OP_ADD`, RGB+A
  (gp_engine.mm:179-186). Also an "opaque" default (no blend) for indexed/
  shader/direct/present.
- **RasterizerState**: `CULL_NONE`, `FILL_SOLID`, `FrontCounterClockwise`
  irrelevant (fullscreen tri / strips, no culling). `ScissorEnable=FALSE`.
- **DepthStencilState**: depth off (no depth buffer anywhere; 2D compositing).
- **SamplerState**: one POINT+CLAMP, shared by text and present.
- **Shaders/InputLayout**: compile all fixed shaders (indexed/sprite/text/direct/
  present + 4 CS) at `open()`; the game shader recompiles on `gpshader`.

---

## 9. Fullscreen

`set_fullscreen` (gp_engine.mm:1316-1326) lifts the view into a fullscreen
window; the layer keeps its logical drawableSize so the whole screen is one
crisp nearest upscale. Windows options (keep S6 simple):
- **Borderless-window fullscreen (recommended):** reparent/re-style the child
  HWND to a top-level `WS_POPUP` covering the monitor (`MONITORINFO` rect),
  `ResizeBuffers` the swapchain to the monitor size; §7's letterbox gives the
  crisp upscale; exit restores the child style + parent. No mode switch, alt-tab
  friendly. (`MakeWindowAssociation(..., DXGI_MWA_NO_ALT_ENTER)` keeps DXGI out
  of it.)
- Avoid exclusive `SetFullscreenState` for MVP (mode-switch flicker, focus
  hazards). `fullscreen()` bookkeeping bit unchanged.

---

## 10. Lifetime / COM discipline

MRC `[x release]` maps to COM `->Release()` / ComPtr. Per
`WINDOWS_PORTING_PLAN.md` §5, the GC-finalizer refcount discipline maps onto COM
AddRef/Release. Recommendation: use `Microsoft::WRL::ComPtr` for all D3D/DXGI/WIC
objects inside `gp_engine_d3d.cpp` (RAII, no manual release), so `close()`
(gp_engine.mm:1288-1301) becomes "reset the ComPtrs / delete the panes"; the
device/context/swapchain/view persist across open/close exactly as the mac keeps
device+queue+view. `GpEngine::instance()` singleton (gp_engine.mm:1216-1220)
unchanged. The direct-pane host buffers are freed only in the pane dtor (called
from `close()`), and the game isolate is dead first (§6b lifetime footgun,
designed out) — unchanged.

---

## 11. gpApply verb → engine call (semantics preserved, unchanged wire)

`gp_natives.mm` is **not modified**. Every verb keeps calling the same engine
method; only the method bodies are D3D11. Confirming the mapping the port must
honour (gp_natives.mm:191-389):

| Verb | Engine call | D3D11 change |
|---|---|---|
| `gppal` i r g b | `pane->set_rgb` | palette StructuredBuffer entry (§5.1) |
| `gplinepal` line i r g b | `pane->set_line_rgb` | per-scanline entry (§5.1) — copper preserved |
| `gpsprite`/`gpframe`/`gpspritepal` | `sprites->define/add_frame/set_rgb` | IMMUTABLE frame tex + palette (§5.2) |
| `gpspawn`/`gpplace`/`gphide`/`gpanim` | `sprites->place`/instance fields/`tick` | CPU-side, verbatim |
| `gpscroll` | `pane->set_scroll` | cbuffer uniform (§5.1) |
| `gpactive`/`gpswap` | `pane->set_active`/`swap_buffers` | pointer-identity swap (§5.1) |
| `gpcls`/`gppset`/`gpline`/`gpfill`/`gpcircle`/`gpdisc` | `pane->*` | CPU mirror + dirty (verbatim) |
| `gpload` slot b64 | `pane->load` | CPU mirror + dirty (verbatim); base64 decode in natives |
| `gpblit` mode src dst … | `blitter->blit` | compute dispatch / CPU fallback (§5.3) |
| `gptext`/`gptextclear` | `text->draw_text`/`clear` | DYNAMIC RGBA8 (§5.4) |
| `gpshader`/`gpparam` | `shader->compile`/`set_param` | runtime D3DCompile (§5.5) |
| `gpsound`/`gpplay` | `sfx->define`/`play` | XAudio2 (GP_AUDIO_DESIGN) |
| `gptune`/`gpmusic` | `music->define`/`control` | winmm midiStream (GP_AUDIO_DESIGN) |
| `gpdpal` (direct) | `direct_pane->set_pal` | palette StructuredBuffer (§5.6) |
| `gpfull` | `set_fullscreen` | borderless window (§9) |

`gpstat` (open, frames, w, h, fullscreen, direct, stride) and `gpBackbuffer`
(external typed data) unchanged (§5.6). The best-effort first-error semantics
(gp_natives.mm:391-401) are wire-side, unchanged.

---

## 12. Open decisions for the S6 architect

1. **Blitter min-term policy** — ship CPU-fallback baseline, GPU min-term as a
   cap-gated optimization? (Recommended: yes, §5.3.)
2. **Direct-pane upload** — accept the one CPU→GPU upload per present as the
   portable baseline (recommended), or special-case UMA GPUs? (§5.6.)
3. **Sprite vertex feed** — dynamic VB + input layout (recommended) vs the MSL
   `SV_VertexID` pull model? (§5.2.)
4. **Present** — explicit letterbox blit shader (recommended, crisp) vs
   logical-size swapchain + DWM stretch (simpler, linear)? (§7.)
5. **Composition** — plain HWND swapchain (MVP) vs `DComp` composed visual (the
   plan lists `dcomp`; only needed if the pane must alpha-compose with other
   view-server widgets)? (§3.)

See `RISKS.md` for the ranked hardest translation points.
