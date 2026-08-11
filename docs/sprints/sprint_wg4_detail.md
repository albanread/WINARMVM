# Sprint WG4 — the shell: the primary first, then the bar it reports on

*docs/ROADMAP.md's WG4 row, unpacked. Written after the first half of the
sprint had already been built and gated, and the ordering below is a
correction to the order that half was built in — recorded rather than
quietly fixed, because the mis-ordering is the instructive part.*

The row, in full:

> the one-window grammar stands: custom-drawn view bar (Fluent glyphs +
> labels + accent underline), view switching with lazy build, docked
> collapsible Transcript, live metrics cluster **reading `VmMetrics` off the
> primary**; Do It/Print It enablement **tracked by focus**

Four clauses. Two are pure UI and were built first; two name things that do
not exist yet in this process, and one of those is architectural.

## Prerequisites

* WG3, gated (`just gate-wg3`, run with `MACVM_WINUI_WG4=off`).
* §2.2's four commitments, of which **commitment 2 is this sprint's real
  content**: *the primary VM never touches a handle; it messages the UI VM,
  copy-passed and asynchronous, over the `#uiReq`/`#uiReply` protocol the
  workers already carry.*
* `win_gui/src/main.rs` states plainly that WG1 has **no primary VM**, and
  defers it: *"back at WG4+, when commitment 2 (a primary messaging the UI
  VM) has [landed]"*. This is that sprint.

## What is already built, and what it is worth

Landed and gated (`just gate-wg4`, `scripts/winui-wg4.tcl`,
`world/93_winui_shell.mst`, `world/tests/64_winui_shell_tests.mst`):

* **View switching with lazy build.** Three views registered, one built at
  open; a re-switch builds nothing; a third view builds on its own first
  visit. Proven live through a real `WM_COMMAND`, and headlessly by counters.
* **The docked collapsible Transcript**, collapsing as a *height change*
  (the band never leaves the layout) so reopening restores a number rather
  than reconstructing a structure.
* **`WinLayout` extended** with bottom-anchored bands and
  `packRow:left:right:gap:dpi:` — still pure arithmetic, still testable with
  no window, and WG3's own layout pinned unchanged by a test.
* A **metrics cluster and a view bar**, as *plain `BUTTON` and `STATIC`
  controls*.

That last line is the honest one. The grammar stands; the **materials are
placeholders**, and the metrics have nothing real to report.

### The mis-ordering, and why it is recorded here

The shell was built before the primary existed, so its metrics cluster grew
a setter — `updateMetricsMem:jit:code:alloc:gc:` — that a *script* calls
with literal strings. The gate then proved the cluster displays what it is
told, which is true and nearly worthless: the row asks for values **read off
the primary**, and a display fed by hand cannot fail the way a real one can
(a stale sample, a dead primary, a push arriving mid-layout).

