# Sprint WG7 — Debugger + Monitor, and the primary restarted in place

*`docs/SPRINTS.md`'s WG7 row: "halt loop fronted natively (+F5/F10/F11);
Monitor with column priority; primary restart-in-place". Written before the
code, as WG6c and WG6d were.*

## What this sprint is, and why it is smaller than it sounds

All three pieces exist on the Mac, with designs and gates. **WG7 ports rather
than invents**, and the two hardest problems are already solved on the Windows
side by earlier sprints:

| the hard part | already solved by |
|---|---|
| the GUI must stay live while the primary is frozen in the halt loop | **WG4 D1's two-VM split.** The UI VM is a *different* VM on the main thread; the primary parks on its own thread and the pump never notices. `73_cocoadebugger.mst` says the same thing about the Mac: *"Because the UI is a SEPARATE VM on the main thread, the whole GUI stays live while the primary is frozen — no reentrancy work."* |
| getting a report across the seam | **WG4 D1's `#uiReq` inbox** plus WG5b-2's `winui_host.dll`. Both directions already carry payloads a view can render. |
| rendering source with the halted statement marked | **WG6d's cell grid.** A highlighted statement is cells with a different background — the same mechanism the selection already uses, and the renderer needs no new concept. |

The three slices are therefore mostly *view* work over seams that exist.

## The data rule, inherited and not re-argued

**Blast, don't patch.** The primary publishes its FULL report and the view
renders it wholesale; the Monitor's `monitorRows` answers the WHOLE table each
refresh. This is the rule 60's editor note settled for the Editor (an
incremental damage protocol *did* drift and was abandoned) and that
`85_cocoamonitor.mst` states for the Monitor. Nothing here re-litigates it.

## WG7-1 — the Debugger, fronted natively

`73_cocoadebugger.mst` is the model. Three panes over one report: the call
stack, the halted method's source with the current statement marked, and the
selected frame's variables. A step bar sends one command line back into the
parked loop (Step Into / Step Over / Finish / Continue / Abort), and a print
field evaluates an expression against the selected frame.

**Two Windows-specific decisions:**

* **The source pane is a `WinCodePane`** (WG6e), read-only, with the halted
  statement as a background run. That is the whole of "highlight the current
  statement" — no new renderer concept, and it is why WG6d landing first
  matters. It also makes the Debugger the second consumer of the cell grid,
  which is the point of WG6d-3's per-window renderers.
* **F5 / F10 / F11 are accelerators, not pane keys.** They must work while
  focus is anywhere in the shell, so they belong to the shell's own
  `WM_KEYDOWN` (WG5a D3's Ctrl-D/Ctrl-P path), NOT to a code pane's. A pane
  that swallowed F5 because it had focus would be the same class of defect as
  the splitter's mouse messages.

*Gate: the primary halts on a planted breakpoint, the Debugger fronts itself
with a stack, clicking a row changes the variables pane, Step Over advances
one statement and the highlight moves with it, and Continue resumes — with the
UI demonstrably live throughout (the metrics cluster keeps ticking while the
primary is parked).*

## WG7-2 — the Monitor

`85_cocoamonitor.mst` is the model, and its data path is already
platform-neutral: `macvm::embed::monitor_snapshot()` answers a `Vec<VmMonitorRow>`
fed by each VM's own thread, so nothing crosses into another VM. One host verb
answers the whole table.

**The row is: name, state, memory, GC, alloc/s, JIT activity**, plus the UI
BRIDGE band — drain passes, door entries, work items — which on Windows means
the counters WG2/WG3 already publish (`messagesSeen`, `drainPasses`,
`doorEntries`) rather than the Cocoa ones.

**ALLOC/S is derived HERE**, diffed from raw running totals, because only this
side knows the refresh period the user picked. That is `85`'s own note and it
is the only computed column.

**"Column priority"** (the SPRINTS row's phrase) means the table sheds columns
narrowest-first as the pane shrinks, so the Monitor stays readable in a narrow
window instead of scrolling horizontally.

*Gate: every VM in the roster appears — primary, UI worker, and a spawned
compute worker — the counts are RELATIONSHIPS not frozen integers (WG2 Δ 14),
ALLOC/S is non-zero under a load and returns to zero at rest, and the table
still reads at a narrow width.*

## WG7-3 — primary restart-in-place

The one piece with no Windows counterpart at all, and the one that unblocks
work already deferred.

`win_gui` boots its primary ONCE through `handshake_wire_vms` and holds it for
the life of the process: there is no teardown, no re-handshake, and nothing
that can hand a caller a fresh world. WG6c-3 recorded that gap when it left
**File In** and **Add to World** unbuilt — File In's contract is *a fresh
world, then your file*, and it cannot be honoured without this.

**What it needs:**

* tear down the wired pair (the primary thread, its inbox, the metrics
  channel) without taking the UI VM or the window with it;
* re-handshake, re-publish the inbox, and re-point the metrics cluster;
* park or fail any `#uiReq` that was in flight — `47_worker.mst` already
  records this hazard: *"a `#uiReq` that was in flight when its primary is
  restarted"* — so the reply path must answer rather than hang;
* leave every VIEW intact. A restart is not a window rebuild; the Browser
  simply refreshes from a primary that is now a different VM.

**The pitfall that is really the whole slice:** a restart must be
indistinguishable from a fresh boot *to the world*, and completely invisible
*to the window*. Anything that caches a primary-side identity across it — a
worker id, a cached tree, an in-flight correlation — is a defect that will
present as a view that silently stops updating.

*Gate: restart with a class defined only at runtime, and it is gone; restart
with `world/user_*.mst` present, and it is there; a `#uiReq` in flight across
a restart answers an error rather than hanging; the window never blinks and
`viewBuildCount` does not change. Then File In and Add to World, which are
this machinery with a file written first.*

## Ordering, and why

**WG7-3 first, or at least before WG7-1's gate.** The Debugger's own gate wants
to plant a breakpoint, halt, and resume repeatedly, and a restart is the
cleanest way to get back to a known world between runs. It is also the piece
with real risk (thread lifetime, in-flight requests), and finding that late
would be finding it while debugging the Debugger.

## Out of scope, decided rather than drifted into

* **Editing while halted.** The Debugger's source pane is READ-ONLY this
  sprint. Accepting a method into a parked frame is a real feature and a real
  can of worms (the frame's method may be mid-execution); it is not smuggled
  in behind a pane that happens to be editable.
* **Games/sound.** Still the recorded stretch — the Demos menu greys with a
  reason naming the design doc, never silently.
* **The splitter's mouse path** (`02907bf`) is unresolved and unrelated;
  it is not WG7's to carry.
