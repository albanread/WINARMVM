# Sprint WG5c — syntax colouring, and the control it needs

*docs/ROADMAP.md's WG5 row, the clause `sprint_wg5_detail.md` D5 deliberately
cut out: **Workspace with syntax colouring**. Written before the code, which
is the order this port keeps.*

D5 deferred it with a specific promise: *"the decision between them is made
with the Browser's own text needs known rather than guessed."* The Browser is
built and WG5b-2's Accept writes through it, so those needs are now facts
rather than predictions. This file is the decision and the evidence for it.

## The choice

A plain `EDIT` cannot colour text. Either:

* **`RichEdit`** (`RICHEDIT50W`, from `Msftedit.dll`) — a different control
  with a mostly-compatible message set, colouring via `EM_SETCHARFORMAT` over
  a selection.
* **A custom-drawn code pane** — we own every pixel, and therefore every
  behaviour.

## The evidence, measured rather than assumed

**1. The dependency surface is four messages, and it is tiny.** Everything the
two text surfaces ask of Win32:

| message | used by | RichEdit |
|---|---|---|
| `EM_GETSEL` | Workspace selection, WG5a D1 | supported |
| `EM_SETSEL` | Print It's insertion point | supported |
| `EM_REPLACESEL` | Print It's inline insert | supported |
| `EM_SETCUEBANNER` | the ghost line, WG5a D4 | **not supported** |

(`EM_RESERVE`/`EM_COMMIT`/`EM_RELEASE` in the same grep are `WinArena`'s own
names, not Win32.)

**2. The one incompatibility is already broken.** `EM_SETCUEBANNER` is
documented for single-line edit controls, and on our multiline Workspace it
**returns 0 — the control refuses it**. Checked directly on a running window:
the pane empty, unfocused, `installGhost` sent, and no ghost text drawn.

So WG5a D4 never worked. That is a defect in a slice I reported as complete,
and it is recorded here rather than quietly fixed, because it is also the
decisive fact for this sprint: **the only thing RichEdit would cost us is a
feature that does not currently exist.**

**3. The custom pane's true scope is a text editor.** Not "draw coloured
text" — caret placement and blink, click and drag selection, shift-arrow
extension, scrolling and scrollbar sync, word wrap, clipboard, undo/redo,
IME composition for non-Latin input, accessibility. Every one of those is
free in a stock control and every one is load-bearing: a code pane that
cannot be selected with the mouse is not usable, however well it colours.

## The decision: RichEdit

Three of four messages port unchanged, the fourth is dead already, and
colouring becomes `EM_SETCHARFORMAT` over ranges — no drawing code at all.
The custom pane buys total control over a surface where we have shown no need
for it, at the cost of writing and then owning a text editor.

**This is reversible in the direction that matters.** If a later sprint wants
a custom pane (for a ligature-aware renderer, say, or for inline widgets),
everything WG5c writes about *what to colour* survives — the tokenizer and
the colour table are pure and control-agnostic by construction. Only the
*applying* changes. That asymmetry is the reason to take the cheap option
first.

## What WG5c delivers

* **D1 — the tokenizer, PURE.** Smalltalk source → a list of
  `#(start length kind)` runs. No control, no Win32, no window: it is a
  function from a String to an Array, and so it is tested exactly, with
  equalities, exactly as WG5a D1's selection rule is. Kinds are the ones
  Smalltalk actually has: comment, string, symbol, character, number,
  keyword-selector, binary-selector, special (`^` `|` `:=`), pseudo-variable
  (`self`/`super`/`nil`/`true`/`false`/`thisContext`), class-name, plain.
* **D2 — the colour table**, keyed by kind, and light/dark aware from the
  same accent/system-colour path WG4 D3 already uses. Pure.
* **D3 — the RichEdit surface.** `Msftedit.dll` loaded before the class is
  named, the pane created as `RICHEDIT50W`, and `isSourceSurface` taught the
  new class name so WG4 D4's enablement keeps working with no other change.
* **D4 — applying the colour**, on an edit-idle rather than per keystroke,
  because recolouring a whole method on every character is how an editor gets
  a reputation for lag.
* **D5 — the ghost line, for real this time.** Whatever WG5c does here must
  be checked ON SCREEN, because that is precisely what was not done before.

## Pitfalls, recorded now

* **RichEdit line endings are CR, not CRLF.** WG5b-2 added a `crlf:`
  conversion at the control boundary for exactly this class of reason; it will
  need to become the RichEdit convention for the panes that move. The rule
  stays the same — convert at the boundary, once — only the target changes.
* **`Msftedit.dll` must be loaded before `CreateWindowExW` names the class**,
  or creation fails with a window that simply is not there. It is the same
  shape of failure as a missing `winui_host.dll`: silent, and easy to
  misdiagnose as a layout bug.
* **`isSourceSurface` hardcodes `klass = 'EDIT'`.** A pane that becomes a
  RichEdit stops being a source surface until that predicate learns the new
  name — and the symptom is the verbs greying out, which is now a bug with
  history. See WG4 D4's own scar.
* **RichEdit does not inherit the dialog font.** It must be told, or the code
  pane will not match the rest of the shell.
* **Check it on screen.** Stated as a pitfall because this sprint exists
  partly because a previous one did not: `installGhost` returned without
  error, its test asserted the TEXT it would use rather than that anything
  appeared, and the feature shipped broken. A colouring sprint whose gate
  only asserts that runs were computed would repeat that exactly.

## Order

1. **D1 + D2** — tokenizer and colours, pure, fully tested, no window.
2. **D3** — the control swap, with the enablement predicate updated in the
   same change.
3. **D4** — apply on idle.
4. **D5** — the ghost line, verified on screen.