WG3's own sprint order got this right for the drain — *"the drain landed,
with its gate, before the first control existed"* — and the reason it gave
applies exactly here: **retrofitting a data source under a display that has
been shaped by a placeholder is how the Cocoa side acquired its scars.**
The selector survives (it is the Mac's own, and the right shape), but its
caller must become the primary, and the sprint's remaining order puts the
primary first.

## Design

### D1. The primary VM, and the messaging seam — first, and alone

Commitment 2, and nothing above it until it holds. Mirrors CG1 on the Mac,
whose machinery (`register_hosted_worker`) the design says *already passes
its tests on Windows*.

* The **primary** is the user's world: long-lived, on a background thread,
  owning no handle. It is the VM a user would call "their image".
* The **UI VM** stays what WG1 made it: pinned to the main thread, holding
  handles, pumping messages — a terminal, not an application.
* Between them, the existing `#uiReq`/`#uiReply` envelopes, copy-passed.
  **Two heaps, strictly separate**; no oop crosses.
* The UI VM's drain is the only place replies are serviced — §2.4a's rule is
  unchanged by the second VM, and the reply pump is another drain callee.

**Why the split exists, in the author's own words (2026-08-11):** *"the
split is core to the design — we have a UI VM that does not block, because
the VMs doing work are not the UI thread."* That is the acceptance
criterion, and it is a stronger one than a round trip: a message loop that
stops pumping is a hung window, whatever the architecture diagram says.

So D1's gate is **two claims, not one**:

1. **The round trip.** A request issued on the primary, serviced on the UI
   VM, replied to, and the reply's continuation run on the primary — with
   both VMs' identities distinct and provable, so "it worked" cannot be
   satisfied by one VM talking to itself.
2. **The UI does not block.** With the primary deliberately busy for
   *seconds*, the window keeps pumping: messages keep crossing the door,
   the drain keeps passing, a resize still lays out, and the snap still
   answers. Measured as a RATIO — messages serviced during the busy window
   against messages sent — because "it felt responsive" is not a test and a
   frozen pump answers zero.

### D2. Metrics, read off the primary

Only once D1 holds.

**Correction to this section's first draft, made when the Mac's own path was
read properly.** The draft said the sample would travel as an ordinary
`#uiReq`. It should not, and `cocoa_gui` does not do that: a metric is a
*sample*, not a *request*, and putting a 4 Hz push on the request seam would
load the very channel whose latency the Do It path depends on — for data
nobody is waiting on. The Mac publishes the primary's `VmMetrics` into a
shared `Mutex` (the VM monitor registry's own shape) and the UI side reads
it, formats it, and execs the update. Same claim — *reading `VmMetrics` off
the primary* — without borrowing the seam to do it.

So: the primary's beat loop publishes `primary.metrics()` on every beat
(250 ms — exactly the ~4 Hz the Mac's cluster rides), and the UI thread
samples that shared snapshot in the pump, formats it, and calls the
cluster's existing selector. The five values keep travelling **as one
sample** — a cluster showing a mixture of two moments is the small lie a
metrics readout must not tell, and one `Mutex<VmMetrics>` copy makes that
structural rather than a matter of care.

What this buys that the placeholder cannot: a **stale** sample is
detectable (the UI knows when it last heard), a **dead primary** is visible
(CG9's restart-in-place behaviour has something to restart), and the push
races the layout for real.

### D3. The custom-drawn view bar

§2.1's row, and the first place the window stops looking like a dialog:

* One **owner-drawn strip**, not comctl's dated `TOOLBARCLASS` chrome.
* View glyphs from **Segoe Fluent Icons** (the system icon font — the native
  answer to SF Symbols), **with 9pt labels beneath**, which is §2.5's
  deliberate divergence from the Mac (weakness #1, and the Mac may adopt it
  back).
* The active view marked by an **accent-colour underline** — the Windows tab
  idiom — with the accent **read from the system**, not chosen here.
* Do It / Print It as **labelled pill buttons on the same bar**.

**The message-architecture question this raises, stated before it is
coded.** An owner-drawn control sends `WM_DRAWITEM` to its *parent*, with a
`DRAWITEMSTRUCT` pointer valid **only for the duration of the call** — the
same lifetime constraint D3 recorded for `WM_NOTIFY`'s `NMHDR`, and the same
rule applies: *read the few fields in the door; stash no pointer.*

But a paint cannot be deferred to the drain the way a click can. §2.4a's
split is "the door records and returns; the drain does the work", and a
`WM_DRAWITEM` that returned without drawing would leave the strip blank
until something else repainted it. The resolution is that **painting is not
work in the drain's sense** — it is a *response*, bounded, allocation-free
and re-entrancy-free by construction (GDI calls into a DC Windows just
handed us; no `CreateWindowExW`, no `SetWindowPos`, nothing that sends a
message back). It is therefore the one class of handler permitted to act
inline, and the permission is **narrow and stated**: draw from fields
already copied out of the struct, call no VM entry, allocate nothing, and
touch no control other than the one being drawn.

If that proves too strong a claim under measurement — if a paint handler
turns out to provoke a message — the fallback is the Cocoa answer: draw into
a **cached bitmap** in the drain and have the paint handler blit it. Written
down now so the fallback is a decision rather than a scramble.

### D4. Do It / Print It enablement, tracked by focus

The row says **focus**, and the shell currently tracks the **active view** —
a coarser thing that happens to agree today because only one view could ever
supply source. It will stop agreeing the moment WG5's Workspace shares a
window with the Browser's four panes.

So: `WM_SETFOCUS`/`WM_KILLFOCUS` reach the door, are recorded there, and the
drain re-evaluates enablement. The predicate becomes *"the focused control
can supply source"*, which is the Mac's own contract (§2.1: *greyed by focus
context — the Mac contract, stated and tested*).

### D5. The transcript's splitter, and the fonts (added 2026-08-11)

§2.1 specifies two things the ladder never attached to a sprint row, and
both are why the window still reads as a *dialog* rather than an *app*:

* **The dock is on a real splitter** with a **visible grab bar + chevron**
  (§2.1's "fixing weakness #3" — the Mac's own dock has no visible grab and
  users do not discover it), plus a **Clear** button at its right. Today the
  dock's height is a constant only a script can change; a user cannot drag it.
* **The fonts are wrong.** §2.1 asks for **Segoe UI Variable** for text at
  Fluent's 9pt/12px metrics, and **Cascadia Mono** for code panes and the
  transcript. Everything currently draws in the stock dialog font, which is
  the single largest reason the window looks like a dialog box.

Recorded as a row of this sprint rather than left to WG8's "polish sweep",
because a shell whose own chrome is placeholder is not "the one-window
grammar standing" in any sense a reader of the row would expect.

**Deliberately still NOT here: a status bar.** Neither this document nor the
Mac has one — the docked Transcript is that surface, and the metrics cluster
carries the VM state a status bar would otherwise hold. If one is wanted it
is a genuine ADDITION to the design, not an omission from it, and it should
be argued for on its own terms rather than smuggled in as polish.

## Implementation order

1. **D1, alone.** The primary VM and the `#uiReq`/`#uiReply` round trip,
   with its own gate, and **no UI change riding along**. The window must
   behave exactly as it does today while this lands — the same discipline
   WG3 applied to the drain.
2. **D2.** Re-point the metrics cluster at the primary; delete nothing, but
   the gate stops feeding it literals and starts asserting a sample arrived.
3. **D3.** The owner-drawn bar, glyphs, labels, accent underline. Pixel-
   proven, as WG3 proved theming: a themed strip is not the same colours as
   an unthemed one, and the snap can settle it.
4. **D4.** Focus tracking, and the enablement predicate that reads it.
5. **D5.** The splitter (drag, grab bar, chevron, Clear) and the fonts.

## Pitfalls

* **Do not let the UI VM become its own primary.** WG1's Δ 1 is explicit
  that `register_hosted_worker` mints an entry in a *primary's* registry;
  calling it on the only VM in the process answers `None` or, worse, makes
  that VM its own host for no reason. D1's gate must prove **two distinct
  VMs**, or it proves nothing.
* **No oop crosses the seam.** The envelopes are copy-passed and pickled.
  A reply that handed back an object would be a second heap's pointer in the
  first heap's hands, and the failure would surface as a GC crash far from
  the cause.
* **A reply is serviced in the drain**, like every other unit of deferred
  work. A reply continuation that ran in the door would be a nested VM entry
  — §2.4a's whole subject.
* **`WM_DRAWITEM`'s struct pointer dies with the call.** Copy the fields.
* **The metrics push races the layout.** Both are drain work; that is the
  point. What must not happen is a push *from* a paint handler.

## Out of scope

* WG5's Workspace and Browser — including `Do It`'s *action*. This sprint
  makes the buttons' **enablement** honest; what they do when clicked is the
  next rung, and it needs the primary this sprint builds.
* Syntax colouring, the ghost line, splitter grab bars (WG5/§2.1).
* The Demos menu, Canvas, games and sound (WG8 and the recorded stretch).
