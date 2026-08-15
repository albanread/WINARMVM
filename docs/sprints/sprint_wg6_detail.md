# Sprint WG6 — Outliner, Find, Editor, and the menu

*docs/win_gui_design.md's WG6 row, unpacked. Written before the code, which
is the order this port keeps.*

The row:

> tree over live reflection with counts; Find sweeps landing selections in
> the Browser; Editor with File In / Add to World

Size **M**, three views — and a fourth deliverable the row does not name but
WG5a already promised: **the menu**. WG5a D3 routed Ctrl-D/Ctrl-P by hand and
said so explicitly — *"no accelerator table, because there is no menu yet to
own one; WG6 adds the menu and this moves into it."* That debt is due here.

## What the research changed, and it changed the plan twice

### 1. The Outliner needs NO new Rust, and is better than the Mac's

`UiBrowserService class>>browseSnapshot` (world/34_tools.mst) already answers a
6-slot node:

```
{name. instVarNames. classVarNames. instSelectors. classSelectors. children}
```

That is *exactly* the Strongtalk outline — class → instance variables, class
variables, instance methods, class methods, subclasses, nested — and WG5b-1
already fetches it, from the **primary**, and caches it in `BrowserTree`.

So the Outliner is a second rendering of a tree the shell already holds. No
host service, no new seam traffic, no Rust at all.

**And it is strictly better than the Mac's.** `69_cocoaoutliner.mst` reads the
UI worker's OWN VM through `ClassMirror`, and carries a scope note admitting
the consequence: *a class defined via a Workspace Do It on the primary will
not appear until a UI rebuild.* Windows has the `#uiReq` seam WG4 D1 built, so
this port reflects the primary and that caveat simply does not exist. Worth
stating plainly because it is the first place where the Windows port is ahead
of the reference rather than catching up.

### 2. The Editor's whole-class save is ALREADY duplicated, and this sprint must not make it three

`cocoa_gui/src/host_service.rs::persist_editor_class` says of itself: *"the
web GUI's `persist_editor_class` logic, over the shared `image_store` API"*.
So the diff-and-write already exists twice — once in the web GUI, once copied
into the Mac's host service.

A third copy in `winui_host` would be exactly the failure the CG8 gate exists
to prevent, one level up: not two GUIs writing the image differently, but
three implementations of *how* to write it, drifting independently.

**So WG6c promotes it into `image_store::flows`** — beside `save_method`,
`new_class_from_source` and `add_variable`, which is where the shared write
path lives — and `winui_host` calls that.

**What this sprint will NOT do is edit `cocoa_gui`.** It is AppKit and cannot
be compiled, let alone tested, on this machine; changing it here would be
changing code I cannot check. The honest outcome is therefore: one shared
implementation in `flows`, used by Windows, with the Mac's copy left intact
and this note recording that it should be migrated by someone on a Mac.
Flagging a duplication I am not in a position to remove is better than
silently adding to it.

## The slices

**WG6a — the Outliner.** One `SysTreeView32` over the cached `BrowserTree`,
with counts on the group rows (`instance methods (42)`), and selection
landing in the Browser the way a Find hit will. Lands first precisely because
it needs nothing new: it is the slice that proves the tree data was already
right.

**WG6b — Find.** Implementors and Senders over `Image::implementors_of` /
`senders_of` — the persisted `method_sends` index, which is what makes senders
*accurate* rather than textual. Two new `winui_host` exports, and the payoff
the Mac calls out: **selecting a hit jumps to the Browser** — switch view,
select the class, flip the side, select the selector — using the Browser's
own selection helpers, which WG5b-1 already exposes.

**WG6c — the Editor.** Whole-class source: `class_source` to load, the
promoted `persist_editor_class` to save, plus File In and Add to World from
the row. Save does both halves the Browser's Accept does — live-compile on the
primary, write the image — in that order and for the same reason.

**WG6d — the menu.** A real `HMENU` with File / Edit / Source, an accelerator
table owning Ctrl-D/Ctrl-P (retiring WG5a D3's hand-rolled `GetKeyState`
routing), and the two dialogs WG5b-2's host service has been waiting for:
**New Class** and **Add Variable** already have exports and Smalltalk wrappers
and no UI at all.

## As built — WG6a, WG6b

