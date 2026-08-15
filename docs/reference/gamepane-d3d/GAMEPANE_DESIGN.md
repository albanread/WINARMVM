# WINDART game pane — the RASM indexed-graphics model

The game pane implements the **RASM GamesCanvas** retro-graphics model (docs at
`E:\RASM\docs\gamecanvas.md` + `gpucanvas.md`), the same model used across the user's
compilers. "A 16-bit console with the brakes off": tile layers, hardware sprites,
palette banks, parallax, mode-7 — with the era's limits removed (unlimited sprites/
layers, fx shaders), all GPU-accelerated (Direct3D 11). Reuse: HLSL/DirectX from
`E:\multiwingui` (shaders + device plumbing) and WINDART's existing
`gp_engine_d3d.cpp`.

## The model (indexed soul, resolved on the GPU)

- **Indexed base**: a **640×360** `R8_UINT` index framebuffer (16:9; presents at
  1280×720 ×2, or 2560×1440 ×4 — integer-scaled, crisp nearest). Games read
  `CanvasW`/`CanvasH`, never hardcode.
- **Index 0 = 100% transparent** everywhere — the only transparency (binary).
- **Palette architecture** (the whole point — do NOT resolve to RGBA early):
  - **240 global colours**: indices **16..255** → a single global `256×1` BGRA LUT.
  - **16 colours per line**: indices **0..15** → *that scanline's* own 16-colour LUT
    (`perLine` = a `16×360` BGRA texture). Gradients, raster bars, split palettes.
  - **Per-sprite 16-colour LUT**: each sprite (each *frame*) carries its own 16-colour
    bank (`spritePal` = `16×slots`); a sprite pixel is `4-bit → that bank → RGB`.
  - Simultaneous colours multiply: `240 + 16·lines + 16·sprites` (thousands per frame
    from 4-/8-bit indices).
- **Deep-colour RGBA surface**: additionally, a full `R8G8B8A8_UNORM` surface for
  true-colour work (the fx layer is already RGBA; expose a first-class RGBA draw
  target too).

## The layer stack (back → front)

Per the user's order (fx behind, text on top):

| # | layer | kind | notes |
|---|---|---|---|
| 0 | **fx / shader bg** | RGBA, procedural pixel shader | plasma / tunnel / blackhole |
| 1 | **tile layer 1** | indexed tile layer | own scrollX/scrollY (parallax) |
| 2 | **tile layer 2** | indexed tile layer | own scroll |
| 3 | **indexed pixels** | indexed free-draw (buffers 0/1) | Cls/FillRect/Line/Text into the buffer |
| 4 | **sprites** | indexed, per-sprite palette | colour-0 transparent, **unlimited** |
| 5 | **text** | font atlas | HUD, top |

