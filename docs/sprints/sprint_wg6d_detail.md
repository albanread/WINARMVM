# Sprint WG6d — the Editor pane on DirectWrite: a cell grid the guest pokes

*A redesign of WG6c-1's rendering, demanded by the author after using it:
"we want a fast modern UI, using fast modern tech" — and, of the GDI text
path, a verdict this document agrees with and does not soften. Written before
the code.*

## The review: what is actually GDI here, and what failed

Five files touch GDI directly. They are not equally guilty:

| surface | GDI use | verdict |
|---|---|---|
| `106_winui_editorpane.mst` — the Editor pane | **per-glyph text layout**: measure a font, multiply columns, `ExtTextOutW` with a hand-built advance array, GDI caret | **replace.** This is the failure. Four shipped defects in two days, three wrong diagnoses, all one cause (below) |
| `94_winui_drawing.mst` — the bar cells | one glyph + one label per cell, `WM_DRAWITEM`, no layout arithmetic | keep for now. Static decoration at rest; nothing accumulates |
| `96_winui_splitter.mst` — the grab bar | a filled rect and a chevron | keep. It is a rectangle |
| `103_winui_ghost.mst` — the ghost line | one grey string in a STATIC | keep. One string, no grid |
| `91_winui_shell.mst` | scratch RECT helpers | keep. Plumbing, not drawing |

Everything else on screen is a stock control (EDIT, RichEdit, LISTBOX,
SysTreeView32) painting itself through comctl32's visual styles — not our
GDI, and not implicated.

**The one cause.** The Editor pane put TWO authorities in charge of the same
pixels: Smalltalk computed positions on a `column × charWidth` grid, and GDI
laid glyphs out its own way. Every defect this shipped was one of the two
drifting from the other:

* metrics measured off the paint DC — the grid's unit changed value depending
  on whether a paint had happened yet (a click landed at the end of the
  document);
* the document drawn relative to `rcPaint` — correct only for the full-client
  invalidations we generate ourselves, doubled text on the partial ones
  Windows generates;
* `DrawText` advancing by the font's own metrics inside a run while the runs
  were placed on our grid;
* `ExtTextOutW` handed a UTF-8 byte count where it wanted UTF-16 units — an
  out-of-bounds read that ASCII testing can never catch.

Four fixes later the author was still looking at smeared, doubled glyphs on a
plain 1920×1080 display. A design that produces a new failure mode per fix is
wrong at the seam, not at the call sites. GDI's text API is also genuinely a
1990s artifact: integer pen positions, no subpixel placement, no color-run
batching, a process-global caret, and `GetDC`/`ReleaseDC` discipline at every
touch. On Windows 11 ARM64 none of this is forced on us.

## The model, which is the author's

> "use a shader to display a monospaced font into a view, and update the
> memory for that view — load char+colour into the view and have the shader
> display it. All Smalltalk does then is poke values into a memory buffer."

This is the terminal-emulator architecture (Windows Terminal's own renderer
is exactly this), and it fits this port unusually well because **the view is
already stateless by construction**: WG6c-1's pane owns no text and repaints
the whole buffer every time, precisely because 60's incremental-damage
protocol drifted and was abandoned. A cell grid is that philosophy made
physical — the whole viewport, as memory.

The split kills the bug class rather than the bugs:

* **Smalltalk owns MEANING**: the rope, the tokenizer, the caret, the
  selection, input. It writes cells: *"row 2, column 4 is `$r`, teal on
  white."* No pixels, no fonts, no DCs — no way to disagree with the renderer
  about where a glyph goes, because it never says where a glyph goes.
* **The renderer owns PIXELS**: one DirectWrite font face, metrics taken from
  the font's design tables (not measured by drawing), Direct2D glyph runs
  onto a DXGI flip-model swapchain. It reads cells; it knows nothing about
  documents, syntax, selections or carets — a selection is just cells whose
  background differs.

## The seam

A new cdylib, **`winui_render`**, the exact shape `winui_host` proved: the
guest reaches it with the FFI's `library:` part (`LoadLibraryA` +
`GetProcAddress`, `g`-typed args only), Rust owns every resource behind
`#[no_mangle]` entry points. Same non-default workspace member, same
"downstream of the VM" position, no new VM mechanism at all.

