# Sprint WG5 — Workspace and Browser, the two flagship views

*docs/ROADMAP.md's WG5 row, unpacked. Written before the code, which is the
order WG3 kept and WG4 had to be corrected back into.*

The row:

> the two flagship views: Workspace with syntax colouring + ghost line +
> Ctrl-D/Ctrl-P; the four-pane Browser over the shared path-scheme model,
> Accept persisting to the image byte-identically to the web path (the CG8
> gate, re-run)

Size **L**, and it is two independent halves that share only the shell they
sit in. They are therefore two slices — **WG5a Workspace**, **WG5b Browser** —
and 5a lands first because 5b's Accept needs a place to report failures and
5a builds it.

## Prerequisites, and what changed to make them true

* **WG4 D1** — the primary VM and the `#uiReq`/`#uiReply` seam. This is the
  one that mattered: *Do It runs the user's code, and the user's code lives on
  the primary.* Before D1 this sprint could not have been built honestly at
  all, which is exactly why the author stopped the first attempt at it.
* **WG4 D4** — enablement tracked by the focused control. Already written to
  ask *"can the thing the user is typing in supply source?"*, and already
  answering `true` for an editable multiline `EDIT`. The Workspace lights it
  up with no change to that file, which is the test that D4 was designed
  rather than fitted.
* **WG4 D5** — the transcript dock, where a Do It's result and a failed
  Accept both report.
* `image_store` (55 tests, builds on Windows) for 5b's persistence.

## WG5a — the Workspace

### D1. Selection-or-everything, ported verbatim as ARITHMETIC

The Mac's rule (`CocoaUI class>>evalTargetFor:loc:len:`) is already pure and
already headless-tested: *a non-empty selection wins; an empty (collapsed)
selection falls back to the whole buffer*, answering `#(source insertAt)`.

It ports **unchanged** — same selector, same shape, same tests — because it is
arithmetic over a string and two integers and has nothing platform in it. What
differs is only where the two integers come from: `EM_GETSEL` rather than
`NSTextView>>selectedRange`.

Stating it as a separate design item because the temptation is to reimplement
it against Win32's API shape and end up with a rule that *nearly* matches the
Mac's. Two GUIs disagreeing about what Do It evaluates would be a bug nobody
could see in either one alone.

### D2. Do It and Print It, over the seam

* **Do It** — ship the target source to the primary as `#uiReq`/`#doit`, and
  append `source => result` to the transcript when the reply lands. This is
  `Worker uiDoit:onReplyTimed:`, which already exists and is already what the
  Mac calls.
* **Print It** — the same request, but the result is inserted **inline** at
  the insertion point, so the Workspace becomes a notebook rather than a
  command line.

**The one hazard, and it is the Mac's own recorded scar:** *capture the
insertion point at INVOCATION, not when the reply lands.* The reply is
asynchronous — the user can move the caret, type, or select something else
while the primary is working — and inserting at "the selection" on arrival
puts the answer wherever they happen to be. The Mac calls this its
`pendingPrintInsertAt` discipline and ported it verbatim from the web GUI;
this port inherits it as a rule rather than rediscovering it as a bug.

### D3. Ctrl-D / Ctrl-P

§2.5: Ctrl rather than Cmd, because Cmd is not a Windows key. `WM_KEYDOWN` is
already allowlisted; the shell needs only to notice the modifier
(`GetKeyState(VK_CONTROL)`) and route. No accelerator table, because there is
no menu yet to own one — WG6 adds the menu and this moves into it.

### D4. The ghost line

§2.1's Workspace row, fixing weakness #4: one ghosted line at the caret on
first open — `3 + 4 * 2   — Ctrl-P prints the result` — vanishing on the first
keystroke. It is a teaching affordance, not a placeholder: the Workspace's
empty state currently teaches nothing, and a user who has never met a
Smalltalk workspace has no way to guess what it wants.

### D5. Syntax colouring — DECLARED OUT OF THIS SLICE

