# Indexed Colour Sprites

Sprites in indexed colour mode are 16-colour, palette-driven, multi-frame GPU sprites rendered on top of indexed panes. Each pane maintains its own **sprite bank** — a registry of sprites identified by client-chosen integer ids. The GPU atlas and palette textures are shared across all panes and managed automatically by the runtime.

---

## Colour convention

| Index | Meaning |
|-------|---------|
| 0 | Transparent (always discarded by the shader) |
| 1–15 | Sprite's own palette colours |

Sprites are strictly 4-bit (16 colours per sprite). There is no access to the global indexed palette from sprite pixels — each sprite carries its own 16-colour palette row.

---

## Pixel format

Sprite pixel data is `R8_UINT` — one byte per pixel, values 0–15. Frames are laid out **left to right** in a single horizontal strip:

```
[ frame 0 | frame 1 | frame 2 | ... | frame N-1 ]
  frame_w    frame_w   frame_w         frame_w
  <-------- frame_w * frame_count pixels wide -------->
  <-- frame_h pixels tall -->
```

The total buffer size required is `frame_w * frame_count * frame_h` bytes.

---

## Sprite bank

Each pane (`SuperTerminalPaneId`) has its own sprite bank. Sprites within a bank are identified by a `SuperTerminalSpriteId` (a client-chosen `uint32_t`, 0 is invalid).

Sprites can be **redefined** at any time by calling `super_terminal_define_sprite` again with the same id. If the frame dimensions match the existing entry the same atlas region is reused; if dimensions change a new region is allocated.

The shared sprite atlas is 2048×2048 pixels and allocated with a shelf packer. Atlas space is **not reclaimed** when a sprite is redefined with the same dimensions — only the pixel and palette data are updated.

---

## API

### Define a sprite

```c
int32_t super_terminal_define_sprite(
    SuperTerminalClientContext* ctx,
    SuperTerminalPaneId         pane_id,
    SuperTerminalSpriteId       sprite_id,
    uint32_t                    frame_w,
    uint32_t                    frame_h,
    uint32_t                    frame_count,
    uint32_t                    frames_per_tick,
    void*                       pixels,
    void*                       palette,
    SuperTerminalFreeFn         free_fn,
    void*                       free_user_data);
```

| Parameter | Description |
|-----------|-------------|
| `pane_id` | The pane whose sprite bank receives this sprite. |
| `sprite_id` | Client-chosen id; reuse the same id to update an existing sprite. |
| `frame_w`, `frame_h` | Pixel dimensions of a single frame. |
| `frame_count` | Number of animation frames in the strip. |
| `frames_per_tick` | Animation speed: advance one frame every N `sprite_tick` increments. Pass `0` for a static sprite (always frame 0). |
| `pixels` | Pointer to the R8_UINT pixel strip (`frame_w * frame_count * frame_h` bytes). Ownership is transferred; freed via `free_fn` (or `delete[]` if null) after the data is copied. |
| `palette` | Pointer to a `WinguiGraphicsLinePalette` (16 × `WinguiGraphicsColour`). Index 0 is transparent. Ownership transferred same as `pixels`. |
| `free_fn` | Optional custom free function called with `free_user_data` for both `pixels` and `palette`. If null, `delete[]` is used. |

The function copies both buffers immediately before enqueueing, so the caller's originals are freed before the call returns.

Returns 1 on success, 0 on failure (check `wingui_last_error_utf8()`).

---

### Render sprites

```c
int32_t super_terminal_render_sprites(
    SuperTerminalClientContext*        ctx,
    SuperTerminalPaneId                pane_id,
    uint64_t                           sprite_tick,
    uint32_t                           target_width,
    uint32_t                           target_height,
    const SuperTerminalSpriteInstance* instances,
    uint32_t                           instance_count);
```

| Parameter | Description |
|-----------|-------------|
| `pane_id` | The pane to render sprites into. |
| `sprite_tick` | Animation clock. Pass `tick->frame_index` from `on_frame` for automatic animation. The displayed frame is `(sprite_tick / frames_per_tick) % frame_count`. |
| `target_width`, `target_height` | Logical screen size in pixels used for position scaling. Pass `0` to use the pane's current layout dimensions. |
| `instances` | Array of `SuperTerminalSpriteInstance`. Copied internally; the caller's array is not modified. |
| `instance_count` | Number of entries in `instances`. |

Call once per frame, typically from `on_frame` after updating game state. Can be called independently of `super_terminal_frame_indexed_graphics_upload` — sprites and the indexed background layer are separate draw calls that composite in order (background first, sprites on top).