```
MacvmRenderAttach(hwnd)            -> 0 ok / 1 err    device + swapchain + font for this pane
MacvmRenderDetach()                -> 0               release everything (pane destroyed)
MacvmRenderGrid(cols, rows)        -> cell buffer address, renderer-OWNED
MacvmRenderMetrics()               -> (cellW << 16) | cellH, physical px, from DWrite design metrics
MacvmRenderSetCaret(col, row, on)  -> 0               the caret is a CELL property now
MacvmRenderPresent()               -> 0 ok / 1 err    draw the grid, present
MacvmRenderLastError()             -> UTF-16 ptr      the winui_host message-slot pattern, verbatim
```

**The cell.** 12 bytes, little-endian, `cols × rows` of them:

```
u32 codepoint     the character (space = empty)
u32 fg            0x00BBGGRR — same COLORREF byte order the tokenizer's
u32 bg            palette already speaks, so WinSyntax needs no conversion
```

Per-cell **bg** is what makes the renderer ignorant on purpose: selection
highlight, find-highlights, even the ghost line are all just cells the guest
painted differently. Nothing about them crosses the seam.

**Who allocates.** The renderer. WinArena's 256 KiB would hold a small grid,
but the buffer's size is the renderer's business (it changes on resize and
DPI, wants alignment, and must outlive nothing) — the guest gets an address
and pokes it with the `Alien forAddress:` accessors WinArena already has.
Guest never frees; `MacvmRenderGrid` reallocates on new dimensions and
`MacvmRenderDetach` releases.

**Why poking is safe with no lock.** Paint and edit both happen on the UI
thread, through the same pump: the guest mutates cells inside a VM entry, the
present happens on `WM_PAINT` after that entry returns. There is no moment
when the renderer reads while the guest writes — the same quiescence argument
the door already makes for every VM entry.

## The paint path — one FFI call where forty GDI calls were

`WM_PAINT` keeps WG6c-1's shape: door → guest, synchronously, `BeginPaint`/
`EndPaint` in Rust. But the guest's whole handler becomes
`MacvmRenderPresent()` — one call. Layout, glyph runs, colors, caret: all on
the far side, in the renderer, from the buffer.

`refreshEditorPane` stays `InvalidateRect`-based and unchanged: edits mutate
cells, invalidate, and the present arrives through the pump — same
busy-guard discipline as today. (A later slice may move the present off the
guest entirely — the door could recognise the pane's hwnd and present without
a VM entry. The contract does not change; deliberately not v1.)

**The swapchain, not `ID2D1HwndRenderTarget`.** DXGI flip-model
(`DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL`, 2 buffers) with a D2D1.1 device context
drawing into the back buffer. It is ~60 lines more than the legacy hwnd
target and buys tear-free presents, a straight path to DirectComposition and
`IDXGISwapChain2::SetSourceSize` resize handling — the Windows Terminal
stack, first-class on ARM64, WARP fallback if there is somehow no GPU.

**Metrics from design tables, not from drawing.** `IDWriteFontFace::
GetMetrics` + `GetDesignGlyphMetrics('M')`: `advance × emSize ÷
designUnitsPerEm`, ceil to physical pixels; line height from ascent + descent
+ lineGap the same way. No DC, no `DT_CALCRECT`, no window needed — which
also makes the metrics **headlessly testable**, something the GDI path never
was. DPI: taken at `Attach`, re-derived on re-`Attach`; the shell re-attaches
on `WM_DPICHANGED` (already routed for layout).

## What dies, what stays

**Dies** (all of it in `106_winui_editorpane.mst`):
* the GDI paint — brushes, `ExtTextOutW`, the dx advance buffer, the
  `PaintOrigin` client-coordinate machinery, `SegDraws`;
* `ensureEditorMetrics`, `naturalWidthOf:`, `editorGlyphDrift:`,
  `utf16UnitsOf:` and the DPI-keyed metrics cache — the renderer's metrics
  are the only metrics;
