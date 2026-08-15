# The WINARM shell, by example

Every fenced `smalltalk` block below is **executed by `just gate-cookbook`**
against a live window. Not checked for syntax — run, in the shell, with the
result asserted. A block that raises fails the gate.

That is the whole reason this file exists in this form. Documentation examples
rot silently: the API moves, the prose stays, and the first person to find out
is someone typing a snippet that no longer works. These cannot rot without
turning red first.

Paste any of them into the **Workspace** and press `Ctrl-D`.

---

## The cell grid

Three views run on it already — Editor, Monitor, Debugger — and the contract is
the same for all of them: **the guest owns meaning, the renderer owns pixels.**
You write codepoint + foreground + background into a buffer. You never compute
where a glyph goes.

Ask a pane for its grid and write a row:

```smalltalk
| hwnd cw ch cols rows base |
hwnd := WinShell canvasPaneHwnd.
cw := WinRender cellWidthFor: hwnd.
ch := WinRender cellHeightFor: hwnd.
cols := ((WinShell canvasSize at: 1) // cw) max: 1.
rows := ((WinShell canvasSize at: 2) // ch) max: 1.
base := WinRender gridFor: hwnd cols: cols rows: rows.
WinRender clear: hwnd to: 16r000000 on: 16rFFFFFF.
'hello from the world' doWithIndex: [ :chr :i |
    WinRender at: base col: i - 1 row: 0 cols: cols
        put: chr asInteger fg: 16rC00000 bg: 16rFFFFFF ].
WinRender present: hwnd
```

**`cols` and `rows` are derived from the pane, never assumed.** A resize changes
them, and the buffer address changes with them — which is why `gridFor:` is
called per frame rather than cached. Colours are COLORREF (`16rBBGGRR`), the
same byte order `WinSyntax`'s palette already speaks.

## The pixel plane

`docs/sprints/upstream_review_2026-08-12.md` (SM0). The guest stores BGRA words
straight into the buffer the renderer draws — no command, no marshalling, no
per-frame allocation.

A red square, by hand:

```smalltalk
| hwnd base stride |
hwnd := WinShell canvasPaneHwnd.
base := WinPixels planeFor: hwnd width: 160 height: 120.
stride := WinPixels strideFor: hwnd.
0 to: 119 do: [ :y |
    0 to: 159 do: [ :x |
        WinPixels at: base x: x y: y stride: stride
            put: (WinPixels bgraR: 200 g: 30 b: 30) ] ].
WinShell renderCanvas
```

**Ask for the stride.** It is `w * 4` today and the guest is still forbidden to
say so: a pixel plane is the one place in this port where the guest computes an
address, and the shape has to come from the side that owns the buffer. That
discipline is what the cell grid gets for free by never letting the guest near a
pixel at all.

## The palette

SM4. An indexed plane is one byte per pixel naming a slot in a 256-entry table.
Re-colouring the whole screen costs **the palette's size, not the screen's**.

```smalltalk
| hwnd ix stride pal |
WinShell canvasMode: #palette.
hwnd := WinShell canvasPaneHwnd.
WinPixels planeFor: hwnd width: 160 height: 120.
ix := WinPixels indexPlaneFor: hwnd.
stride := WinPixels indexStrideFor: hwnd.
0 to: 119 do: [ :y |
    0 to: 159 do: [ :x |
        WinPixels indexAt: ix x: x y: y stride: stride put: (x + y) // 2 ] ].
pal := WinPixels paletteFor: hwnd.
0 to: (WinPixels paletteLenFor: hwnd) - 1 do: [ :i |
    WinPixels palette: pal at: i put: (WinPixels bgraR: i g: 255 - i b: 128) ].
WinRender present: hwnd
```

Now animate it without touching a single pixel — 256 stores per frame:

```smalltalk
| hwnd pal n |
hwnd := WinShell canvasPaneHwnd.
pal := WinPixels paletteFor: hwnd.
n := WinPixels paletteLenFor: hwnd.
1 to: 30 do: [ :frame |
    0 to: n - 1 do: [ :i |
        | v |
        v := (i + (frame * 8)) bitAnd: 255.
        WinPixels palette: pal at: i put: (WinPixels bgraR: v g: 255 - v b: 128) ].
    WinRender present: hwnd ]
```

The index buffer's stride is `w`, **not** `w * 4` — one byte per pixel. Writing
it with `at:x:y:stride:put:` instead of `indexAt:x:y:stride:put:` stores four
pixels at a time and looks exactly like a stride bug.

## Composing text over pixels

A cell whose background is `WinPixels transparentBackground` is **not filled** —
the plane underneath shows through, and the glyph still draws. That is the whole
mechanism; there is no layer system and no compositor.

```smalltalk
| hwnd cols rows base |
hwnd := WinShell canvasPaneHwnd.
cols := ((WinShell canvasSize at: 1) // (WinRender cellWidthFor: hwnd)) max: 1.
rows := ((WinShell canvasSize at: 2) // (WinRender cellHeightFor: hwnd)) max: 1.
base := WinRender gridFor: hwnd cols: cols rows: rows.
WinRender clear: hwnd to: 16rFFFFFF on: WinPixels transparentBackground.
'over the pixels' doWithIndex: [ :chr :i |
    WinRender at: base col: i - 1 row: 2 cols: cols
        put: chr asInteger fg: 16rFFFFFF bg: 16r303030 ].
WinRender present: hwnd
```

Note the label's own cells get an opaque background. White-on-transparent is
legible over a still image and unreadable over a cycling palette — there is no
ink colour that contrasts with a field whose colours are the thing moving.

## Files

```smalltalk
WinFile spit: WinShell tempDirectory , 'winarm-note.txt'
    contents: 'written by the VM, through CreateFileW'
```

Answers a Boolean. On failure `WinFile lastError` says why — checked rather than
assumed, because a silent write failure downstream of File In looks exactly like
a file-in that did nothing.

## The Monitor and the Debugger

Both render onto cell panes, so their columns line up by construction rather
than by a font's good manners.

```smalltalk
| widest |
WinShell monitorVmCount.
WinShell monitorRefreshes.
widest := WinShell monitorColumnsFitting: 200.
WinShell appendTranscript: 'monitor: ' , WinShell monitorVmCount printString
    , ' VMs, ' , widest size printString , ' columns fit'
```

The Monitor's rows arrive from the host, one per running VM. `monitorColumnsFitting:`
is the interesting one — columns are shed **narrowest first** when the pane is
too small, so what survives a squeeze is what you most needed to see.

```smalltalk
WinShell haltReport
```

`haltReport` answers `RUNNING …` when nothing is parked. To see the Debugger
front a real halt, evaluate something that fails **in the primary** — the
Debugger view's own empty state tells you how.

---

## Why these run rather than merely appear

`just gate-cookbook` starts the shell, extracts every block above in order, and
evaluates each one wrapped in a handler that records failures. It then asserts
the failure list is empty.

Wrapped rather than unhandled deliberately: an unhandled error in one example
would abort the run and hide every example after it, so the gate would report
one broken snippet when there might be five. Recording and continuing means one
run tells you about all of them.