Returns 1 on success, 0 on failure.

---

### Sprite instance fields

```c
typedef struct SuperTerminalSpriteInstance {
    SuperTerminalSpriteId sprite_id;       // which sprite from this pane's bank
    float x, y;                            // screen-space position (pixels)
    float rotation;                        // radians, clockwise
    float scale_x, scale_y;               // 1.0 = natural size; 0 treated as 1.0
    float anchor_x, anchor_y;             // 0..1 fraction of frame size; (0,0) = top-left
    float alpha;                           // 0.0..1.0; 0 treated as 1.0
    uint32_t flags;                        // WINGUI_SPRITE_FLAG_*
    uint32_t effect_type;                  // post-process effect (see below)
    float effect_param1, effect_param2;
    uint8_t effect_colour[4];              // RGBA effect tint
    uint32_t palette_override;             // 0 = use sprite's own palette
} SuperTerminalSpriteInstance;
```

**Flags** (`WINGUI_SPRITE_FLAG_*`):

| Flag | Value | Effect |
|------|-------|--------|
| `WINGUI_SPRITE_FLAG_VISIBLE` | `1 << 0` | Set automatically by the runtime; not needed in client code. |
| `WINGUI_SPRITE_FLAG_FLIP_H` | `1 << 1` | Mirror horizontally. |
| `WINGUI_SPRITE_FLAG_FLIP_V` | `1 << 2` | Mirror vertically. |
| `WINGUI_SPRITE_FLAG_ADDITIVE` | `1 << 3` | Additive blending (reserved; standard alpha-over used currently). |

**Effect types**:

| Value | Effect |
|-------|--------|
| 0 | None |
| 1 | Additive colour highlight: `colour.rgb += effect_colour.rgb * (effect_param2 * 0.3)` |
| 4 | Multiplicative tint: `lerp(colour.rgb, colour.rgb * effect_colour.rgb, effect_param1)` |
| 5 | Flash (binary): alternate between sprite colour and `effect_colour` at frequency set by `effect_param1` |

---

## Animation

Frame selection is performed entirely on the CPU before submission:

```
display_frame = (frames_per_tick > 0)
    ? (sprite_tick / frames_per_tick) % frame_count
    : 0
```

Pass `tick->frame_index` as `sprite_tick`. For a sprite with `frames_per_tick = 4` and `frame_count = 8`, one full animation cycle takes 32 ticks.

To pause animation at a specific frame, pass a constant `sprite_tick` or set `frames_per_tick = 0` and call `super_terminal_define_sprite` with `frame_count = 1` containing only the desired frame.

---

## Typical usage (hosted app)

```c
// --- setup ---
SuperTerminalSpriteId SPRITE_PLAYER = { 1 };

void on_setup(SuperTerminalClientContext* ctx, void* user_data) {
    SuperTerminalPaneId pane = ...;

    // Build a 16x16, 4-frame walk strip (64x16 total)
    uint8_t pixels[64 * 16];
    WinguiGraphicsLinePalette palette = {};
    // ... fill pixels and palette ...

    super_terminal_define_sprite(ctx, pane, SPRITE_PLAYER,
        16, 16,   // frame_w, frame_h
        4,        // frame_count
        6,        // frames_per_tick: advance every 6 frames (~10fps at 60fps)
        pixels, &palette, NULL, NULL);
}

// --- per frame ---
void on_frame(SuperTerminalClientContext* ctx,
              const SuperTerminalFrameTick* tick, void* user_data) {
    SuperTerminalPaneId pane = ...;
    SuperTerminalSpriteInstance inst = {};
    inst.sprite_id = SPRITE_PLAYER;
    inst.x = player_x;
    inst.y = player_y;
    inst.scale_x = inst.scale_y = 2.0f;
    inst.alpha = 1.0f;

    super_terminal_render_sprites(ctx, pane,
        tick->frame_index,  // sprite_tick drives animation
        0, 0,               // target_width/height: 0 = use pane size
        &inst, 1);
}
```

---

## Limits and constraints

| Item | Value |
|------|-------|
| Palette colours per sprite | 16 (4-bit indexed) |
| Colour index 0 | Always transparent |
| Shared atlas size | 2048 × 2048 pixels |
| Atlas allocation policy | Shelf packer; space not reclaimed on redefine unless dimensions grow |
| Max instances per render call | Unlimited (dynamic vertex buffer) |
| Palette rows (total sprites across all panes) | Configured by `sprite_max_palettes` in `WinguiIndexedGraphicsRendererDesc`; defaults to renderer maximum |
| Sprite bank scope | Per pane; sprite ids are local to a pane |
