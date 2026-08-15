# The GamePane on D3D11 — prior art, vendored for WG11

The author's words, when WG11's shader port was framed as new work: *"we port
the shaders from metal to directx in this case, and we have done that before"*.
This directory is that prior art, copied verbatim so WG11 builds against a
reference rather than a memory.

| file | from | what it is |
|---|---|---|
| `shaders.hlsl` | [albanread/WINDARTTALK](https://github.com/albanread/WINDARTTALK) `gamepane-design/` | **The complete Metal→HLSL translation of `gp_engine.mm`'s shaders**, line-cited against the MSL source: indexed pane with the per-scanline copper palette, sprites, text overlay, the runtime-compiled `fmain` background template, the compute blitter, the direct pane, and an explicit nearest-letterbox present pass. |
| `GP_ENGINE_D3D_DESIGN.md` | WINDARTTALK | The host-side design: device/swapchain/child-HWND, frame lifecycle, per-class translation, COM lifetime, fullscreen. §5.6 independently reaches this port's own UMA conclusion (`D3D11_FEATURE_D3D11_OPTIONS2`, upload as the portable path, persistent map as the UMA fast path). |
| `GAMEPANE_DESIGN.md` | WINDARTTALK `port-win/` | The earlier port-planning doc. |
| `wingui/*.hlsl` | [albanread/wingui](https://github.com/albanread/wingui) | Production HLSL from the wingui project: `text_grid.hlsl` (per-glyph quads over an atlas, with a CRT effects mode), `indexed_fill.hlsl` (compute fill + DDA line), `sprite.hlsl`, `rgba_blit.hlsl`, `vector.hlsl`, `graphics.hlsl`. |

## The traps the reference already paid for

Read `shaders.hlsl`'s header before writing any shader for WG11. The ones that
bite hardest:

- **No Y flip.** Metal and D3D11 agree on NDC (+1 = top) and texel origin
  (top-left); the famous flip is OpenGL/Vulkan. The risk is *over-correcting* —
  a flip that isn't needed vertically mirrors the whole game.
- **cbuffer array packing.** `float p[8]` occupies 128 bytes (each element its
  own 16-byte register), so the host upload for `shaderParam:value:` must use a
  16-byte stride — the tightly-packed Metal layout is wrong on D3D.
- **SRV/UAV namespace split** in the blitter (Metal's one texture namespace
  becomes `t0` + `u0`), the **UAV-unbind hazard** before the indexed pane
  samples a blitted texture, and the **typed-UAV-load gate**
  (`D3D11_OPTIONS2::TypedUAVLoadAdditionalFormats`) for minterm blits — with a
  CPU-mirror fallback already designed.
- **SM5.0 has no inline samplers** — every `constexpr sampler` becomes a
  host-created `SamplerState`.

## Relation to `winui_render/src/gpu.rs`

`gpu.rs` (WG10a) is the SHELL's renderer — cell grid + one pixel plane — and
was written before this reference was pulled; it independently matches the
reference's fullscreen-triangle VS, `Texture2D<uint>` + palette-lookup PS, and
point-sampled present, and already queries `D3D11_OPTIONS2` (for UMA). WG11's
GamePane implements THIS reference behind upstream's primitives, and `gpu.rs`
converges where it diverges (palette as `StructuredBuffer<float4>` with the
per-scanline copper split; index 0 = discard).