* the Win32 caret block (`CreateCaret`/`SetCaretPos`/`ShowCaret`/
  `DestroyCaret`, `CaretOwned`, the `checkFocusChanged` reconciliation) — the
  caret is a cell property, the thread-global-caret pitfall stops existing;
* `editorSelectionRectsAtX:y:` — selection is background color at fill time.

**Stays untouched**, which is most of WG6c:
* the rope, tokenizer, `EditorSession` (60, 100);
* all of 107 — routing, keys, selection model, navigation, hit-testing
  (`editorOffsetAtX:y:` divides by `MacvmRenderMetrics` instead of the dead
  cache; still pure, still tested);
* 108 clipboard, 109 view, the door, the allowlist, the gates' input half.

**New guest code**, and it is small and pure: `fillEditorGrid` — walk the
tokenizer's runs once (the same walk the paint does today), write
codepoint+fg per cell, overwrite bg across the selection span, set the caret
cell. Pure over (text, runs, selection, caret) → testable headless against a
mock buffer, no window, no renderer.

## Slices

* **WG6d-1 — the renderer exists.** `winui_render` crate: device, swapchain,
  font face, metrics, grid, present. **Gate (headless, Rust):** render a
  known grid into a WIC bitmap target and assert cells — the 'M' cell has
  ink, the space cell has none, a red-bg cell is red, metrics are non-zero
  and stable. *This is the test GDI could never give us: a pixel assertion
  with no window.*
* **WG6d-2 — the pane presents through it.** Guest fills the grid, paint
  handler becomes one call, GDI paint + caret + metrics machinery deleted.
  **Gate:** `gate-wg6c` unchanged on the input half (typing, undo, click,
  drag-select, clipboard — none of it knows the renderer changed), minus the
  paint-origin/drift assertions (impossible by construction), plus renderer
  stats (frames presented, last error empty) and the screenshot.
* **WG6d-3 — polish, if profiling asks.** Caret blink, damage-based
  presents, DirectComposition, a glyph atlas. Explicitly not v1: 80×40 cells
  is ~3,200 glyphs and D2D batches color runs per row without help.

## Out of scope, decided rather than drifted into

* **The bar, splitter, ghost stay GDI.** They are rectangles and single
  strings drawn at rest; nothing accumulates and nothing has failed. If the
  cell renderer proves out and someone wants the bar on it later, that is a
  sprint with its own argument, not a side effect of this one.
* **CJK/IME**: unchanged from WG6c's decision — out for this release. The
  cell buffer takes codepoints, so the DATA path is ready; composition UI is
  the missing (large) piece, and wide glyphs would need double-cells.
* **Ligatures.** A cell grid deliberately has none — Cascadia Mono (not
  Cascadia Code) is already the face precisely because code columns should
  be columns.

## Pitfalls

* **Two copies of one DLL is split state.** `win_gui` must NOT link
  `winui_render` as an rlib while the guest `LoadLibraryA`s the cdylib — the
  statics (device, buffer) would exist twice and the pane would present from
  the copy nobody wrote to. The exe stays ignorant of the renderer; only the
  guest names it. (`LoadLibraryA` on an already-loaded module refcounts the
  same image, so repeated `library:` calls are safe — winui_host already
  relies on this.)
* **The swapchain must be created for the PANE's hwnd, not the shell's** — 
  presenting over the whole client would draw the editor across the picker.
  `Attach` takes the hwnd the guest already has.
* **Resize.** `ResizeBuffers` fails while the old back buffer is held;
  release the D2D target bitmap first, re-create after. The classic, named
  so it is not re-discovered.
* **`GetDesignGlyphMetrics` wants a glyph index, not a codepoint** —
  `GetGlyphIndices('M')` first. Feeding it the codepoint compiles and
  answers the metrics of whatever glyph 77 is, which for most faces is
  quietly wrong.
* **Cell coordinates are (col, row) everywhere in the seam.** The guest's
  `editorLineColOf:` answers (line, col) — swap at the one call site that
  writes the caret, and say so, or the caret walks the wrong axis. The kind
  of bug this project finds by gate, so the gate asserts the caret CELL.