**WG6a landed with no Rust at all**, as predicted, and one thing the design
did not predict: inserting the outline eagerly is 3507 rows and ~600ms of
synchronous UI work on first open — it timed out three control-port calls and
would have been a visible freeze on a view switch. The hierarchy alone is 175
rows and 11ms; a class's own rows are added when it is SELECTED. Selection
rather than expansion is FORCED, not preferred: `TVN_ITEMEXPANDING` names its
item only through `NMTREEVIEW`, and the door reads NMHDR's three fields and
lets the pointer go, so an expansion cannot be attributed to a node while a
selection can simply be asked for afterwards.

Two further traps, both invisible to state:

* `TVM_GETITEMW` fills a fixed buffer and does not say how much it used, so
  reading 255 units answers a 255-character String padded with NULs. It
  PRINTS as `Object` and compares equal to nothing — the lookup missed every
  time and the tree never populated. The NULs leaking into the control port's
  output (grep calling it a binary file) is what gave it away.
* Lazily-added group rows land BEHIND every subclass, putting `Object`'s
  methods below a hundred descendants. The four groups now thread their
  handles so they lead.

**WG6b needed two exports and one wire format.** `MacvmHostImplementorsOf` /
`MacvmHostSendersOf`, both opening the image READ-ONLY, answering one line
per hit with 0x1F between fields — the Mac's own format, so the two GUIs'
parsers agree without either being able to see the other. Both the Rust and
the Smalltalk halves of that contract are tested where no window is involved.

The jump works and is gated as hard as the search: choosing a hit switches to
the Browser, selects the class, flips the side and selects the selector, and
the source pane shows the method.

One bug worth recording because it was silent in the worst way: the results
list stayed EMPTY while the transcript reported five hits. `listSet:items:`
takes a control NAME and was handed a control, so `controlNamed:` answered
nil and the method returned without a word. The gate now asserts that the
listbox row count EQUALS the hit count, which is the assertion that would
have caught it.

## Pitfalls, recorded before they bite

* **`TVN_SELCHANGEDW` arrives as `4294966845`.** It is `-451` as an unsigned
  32-bit value, and every tree-view notification is negative this way. A
  comparison against a signed constant silently never matches — the tree
  simply never reports a selection, with nothing to see. winkb answers the
  unsigned form, so compare in that form or convert once, deliberately.
* **`sizeOf: 'TVINSERTSTRUCTW'` answers 24, and allocating 24 bytes would
  corrupt the heap.** The struct's third member is an anonymous UNION of
  `TVITEMEXW`/`TVITEMW` at offset 16, and Win32Metadata records the union as a
  nested type reference rather than inline — so the recorded size covers the
  two handles and the union's *start*, not its contents. The real struct is
  `16 + sizeOf('TVITEMW')` = 72, and `TVM_INSERTITEMW` writes all of it.

  This is the one place where "derive, never transcribe" is not enough on its
  own: the database's answer is MISLEADING rather than missing, so it has to
  be composed (`offsetOf(Anonymous) + sizeOf(TVITEMW)`) instead of taken. The
  item's own fields are then at the union offset plus `TVITEMW`'s offsets,
  which stays fully derived.
* **`WM_NOTIFY`, not `WM_COMMAND`.** A tree view reports through `WM_NOTIFY`,
  which the door already carries and `serviceNotifyFrom:id:code:` already
  drains — but the shell's existing control plumbing is `WM_COMMAND`-shaped,
  and reaching for the familiar one is how a tree ends up mute.
* **The tree must be built once, not per refresh.** `TVM_INSERTITEMW` per node
  over 175 classes is fine; doing it on every drain pass is not. Rebuild on
  refresh, never on reconcile — the same rule WG4 D6 established for views.
* **Counts are on GROUP rows, not class rows.** `instance methods (42)` is
  useful; `Point (17)` invites the question "seventeen what?".
* **Check it on screen.** WG5c earned this its own line and it stays: the
  ghost line, the blank bar and the dead verbs were all states that read
  correct and drew wrong.

## Order

1. **WG6a** — the Outliner, over data the shell already has.
2. **WG6b** — Find, with the jump.
3. **WG6c** — the Editor, and the promotion of `persist_editor_class`.
4. **WG6d** — the menu, accelerators, and the two waiting dialogs.
