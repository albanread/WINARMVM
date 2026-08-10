# Sprint WG3 — the drain pass, then controls and layout

Objective: stand up the **flag-and-drain pass** the environment's whole
message architecture rests on (`win_gui_design.md` §2.4a), then put real
Win32 controls behind it — created by Smalltalk, notifying through WG2's
door, laid out by a Smalltalk layout object on `WM_SIZE`, under visual
styles v6 and per-monitor DPI.

**Order matters and is not negotiable:** the drain lands *before the first
control exists*. Controls are what generate the notification storm the
drain exists to absorb, and retrofitting the pattern afterwards is exactly
how the Cocoa side acquired its "browser shows no data" scars.

## Prerequisites

- **WG2 landed**; its Δ is required reading — sixteen corrections. Five
  bear directly:
  - **Δ2: there are TWO re-entrancy sources.** `CreateWindowExW`,
    `SetWindowPos` and `DestroyWindow` **send messages synchronously**, so
    the door is entered with depth legitimately 0 while a host `eval` is
    live — and `claim_jmp_slot` gives one slot per thread, so a nested
    `sigsetjmp` overwrites the outer recovery buffer. `BusyGuard` brackets
    every host `eval`/`exec` today. **WG3 owns moving it into `eval`/`exec`
    themselves** (D5) — a core change, deliberately deferred to here.
  - **Δ3: a message that reaches Smalltalk must originate OUTSIDE every VM
    entry.** Driving a control from a doit sends a synchronous message
    into a VM already inside `exec`, which the guard correctly refuses.
    Every WG3 test drives from the host's control drain. **This is now a
    permanent constraint on every WG gate.**
  - **Δ12: the allowlist's message numbers are transcribed in Rust**,
    necessarily — winkb is not reachable from the door's hot path. WG3's
    new messages join that list the same way, with the same comment.
  - **Δ13: the vendored RUSTTCL has no `for`, no `%`, no `eq`/`ne`, no
    `string`.** Put tallies in Smalltalk, which makes a better test anyway.
  - **Δ14: frozen integer counts in a gate cannot hold** — assert
    relationships and deltas.
- `win_gui_design.md` **§2.4a is the specification for D1–D2**; read it
  before writing a line.

## Deliverables

- The drain: a `WM_APP+n` post + `WM_TIMER` heartbeat, tracking
  suppression, and `WinShell class>>drainPass` servicing flags with fresh
  top-level entries.
- `world/92_winui_controls.mst`: `WinControl` (a handle + its notification
  identity), `WinLayout` (the Smalltalk layout object), and the concrete
  control wrappers WG3 needs.
- Visual styles v6 (a manifest) and DPI-aware metrics.
- `world/tests/63_winui_controls_tests.mst`; `just gate-wg3`;
  `docs/gallery-win/wg3-controls.png`.

## Design

### D1. The drain pass — first, and alone

```
door (wndproc)          →  record + set flag + PostMessage(WM_APP_DRAIN) + return
WM_APP_DRAIN / WM_TIMER →  if tracking { return }         ← D2
                           WinShell drainPass             ← fresh top-level entry
```

`drainPass` reads the flags Smalltalk set, does the real work, clears
them. It runs with the VM quiescent and **never inside a callback**, which
is the entire property the architecture buys.

Two rules the Cocoa side learned the hard way and states in its own
comments:

- **Flags coalesce.** Ten resizes before a drain produce one layout pass
  against the *settled* state, not ten. That is the design doc's
  "coalesce in the shell, dispatch the settled state" made real.
- **The drain is the only place a `VmHandle` may be dropped or swapped**,
  because it is the only point where the VM is provably not running.
  WG3 does not need that yet; WG7's primary-restart does, and it inherits
  this pass.

**Prove the drain before any control exists**: a flag set from a
`WM_SIZE` handler is serviced on the next pass, with a counter showing
*fewer* drains than messages under a resize burst. That coalescing ratio
is the gate.

### D2. Tracking suppression — the one thing Windows does not give us