The row asks for it and it is not free: a plain `EDIT` cannot colour text, so
it means either `RichEdit` (a different control, different message set,
different quirks) or a custom-drawn code pane (a full text editor: caret,
selection, scrolling, IME). Both are real work with real risk, and neither
belongs in the slice that first makes Do It work.

Recorded as **WG5c**, after the Browser, so that the decision between them is
made with the Browser's own text needs known rather than guessed. The
Workspace ships monospaced and uncoloured first, which is what every one of
these environments did in its first year.

## WG5b — the Browser

Four panes over the shared model, Accept persisting to the image. Detailed
now that 5a has landed, and the research changed the shape of it.

### What the Mac actually does, and why it matters here

Reads are **DUAL**, deliberately (`66_cocoabrowser.mst`'s own note):

* **hierarchy and selector rows** come from the PRIMARY's live reflection —
  `UiBrowserService browseSnapshot`, which already exists, and which projects
  the live hierarchy into a names-only nested tree of Strings, Arrays and
  smis. Names only, because *the pickle REFUSES class objects* and that
  refusal is the enforcement that no class oop crosses a VM boundary.
* **the variables pane and all source text** come from the IMAGE
  (`image_store`), so an added variable shows immediately while live instance
  shape honestly waits for the next boot — there is no `become:`.

Writes drive `image_store::flows` — the web GUI's own implementation, shared
rather than reimplemented — and live-compile on the primary through the
ordinary `Worker uiDoit:` channel.

### The consequence for this port, and the slicing it forces

The live half needs **nothing new**: WG4 D1 built the seam, `browseSnapshot`
is already pickle-safe by construction, and `Worker uiDoit:` already
live-compiles. A four-pane browser over live reflection is therefore reachable
today, entirely in Smalltalk, with no new Rust at all.

The image half needs a **host service** — the Mac's `host_service.rs` is an
ObjC-shaped adapter over `image_store::flows`, and Windows has no equivalent.
That is real work, it is where the CG8 gate lives, and it is the reason this
row is sized **L**.

So:

* **WG5b-1 — the browser, over live reflection.** Four panes (packages,
  classes, protocols, selectors) plus a source pane, fed by
  `browseSnapshot` across the seam. Read-only. Everything a user needs to
  BROWSE, which is most of what a browser is for, and it exercises the seam
  with a payload far larger than a doit.
* **WG5b-2 — Accept, over `image_store::flows`.** The host service, the write
  path, and the CG8 gate: *Accept persists byte-identically to the web path*.
  That gate is the specification and it is not negotiable — two GUIs that
  wrote the image differently would be a corruption bug with a UI in front of
  it.

Slicing it this way keeps a read-only browser from waiting on a write path,
and keeps the write path from being rushed to make a demo look complete.

### Pitfalls specific to 5b

* **`browseSnapshot` is a WHOLE-HIERARCHY payload.** It crosses the seam as
  one pickled tree. That is fine — it is names only — but it must be requested
  on a refresh rather than per pane, or four panes become four traversals of
  the entire image.
* **Names only, and that is load-bearing.** Anything the browser wants that
  is not in those six slots (source, comments, categories) comes from the
  image path, not by widening the snapshot until a class oop sneaks in.

## Implementation order

1. **5a D1** — the pure rule + its tests, with no window involved.
2. **5a D2/D3** — the Workspace view, editable, Ctrl-D/Ctrl-P over the seam.
3. **5a D4** — the ghost line.
4. **5b** — the Browser, on its own sprint detail.
5. **5c** — syntax colouring, once the control choice is informed.

## Pitfalls

* **The insertion point is captured at invocation.** Stated twice on purpose.
* **`EM_GETSEL` answers UTF-16 code-unit offsets**, and the guest's `String` is
  not UTF-16. Every offset that crosses the FFI is in the control's units and
  must be converted at that boundary, exactly once — a rule worth writing down
  now, because the failure mode (an emoji in a workspace shifting every
  subsequent offset by one) is invisible until it is baffling.
* **Do It's reply may outlive its view.** A user can switch views, or collapse
  the dock, while the primary works. Continuations must tolerate the world
  having moved, which the `(peer, corr)` keying already guarantees — but the
  *view* must be re-found rather than captured.