Each layer is a 640×360 render target (or procedural); drawn/resolved through its
palette at composite time, blended back-to-front into the back buffer (optional affine
transform = mode-7). (RASM's reference order has the free-draw *graphics* layer below
the tiles; WINDART follows the user's order — indexed pixels above the tiles.)

## Buffers + the blitter (the workhorse)

- **8 index buffers**: **0/1** = the displayed indexed-pixels layer (double-buffer /
  flip); **2..7** = assets + temporary/scratch data.
- **Blitter functions** copy rectangular index blocks in/out of buffers → **"blitter
  objects"** (rectangular data, *not* sprites — no per-sprite LUT, just index blocks).
  Modes: **copy**, **colour-key** (index 0 / a key transparent), **alpha**. On the GPU
  the blitter is a textured quad (src SRV → dst RT); it's `library/blit.was` reborn as
  a draw call — every layer op (stamp a tile, composite a sprite, flatten a layer)
  goes through it.
- **Asset textures** (uploaded once): tile atlas, sprite atlas, font atlas — all
  `R8_UINT`.

## GPU pipeline (D3D11)

```
per frame:  bump scroll / sprite-pos / palette constants (CPU) → draw calls only
  fx RT (RGBA procedural)                    → composite
  tile layer 1/2 (R8_UINT worlds + scroll)   → palette-LUT resolve → composite
  indexed-pixels buffer 0/1 (R8_UINT)        → palette-LUT resolve → composite
  sprites (atlas + per-sprite LUT, batched)  → composite (colour-0 discard)
  text (font atlas)                          → composite
  → present (autoflip swapchain, 720p/1440p)
```
Palette resolve in the pixel shader (HLSL):
```hlsl
// background pixel, index c, scanline sy:
col = (c < 16u) ? perLine.Load(int3(c, sy, 0)) : global.Load(int3(c, 0, 0));
// sprite pixel, index c, palette slot s:
if (c == 0u) discard;   col = spritePal.Load(int3(c, s, 0));
```
D3D11 specifics: `Texture2D<uint>` sampled with `.Load()` (integer fetch, no filter);
POINT+CLAMP sampler; fullscreen-triangle VS from `SV_VertexID` + `Draw(3,0)`; composite
via `OMSetRenderTargets` to a layer RT / the back buffer; `Present()`.

## gpApply command protocol (Dart → engine)

The Dart side describes a frame as a command list (like the view-server). Target
shape (superset of the existing protocol — see the gap analysis):
- **palette**: `setGlobal(idx,rgb)`, `setLine(row,idx,rgb)`, `setSpritePal(slot,idx,rgb)`,
  `cyclePalette(...)`.
- **buffers**: `selectBuffer(n)`, `cls(idx)`, `pset/hline/vline/fillRect/line/circle/
  text` (into the current buffer), `blit(srcBuf,srcRect,dstBuf,dstXY,mode)`.
- **tiles**: `defineTileset(...)`, `setLayerMap(layer,map,w,h)`, `setLayerScroll(layer,
  x,y,rate)`.
- **sprites**: `defineSprite(id,frames)`, `setSpriteFrame(id,f)`, `drawSprite(id,x,y,
  slot)`.
- **fx**: `setFxShader(name/params)`.
- **rgba**: `rgbaSurface(...)` for the deep-colour path.
- **present** / **snapshot** (BMP/PNG readback).

## Gap analysis — the existing pane is ~80% of the model

The existing `gp_engine_d3d.cpp` (T3) is **structurally aligned**: the hard, defining
RASM pieces are built and shader-correct. What remains is *breadth*, not foundation.

| RASM feature | Status | Note |
|---|---|---|
| R8_UINT indexed base, palette resolved IN the pixel shader | ✅ HAS | the whole point — stays indexed end-to-end |
| index 0 transparent everywhere | ✅ HAS | `ci==0 → discard` in indexed + sprite PS |
| per-line 16-colour LUT (0..15) | ✅ HAS | palette `[H*16 + 240]`; keyed by `screenY` (true copper) |
| global 240 (16..255) | ✅ HAS | `gppal`, resolved in-shader |
| per-sprite 16-colour LUT | ✅ HAS | `GpSpriteDef.palette[16]`, own StructuredBuffer |
| graphics / free-draw indexed layer | ✅ HAS | pset/line/fill/circle/disc/cls/load |
| unlimited sprites | ✅ HAS | `std::vector`, no cap (but 1 draw call each — no batching) |
| fx / procedural RGBA layer | ✅ HAS | `GpShaderPane`, runtime HLSL `fmain`, time/aspect/p[8] |
| 8-buffer pool (0/1 displayed, 2..7 assets) | ✅ HAS | matches the spec exactly; `gpactive`/`gpswap` |
| present to flip-model swapchain + PNG readback | ✅ HAS | offscreen BGRA → DXGI flip, letterboxed; WIC snap |
| **3 tile layers (bg0/1/2) + parallax + overscan** | ❌ LACKS | *biggest gap* — one indexed layer, one global scroll |
| **GPU blitter (copy/key/alpha)** | ⚠️ PARTIAL | exists but CPU mirror+reupload; copy/key/AND/OR/XOR/clear, **no alpha**, no named "blitter objects" |
| **font-atlas text** | ⚠️ PARTIAL | CPU 7-segment raster (digits ok, letters draw as boxes) |
| **game-drawable deep RGBA surface** | ⚠️ PARTIAL | RGBA is only the composite target + fx/text, not a draw surface |
| fixed 640×360 base | ⚠️ PARTIAL | parametric (demos run 424×240), not fixed |

Current `gpApply` protocol (Dart → engine, one list/frame, `render_present()` at end):
palette `gppal`/`gplinepal`/`gpspritepal`; buffers `gpactive`/`gpswap`/`gpcls`/`gppset`/
`gpline`/`gpfill`/`gpcircle`/`gpdisc`/`gpload`/`gpscroll`/`gpblit`(modes 0 copy·1 key·
2 AND·3 OR·4 XOR·5 clear); sprites `gpsprite`/`gpframe`/`gpspawn`/`gpplace`/`gphide`/
`gpanim`; text `gptext`/`gptextclear`; fx `gpshader`/`gpparam`; audio `gpsound`/`gpplay`.
Demos drive it via `gamepane.dart` (a retained-scene serializer flushing `['draw',cmds]`
per tick) through `game_live.dart` (on-screen) / `gp_runner.dart` (headless PNG).

**Implication:** steps 3–4 (a Game tab + a Demos menu running the existing demos —
invaders, brickout, plasma, mandelbrot, …) are achievable **now on the existing engine**.
The RASM gaps below are enhancements to *fully* reach the model, sequenced after.

## Reuse map (multiwingui — ~80–90% liftable)

All reusable GPU code is in `E:\multiwingui\src\wingui.cpp` + `shaders\*.hlsl`
(`native_ui.cpp`/`terminal.cpp` have none — skip). Nearly every gap has a direct lift:

| WINDART piece | multiwingui source | Action |
|---|---|---|
| device + swapchain + present + resize + readback | `wingui_create_context`, `_present`, `_resize_context`, `readBackTextureBgra` (+WIC BMP/PNG) | **LIFT** |
| index→palette-LUT resolver (the present shader) | `graphics.hlsl` `graphics_fragment` (`<16 ? perLine : global(240)`, index 0 → transparent) + `ensureIndexedGraphicsTextures` (R8_UINT, Map full-frame) | **ADAPT** (BGRA pack, 256/16×360 split) |
| **font-atlas text** (fixes the 7-seg gap) | `text_grid.hlsl` + `wingui_create_text_grid_renderer` (alpha atlas, per-cell fg/bg, `DrawInstanced`) | **LIFT + ADAPT** (retarget to text layer) |
| **GPU blitter** copy/alpha/colour-key | `rgba_blit.hlsl` + `wingui_rgba_surface_shader_blit`; index-space key via `indexed_blit_cs` | **LIFT** |
| indexed sprite pass, per-sprite LUT | `sprite.hlsl` + `createSpriteResources` (16×slots LUT, index-0 key, rot/flip/fx) | **LIFT + ADAPT** |
| **RGBA draw surface / layer RT** | `createRgbaPaneBufferResource` (SRV+RTV) + `vector.hlsl` SDF primitives | **LIFT** |
| GPU compute index draw (fill/line/blit) | `createIndexedSurfaceBuffer` + `indexed_fill.hlsl` (3 CS kernels) | LIFT (optional) |

**Net-new (not in either codebase):** (1) the **back-to-front layered compositor loop**
that owns the 6 stacked layer RTVs (fx → graphics → 3 tiles → sprites → text) — multiwingui
composites straight to the backbuffer, WINDART's pane composites to an offscreen but has
only one indexed layer; (2) **3 independent indexed tile layers** (instantiate the indexed
surface 3× + parallax scroll — the scroll-wrap is already in `graphics_vertex`); (3) the
BGRA-vs-packed-RGBA palette swap + the exact 256-global / 16×360-per-line split.

**Conclusion:** WINDART's pane supplies the correct indexed spine; multiwingui supplies a
font atlas, a GPU blitter with alpha, RGBA/vector primitives, and the layer RT primitive.
The gap work (tiles, GPU blitter, font atlas, RGBA surface) is mostly **assembly of lifted
parts** into the layered compositor — not net-new graphics research.

## Phased plan (RASM G0–G6, adapted to WINDART)

Each phase independently buildable + verified by a **BMP/PNG readback** (correct render)
AND a **live pane** (device/swapchain/present path), per RASM §8.

- **G0 — Spine**: D3D11 device + swapchain in the pane child HWND; upload a 640×360
  `R8_UINT` test pattern + a global palette LUT; the palette-LUT pixel shader (global +
  per-line); fullscreen present; snapshot readback. *Proves the whole spine.*
- **G1 — Buffer pool + blitter**: the 8-buffer pool (alloc by handle) + the GPU blit
  (copy/colour-key/alpha) + the layer-RT compositor skeleton.
- **G2 — Tile layers + scroll**: tile atlas; 2 indexed tile layers as world buffers;
  per-layer scroll → parallax/overscan (constants, zero redraw).
- **G3 — Sprites**: sprite atlas + per-sprite palette + frames; the batched sprite pass
  (unlimited, colour-0 transparent).
- **G4 — Text + fx + compositor**: font atlas → text layer; the procedural fx shader
  slot; wire the full stack; the RGBA surface.
- **G5 — Parity + a real game**: expose the API to Dart; run an existing demo/game
  (brickout / invaders) on the pane unchanged; add them to the Game tab + Demos menu.
- **G6 — Bonuses**: per-layer affine (mode-7); 1440p pixel-double; GPU palette cycling;
  a showcase (thousands of sprites + fx bg + mode-7 floor).

## Integration with the shell (steps 3–4)

- A **Game tab** hosts the `game` widget (the D3D pane) — a native tab page (already
  built), so the pane lives on its own page and is hidden/shown by the OS.
- A **Demos / Games menu** (menu bar submenu) lists the demos/games; selecting one runs
  its `update`/`render` loop against the pane. Demos already exist
  (`test/demos/*.dart` — bounce, lissajous, mandelbrot, plasma, copper, pong, invaders,
  brickout); G5 retargets them onto the pane.

This is a multi-phase build (a full retro-graphics engine); G0 (the spine) is the first
verifiable milestone.