Cocoa restricts the drain to `NSDefaultRunLoopMode`, so it never runs
mid-tracking. **Windows has no equivalent**: a modal move/size loop and a
menu loop pump the queue themselves, so both posted messages *and*
`WM_TIMER` fire inside them. Without suppression the drain would run a
full layout pass on every mouse-move of a window drag, from inside a modal
loop.

So: set a tracking flag on `WM_ENTERSIZEMOVE` / `WM_ENTERMENULOOP`, clear
it on `WM_EXITSIZEMOVE` / `WM_EXITMENULOOP`, and make the drain a no-op
while it is set — flags accumulate and one pass runs on exit. Add those
four messages to the allowlist (Δ12's rule applies: transcribed, commented).

> The window will therefore not re-layout *during* a drag. That is the
> correct v1 trade and matches what the flag-and-drain design is for; a
> later sprint wanting live-drag layout can drain on a throttle inside
> tracking, having measured that it is affordable.

### D3. Controls — created by Smalltalk, identified by id

`WinControl` wraps an HWND created with `CreateWindowExW` against a system
class (`BUTTON`, `EDIT`, `SysListView32`, `SysTreeView32`), with a child
id. `WM_COMMAND`'s `wParam` low word carries that id, so the door's
existing `WM_COMMAND` arm becomes: look up the control, set its flag,
return. **The control's *meaning* runs in the drain.**

`WM_NOTIFY` (which list/tree views use) carries a pointer to an `NMHDR` —
so it needs the arena's struct-reading path, and it is the first message
whose payload must be read *before* the door returns, because the pointer
is only valid during the call. **Read the few fields into Smalltalk values
in the door; do not stash the pointer.** That is a real constraint, stated
now because it is invisible until it corrupts something.

### D4. Layout in Smalltalk

`WinLayout` holds a tree of frames with a `layoutIn:` protocol; `WM_SIZE`
flags, the drain runs the layout, and each control is moved with one
`SetWindowPos` (or `DeferWindowPos` if measurement says the flicker
warrants it). All metrics scale by `GetDpiForWindow / 96` — WG1 proved the
plumbing; WG3 is the first sprint whose *content* depends on it.

### D5. `BusyGuard` moves into `eval`/`exec` (WG2 Δ2)

Today the host brackets every `eval`/`exec` by hand, enforced by a source
test. That is a discipline, not a mechanism — and the class of bug it
prevents (a nested `sigsetjmp` overwriting the outer recovery slot) is
silent. Move the guard inside `VmHandle::eval`/`exec` so it cannot be
forgotten, keep the source test as a backstop, and note in the Δ whether
any caller legitimately needs the un-guarded form.

### D6. Visual styles and the manifest

Common controls v6 needs an assembly manifest, or `SysListView32` renders
in its Windows-95 skin. Embed it in `macvm-winui` (a `.rc`/`build.rs` step
or the linker pragma). Verify by **pixel**, not by faith: a themed button
is not the same colours as an unthemed one, and the snap can prove it.

## Implementation order

1. The drain pass + tracking suppression + its coalescing gate. **No
   controls.** The window must still behave exactly as WG2's.
2. D5's `BusyGuard` move (core change, small, and everything after depends
   on it being right).
3. The manifest + one `BUTTON`, created by Smalltalk, whose click sets a
   flag serviced by the drain.
4. `WinLayout` + `WM_SIZE`-driven placement of two controls.
5. `SysListView32` with `WM_NOTIFY` — the D3 payload-lifetime constraint.

## Pitfalls

- **`WM_TIMER` is low-resolution and coalesces already**; do not use it as
  the primary wake. `PostMessageW(WM_APP_DRAIN)` is the wake; the timer is
  a heartbeat that catches a flag set with no message following.
- **Do not post the drain message from inside the drain.** A flag set
  during a drain pass is serviced by the *heartbeat*, or the pass loops.
- Control creation itself sends messages (Δ2) — create controls **in the
  drain or at open time**, never from a doit.
- `SysListView32` requires `InitCommonControlsEx` before first use.
- Δ9: `SmallInt::new` panics out of range; a control id or an `NMHDR`
  field is a `u32` that must be range-checked before it becomes a smi.

## Out of scope

Painting the shell's own chrome (WG4), the view switcher, the Transcript,
the metrics cluster, menus, the primary VM, COM.
