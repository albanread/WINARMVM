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

## As built — D1, D2, D3, D4

**D1/D2 landed pure**, as designed, and their gate is the tiling property
(`world/tests/66_winui_syntax_tests.mst`): the runs must reassemble the
source exactly, asserted on every case rather than in one test of its own.
Writing them turned up two defects in the TEST FRAMEWORK, not the tokenizer:
`assert:description:` was resolving to `Object`'s general-purpose assertion,
which SIGNALS — so inside a `TestCase` one wrong expectation aborted the
whole suite instead of reporting a failure, and 302 assertions across
WG5a/5b/5c were never counted. SUnit now has its own.

**D3 swapped both code surfaces**, and the swap is invisible — which is what
success looks like here. Verified on screen and in the gate:

| | |
|---|---|
| `Msftedit.dll` loads | yes |
| pane class | `RICHEDIT50W`, not the EDIT fallback |
| Workspace `isSourceSurface` | true |
| Browser source `isSourceSurface` | true |
| text round-trip, `EM_GETSEL`, eval-target rule | unchanged |
| Do It over the seam | fires, `3 + 4 * 2 => 14` |

Two things worth recording because they were predictions that turned out
either right or unnecessary:

* **`isSourceSurface` was the real risk and it was handled in the same
  file as the swap** — deliberately, so nobody can change which control the
  code panes use without meeting the predicate that decides whether the verbs
  light up for them. The port has already shipped that failure twice.
* **The CRLF worry did not materialise.** WG5b-2 converts LF to CRLF at the
  control boundary and RichEdit renders it correctly — the Browser's source
  pane shows a real multi-line method where the plain EDIT had shown boxes.
  No change was needed, and the pitfall below is left standing as a caution
  rather than a fixed defect.

Only the *other* views' content children stay plain `EDIT`s: they are
read-only placeholders with no code in them, and a text engine they never
colour would be cost for nothing.

**D4 applies it, and it is on screen.** A method with a comment, a class
name, keywords, a string, a symbol, a character, a radix number and `self`
colours the way a Smalltalk reader expects. Verified visually — not merely
counted — because a gate that asserted only "36 runs were computed" would
repeat WG5a D4's mistake exactly.

The three hazards the design predicted all turned out real, and all are
handled:

* **The selection had to be saved and restored.** `EM_SETCHARFORMAT` applies
  to the selection, so colouring N runs is N selection changes; a pass that
  did not put the caret back would move the user's cursor as a side effect of
  colouring. Restored in an `ensure:`, so it happens even if the pass raises.
* **It had to stop flickering.** `WM_SETREDRAW` off for the whole pass, one
  `InvalidateRect` at the end.
* **It could have looped**, since `EM_SETCHARFORMAT` can itself raise
  `EN_CHANGE`. A flag held for exactly the length of the pass.

And one the design did not predict: **a RichEdit sends no notifications at
all by default**, unlike a plain EDIT. Without `EM_SETEVENTMASK`/`ENM_CHANGE`
the EN_CHANGE handler simply never fires, which reads as a broken handler and
is not one.

The struct is DERIVED, not transcribed, and that paid: Win32Metadata records
`CHARFORMAT2W` as inheriting `CHARFORMATW` through a `Base` field at offset
0, so `cbSize`/`dwMask`/`crTextColor` are looked up on the base type. Counting
bytes by hand would have been a silent off-by-92.

**Known limit, recorded rather than half-fixed.** The tokenizer counts in the
guest String's units and `EM_EXSETSEL` counts in the control's. They agree
below U+0080 and diverge above it — the same boundary rule WG5a records for
`EM_GETSEL`. The honest fix is one conversion at that boundary when the panes
first hold non-ASCII; a partial one now would be harder to find later than
none.

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
