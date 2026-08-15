# WG14 — the declarative UI: spec + bind as the first-class UI layer

Decision of record, 2026-08-13. The direction, in the author's words: *"we
need a first class replacement here for the cocoa ui, and I think the
declarative UI will be it"* — the declarative model of
[albanread/wingui](https://github.com/albanread/wingui) (win32 + directx
declarative gui), proven by winscheme. Both repos are cloned beside this one
(`C:\projects\wingui`, `C:\projects\winscheme-dev-2026`) and were read in
full before this was written.

## 1. Why — what the Cocoa layer actually is

The Mac GUI was surveyed before designing its successor
(`world/64_cocoaui.mst`, `76_spriteed.mst`, `77_sounded.mst`). The findings
that matter:

- The whole Smalltalk GUI layer holds exactly **three** NSWindows: the shell
  and two tool editors. Everything else is a tab in the view registry.
- The Tools menu's entire contract is a class-side unary `open`, resolved by
  name at click time. **There is no tool-window abstraction at all** — each
  editor hand-rolls a ~15-line NSWindow prologue, duplicates the control
  factories (`77_sounded.mst:65` says so in a comment), and is torn down by
  hard-coded name from `CocoaUI teardown`.
- The informal protocol is four duck-typed messages: `open`,
  `windowShouldClose:` (hide, don't die), `teardown`, and `previewTick`
  (tool windows cannot own timers — they ride the shell's 4 Hz beat).
- The costs are real and shipping: the tool windows miss the font-size and
  theme broadcasts (`applyFontEverywhere` never asks them), miss
  `environmentRestarted`, and every control is placed by hand-stepped
  absolute floats in a deliberately non-resizable window.
- Layout is magic numbers everywhere; the positional-argument constructor
  style produced real bugs in winscheme's own demos
  (`user-app-native-demo.ss` calls `user-app-tabs` with the wrong arity and
  cannot run as written).

So the assignment is not "port two NSWindows". It is: give the Windows shell
the thing the Mac never had — a declared UI a tool can be *written in*, with
lifecycle, layout, theme and events owned once.

## 2. The decision

**Adopt wingui's spec+bind CONTRACT. Implement the realizer natively in this
repo, in Smalltalk, over the layers this port already trusts.** Do not load
the wingui C++ runtime.

The precedent is winscheme itself: it does **not** link wingui — wingui is
the later *extraction* of winscheme's UI (`wingui/native_ui_port.md`). The
Scheme system builds spec trees as plain data, normalizes and **diffs them in
Scheme**, and ships patch ops to a native applier. The host language owns
state and reconciliation; the native side renders and emits events. That
split is the design; the C++ is one realization of it, and this repo already
has the other half built:

| wingui piece | WINARM already has |
|---|---|
| Win32 control realizer | `WinControl` factory + `WinApi` FFI (the guest creates real HWNDs today) |
| `text-grid` pane | `winui_render`'s cell grid — the same shader |
| `indexed-graphics` pane | the indexed plane + copper palette |
| `rgba-pane` | the direct BGRA plane |
| event transport | the door + ALLOWLIST + flag-and-drain |
| multi-window message routing | the door routes by hwnd already |
| dark titlebar / Mica / DPI | WG1 shipped it (wingui has **no** dark mode) |

Where a contract question arises, **the wingui headers are truth, not its
markdown** — the survey found the docs stale in places (`EventView` has four
fields, not three; the diff/patch engine exists; validation is deep). The
authoritative files are `include/wingui/ui_model.h`, `src/ui_model.cpp`,
`src/native_ui.cpp`, `include/wingui/spec_bind.h`.

## 3. The contract, as adopted

A UI is a **tree of nodes**. A node is a Dictionary with `type`, an explicit
`id`, props, and `children`. The root is `type: window` with `title`,
optional `menuBar`/`statusBar`, and a `body`. Handlers are never in the
tree — only event *names*; the host binds names to blocks.

The Smalltalk spelling is keyword messages on a builder (this is the direct
lesson of winscheme's arity bugs — keyword messages make them inexpressible):

```smalltalk
WinSpec window: 'Sprite Editor' id: #spriteEd body: (
    WinSpec stack: {
        WinSpec card: 'Frame' id: #frameCard children: {
            WinSpec indexedPane: #grid width: 432 height: 432.
            WinSpec row: {
                WinSpec button: 'Pencil' event: #toolPencil id: #btnPencil.
                WinSpec button: 'Fill'   event: #toolFill   id: #btnFill } }.
        WinSpec input: 'Name' value: doc name event: #nameChanged id: #nameField })
```

Adopted from the contract, staged (§6):

- **Containers**: `stack` (gap 12), `row` (natural widths, proportional
  shrink, 72px floor), `grid` (columns, row-major), `card` (fixed padding 12
  / gap 10, heading font title), `divider`; later `scroll-view`,
  `split-view` (exactly two `split-pane` children — validated).
- **Controls**: `text`, `heading`, `button`, `input`, `textarea`,
  `checkbox`, `slider` (trackbar; needs `ICC_BAR_CLASSES` added to
  `ensureCommonControls`), `select`, `list-box`, `progress`; later `table`,
  `tree-view`, `tabs`.
- **Panes**: `text-grid`, `indexed-graphics`, `rgba-pane` — realized as
  children of the shell's own window class so their WM_PAINT reaches the
  door, drawn by `winui_render` exactly as the Canvas view is today.
- **The label rule**: a labeled control renders a STATIC above itself,
  costing labelHeight + 8.
- **Events**: the envelope `{event, id, value/checked/..., source}` arrives
  through the door as WM_COMMAND/WM_NOTIFY from realized controls, is
  flagged, and is dispatched by the drain to the window's handler block —
  `event` as a Symbol. Payloads stay small by rule (wingui's transport
  truncates at 512 bytes; ours doesn't, but the discipline keeps specs and
  events cheap to log and test).
- **Reserved events**: `#closeRequested` (default: hide, never die — the
  Mac's own choice, kept deliberately), `#hostStopping`.

### Identity and reconciliation — THE rule

Republish is the programming model: a handler updates state and republishes
the whole spec; the DIFFER makes that affordable. The diff is **id-based**:

- Node identity is `(type, id)`. Either changing = rebuild that subtree.
- Prop changes on the same identity = in-place ops (`setText`, `BM_SETCHECK`,
  `TBM_SETPOS`...), applied with a **suppress-events guard** so programmatic
  updates never re-enter as user events.
- Child-list changes reconcile only when every child has a stable id
  (`append/insert/remove/move-child`); otherwise the container rebuilds.
- **Every node gets an explicit id in house style.** Auto-ids exist
  (`__auto__:path`, matching wingui's grammar, stamped by `normalize`) but
  they are positional — anything whose sibling order can change must be
  named. This is the single highest-leverage rule in the whole design.
- The differ answers `nil` for "I cannot express this as a patch" and the
  realizer falls back to a full rebuild of that window — winscheme's
  cleanest idea, kept verbatim.

### The dispatch discipline

Ported from `user-app-ui.ss:1196-1214` because it is correct: event depth is
counted; `rerender` during a handler only sets a pending flag; the drain
flushes ONE coalesced reconcile when depth returns to zero. N state
mutations in a handler = one diff = one screen update. The host-echo rule
rides along: state the realizer already applied from user interaction
(a slider position, a selection) is mirrored into the retained spec on event
receipt, so the next diff is against what is actually on screen.

## 4. The pieces

- **`WinSpec`** (world file, pure) — keyword constructors for every adopted
  node type; `normalize` (auto-ids by path); `validate` (the publish-time
  checks, §6.4 of the survey, so shape errors surface at build time);
  `diff:against:` → ops or nil; `asJson` for interop and for cross-checking
  a spec against `wingui_spec_builder_validate_json` in a gate someday.
  Headless-testable end to end — build, normalize, diff, assert ops — with
  no window, exactly like the SoundDoc/view-row precedent.

- **`WinRealize`** (world file) — spec → live `WinControl` tree: measure and
  layout per the surveyed algorithm (two DFS passes; stack/row/grid/card
  arithmetic is specified in prose in the survey and is PURE — layout gets
  headless tests too); create/update/destroy controls by patch op; the
  suppress-events guard; scroll preservation later.

- **`WinToolWindow`** (world file) — the piece the Mac never had, formalized
  from its duck-type: a registry of `id → window` where each window owns a
  spec-render block, a handler block, its retained last spec, and its HWND
  (a second top-level of the shell's class — the door routes to it by hwnd,
  as it already does for panes). Lifecycle: `open` fronts-or-builds;
  `closeRequested` hides; `teardown` in delegate-detach order; the shell's
  beat ticks every open tool window; **theme, font and environmentRestarted
  broadcasts reach every registered window** — the five things the Mac
  hand-wires or forgets, owned once.

- **The Tools menu** — `menuSpec` grows a Tools popup between Demos and
  View: Sprite Editor…, Sound Editor…. Items open tool windows through the
  registry, not through by-name `open`.

## 5. Deliberate departures from wingui

Recorded so they are choices, not drift:

1. **No JSON on the hot path.** Specs are Smalltalk data end to end; JSON is
   an emitter for tests/interop. The contract is the SHAPE, not the text.
2. **Keyword messages, not positional constructors** — winscheme's own
   demos carry the scars.
3. **Events are Symbols** in Smalltalk, strings at the JSON boundary.
4. **Dark mode and DPI are ours** — wingui has none; the shell already does.
5. **The door, not a second message loop.** Realized controls are ordinary
   children of shell-class windows; the ALLOWLIST and drain discipline apply
   unchanged. No `wingui_pump_message`.
6. **Fonts**: the shell's existing metrics, not `lfMessageFont` — one font
   story per app.

## 6. Staging

| Stage | Contents | Gate |
|---|---|---|
| **WG14a** | `WinSpec`: constructors, normalize, validate, diff. Pure. | headless tests: build/normalize/diff/ops, id rules, validator rejections — in the world suite |
| **WG14b** | `WinRealize` MVP (window, stack, row, card, text, heading, button, input, checkbox, divider; label rule; layout per contract) + `WinToolWindow` + the Tools menu. One demo tool window opens, handles a click, republishes, patches in place. | scripted: open from Tools menu, drive a button through the door, assert the patch was in-place (a rebuild counter, mirroring wingui's patch metrics) |
| **WG14c** | Panes over `winui_render` (text-grid/indexed/rgba), `slider` (+`ICC_BAR_CLASSES`), `select`/`list-box`. **Sprite Editor ships on it** — the first real tool, closing the WG13 gap. | sprite editor: paint, frames, save-as-sheet round trip, preview into the indexed pane |
| WG14d+ | `table`, `tree-view`, `tabs`, `split-view`, `scroll-view` as real tools need them; migrate existing views opportunistically, Sound Editor first. | per-tool |

The long arc, stated once: the same spec model realized by a Cocoa realizer
would replace the hand-rolled NSWindows on the Mac too — the games'
portability contract applied to the UI. Nothing in WG14 depends on that
happening, and nothing in it forecloses it.

## 7. Traps carried over, so they are paid once

- Patch metrics from day one (`publishCount`, `patchCount`,
  `subtreeRebuilds`, `windowRebuilds`) — they are how an id-strategy failure
  is SEEN rather than felt as flicker.
- The suppress-events guard, or programmatic updates loop.
- Mirror user-applied state into the retained spec (host echo), or every
  interaction is followed by a needless patch.
- `ICC_BAR_CLASSES` before the first trackbar, or `CreateWindowExW` answers
  NULL and, in this codebase's own words, nothing else says why.
- Only whitelisted array props patch in place (`options`, `tabs`, table
  `rows`/`columns`, tree `items`/`expandedIds`); any other non-scalar change
  is a rebuild — model props as scalars wherever possible.
- Bind `#closeRequested` in `WinToolWindow` itself so no tool can forget it.
