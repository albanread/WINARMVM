# Sprint WG6c — the Editor, on the rope

*Supersedes the WG6c paragraph in `sprint_wg6_detail.md`, which assumed a
RichEdit editor. Written before the code.*

## Why this changed

WG6c was going to be a RichEdit view over `class_source`. The author asked
whether we should write a rope-based editor in Smalltalk with its own syntax
highlighter instead. The research says yes, and says the work is much smaller
than the question implies — **two of the three pieces already exist**:

| piece | state |
|---|---|
| the rope | **built** — `world/60_editor.mst`: a persistent balanced tree of immutable fragments, with a newline index for O(log n) line lookup, and `TextBuffer` as a list of roots so **undo is free**. Pinned by a differential test against a `String` oracle (`world/tests/43_rope_tests.mst`), passing today. |
| the highlighter | **built** — WG5c D1/D2, deliberately control-agnostic. That sprint recorded the reversibility as the reason to take RichEdit first. |
| `EditorSession` | **built** — `handleKey:at:`, `handleCommand:`, `replaceFrom:length:with:`. |

`docs/editor_design.md` already carries the whole design as M0–M6, and M2 is
done.

**The hard problem is already solved, by abandonment.** The original design had
an incremental `EditorDamage` line-range protocol; 60's own note records that
it *did drift*, and it was replaced by blasting the whole buffer plus the
caret offset on every change. So **the view is stateless** — it owns no text
and keeps no line bookkeeping, and therefore cannot fall out of sync. A class
fits on screen, so the whole buffer is the viewport.

Building WG6c on RichEdit and then replacing it would be writing the Editor
twice, which is the rework this project has consistently refused.

## The architecture, and the one thing that forced it

A custom pane has to **paint** and has to **receive keys**. Two candidate
routes, and the obvious one does not work:

* **An `SS_OWNERDRAW` STATIC** — paints beautifully through the `WM_DRAWITEM`
  path WG4 D3 built and WG5c D5 reused, with a ready HDC and rect and no
  `BeginPaint` needed. And it CAN hold focus: `SetFocus` on a STATIC works,
  measured, `GetFocus` answers it. But a STATIC's WndProc is the SYSTEM's, and
  only `WM_COMMAND`/`WM_NOTIFY`/`WM_DRAWITEM` are forwarded to a parent —
  `WM_KEYDOWN` and `WM_CHAR` are not. The pane would be focusable and mute.
* **A child of OUR OWN registered class** (`MacvmWinUiShell_*`). Its WndProc
  is `macvm_wndproc` — the door — so every message reaches the existing
  allowlist, and `WM_KEYDOWN`/`WM_CHAR` are already on it. **This is the
  route.**

It costs one core change: **`WM_PAINT` joins the allowlist**, handled
SYNCHRONOUSLY rather than flag-and-drained.

That is a departure from §2.4a and it needs its argument, in the shape WG4 D5
used to admit `WM_MOUSEMOVE`:

1. **It cannot be deferred.** A flagged paint is a paint that did not happen,
   and Windows re-sends `WM_PAINT` until the region is validated — a drain-
   later scheme spins forever or blanks the pane.
2. **There is precedent, and it is the same shape.** `WM_DRAWITEM` already
   runs Smalltalk synchronously inside the door (`perform_drawitem`), for
   exactly this reason: drawing needs the DC that exists only during the
   message. `WM_PAINT` follows it — the door does `BeginPaint`, hands the
   guest an HDC and a rect, and does `EndPaint`.
3. **It is scoped to windows we own.** Only children of our class generate a
   `WM_PAINT` that reaches the door at all; stock controls paint themselves.

## What is NOT in scope, decided rather than deferred silently

* **IME / CJK composition.** The author's call: *"we won't support CJK for
  this release, we will create a special language release if requested."*
  Recorded here so the gap is a decision with a name on it rather than an
  omission someone finds later.
* **Accessibility.** Custom-drawn text is invisible to a screen reader without
  UIA. This is a real loss against a stock control and it is not addressed by
  this sprint. Stated plainly for the same reason.
* **Viewport windowing.** 60's note already scopes it: a class fits on screen,
  so the whole buffer is the viewport. Large-document scrolling is later work.

## The slices

* **WG6c-1 — the pane paints.** A child of our class, `WM_PAINT` through the
  door, rendering `EditorSession`'s text with WG5c's tokenizer and palette,
  plus a real Win32 caret. Read-only at this point: it displays a document and
  nothing types into it. *Gate: the text is on screen, coloured, with a caret,
  and it survives being uncovered.*
* **WG6c-2 — it accepts input.** `WM_CHAR`/`WM_KEYDOWN` to `handleKey:at:`,
  mouse hit-testing x/y → offset for caret placement and selection, clipboard.
  *Gate: typing changes the document and undo restores it — which costs
  nothing, because the rope is persistent.*
* **WG6c-3 — the Editor view.** A class picker, whole-class source loaded from
  `Image::class_source`, Save through the now-shared
  `flows::persist_editor_class`, plus File In and Add to World from the WG6
  row. *Gate: an edited class round-trips, and the parse gate refuses a bad
  one without changing anything.*

## Pitfalls

* **`BeginPaint` must be matched by `EndPaint`, on every path including a
  raise.** A guest error inside the paint handler that skipped `EndPaint`
  would leave the region invalid and Windows would re-send `WM_PAINT`
  immediately — a spin that looks like a hang. WG4 D3's own drawing handler
  already captures errors rather than propagating them (`DrawError`); this
  needs the same, and the `EndPaint` in an `ensure:`.
* **Caret ownership is per-thread and global.** `CreateCaret` replaces
  whatever caret exists; `DestroyCaret` on focus loss. Getting this wrong
  leaves a caret blinking in a pane that does not have focus, which reads as
  two cursors.
* **The tokenizer counts in the guest String's units.** Same boundary rule
  WG5a and WG5c D4 both record. With CJK out of scope for this release the
  divergence is far less likely to be met, but the rule is unchanged.
* **Check it on screen.** Unchanged, and this sprint is the one where it
  matters most: every pixel is ours now.
