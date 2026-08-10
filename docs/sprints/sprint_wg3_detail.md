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

---

> **Δ (2026-08-10, WG3 — BUILT; what measurement corrected).** The drain is
> real and it coalesces: **200 `WM_SIZE` messages in one burst produce 3 drain
> passes and exactly 1 layout — a pass/message ratio of 0.015**, with the layout
> run against the *settled* client rect and `GetClientRect` agreeing. Tracking
> suppresses completely: **0 passes during 30 resizes inside a modal loop, one
> layout after**, driven by real `WM_ENTERSIZEMOVE`/`WM_EXITSIZEMOVE` sent from
> the host's control drain. A `BUTTON`, an `EDIT` and a `SysListView32` are
> created by Smalltalk, laid out by `WinLayout` (whose computed rects equal
> `GetWindowRect`'s to the pixel), and a synthesised click is *recorded* by the
> door and *meant* by the drain — proven by holding the drain off and watching
> the click sit in the queue with nothing done about it. `WM_NOTIFY`'s `NMHDR`
> is read in the door and surfaces one pass later as `#(<listHWND> 102
> 4294967195)`. The button's face measures **253,253,253** where the unthemed
> `COLOR_BTNFACE` is **240,240,240**. `BusyGuard` now lives inside the VM's own
> entry points and no caller needs an un-guarded form. Twenty-two corrections,
> in the order WG4 will meet them.
>
> **The gate, and the two older gates it had to move.**
>
> 1. **`tests_wg3.md` item 1 cannot hold as written, and its own wording is the
>    fix.** "WG2's entire gate passes unchanged" is impossible once this window
>    has controls in it, for two independent reasons that took one run each to
>    find: a themed `SysListView32` paints **white** pixels exactly where WG1's
>    and WG2's gates read the window's background fill (243,243,243 — the centre
>    pixel WG2's Δ 6 went to such trouble to locate correctly), and it sends
>    `NM_CUSTOMDRAW` once per paint through a door those gates count messages
>    at (`entries=200`, `messagesseen 40`). The item's own phrase — "with the
>    drain installed and **no control created**" — is the answer, and it needed a
>    mechanism: **`MACVM_WINUI_CONTROLS=off`**, read by the host and applied as
>    `WinShell controlsEnabled: false` before `openMain`. `gate-wg1` and
>    `gate-wg2` now set it and go on testing the configuration they were written
>    against, rather than being loosened to accommodate a later sprint. **This is
>    the general shape WG4 will meet again**: every sprint that adds pixels or
>    messages to this window invalidates an earlier gate's reading of it, and the
>    honest fix is a switch that restores the earlier configuration, not a weaker
>    assertion.
> 2. **`gate-wg2`'s item-2 cross-check could never have passed, and WG3 had to
>    fix it because `gate-wg3` chains it.** `LAST=$(grep '^WG2 lastsize ' … | cut
>    -d' ' -f3 | tr -d '#()')` takes the FIRST field of a two-field `Array`
>    `printString`: `#(684 461)` becomes `684`, and `test "$LAST" = "$CL"`
>    compares it against `684 461`. `f3-` is the whole fix. This is the same
>    shape of defect WG2's own Δ 6 found in `gate-wg1` one sprint earlier, and
>    the lesson is the same one stated more sharply: **a gate line that cannot
>    pass is indistinguishable, from outside, from one that has not been run.**
>    Every WG3 gate line that reads a multi-field value uses `f3-`.
>
> **D5 — the guard's move, which is the change with the largest blast radius.**
>
> 3. **D5 names `eval`/`exec`. There are SIX.** `render_fragment`,
>    `fire_widget_action`, `eval_to_string` and `eval_to_bytes` each claim the
>    same one-per-thread `sigsetjmp` slot with their own inline `sigsetjmp`, and
>    the hazard is identical in all of them — a Win32 call from inside any of
>    them that SENDS a message re-enters the VM with the door's depth counter
>    legitimately 0. All six now take the guard, through one
>    `embed::host_entry_guard()` that compiles to nothing off Windows.
> 4. **`dispatch_callback` must NOT be guarded, and that is a decision.** It IS
>    the door's own entry (and the Cocoa delegates'), it has its own
>    re-entrancy guard, and marking it busy would make the drain pass decline
>    every message its layout provokes for the *wrong* reason — the depth guard
>    already declines them for the right one.
> 5. **No caller legitimately needs the un-guarded form**, which was D5's open
>    question. Measured: both winkb-DB states green, `macvm-gui` 104/0,
>    `image_store` 55/0, world 7827/0, and the listed core test set moved by
>    exactly +5 (the five tests this sprint added). The host's hand-written
>    brackets in `win_gui/src/main.rs` are now redundant and were **kept**, as
>    the sprint asked — `BusyGuard` is a counter, so double entry is safe, and
>    `every_vm_entry_is_bracketed` stays as the backstop.
>
> **D1 — what the drain's mechanism actually needs, beyond the diagram.**
>
> 6. **The wake needs a LATCH, not just a post.** D1's diagram says "set flag +
>    `PostMessage(WM_APP_DRAIN)`", and that alone coalesces the WORK while
>    leaving the PASSES one-per-message: N messages queue N wakes, N−1 of which
>    find nothing to do. `drainPasses < sizeCount` is the gate, so the latch —
>    "a wake is already in the queue" — is not an optimisation, it is the
>    measurement. With it: 200 requests, **1 post**.
> 7. **A heartbeat with nothing to do must cost NO VM entry.** Otherwise
>    `drainPasses` counts clock ticks and the coalescing ratio becomes a
>    measurement of the timer. So Rust — not Smalltalk — holds the "deferred work
>    exists" bit; `WM_TIMER` reads it and returns without entering the VM when it
>    is clear. That is why the drain-requesting set (`WM_SIZE`, `WM_COMMAND`,
>    `WM_NOTIFY`) is a Rust-side constant: `dispatch_callback`'s single `u64`
>    return is already spoken for by the LRESULT, so a handler cannot say "and
>    drain me" in the same breath.
> 8. **The guest's escape hatch is the OTHER end: `drainPass` answers non-zero
>    for "run me again".** That is the whole of this sprint's "do not post the
>    drain message from inside the drain" pitfall, made a mechanism: the request
>    is left standing and the *heartbeat* services it, so a handler that always
>    asked again could not spin the pump. One-shot in the guest, bounded in the
>    gate.
> 9. **`WM_APP_DRAIN` and `WM_TIMER` are handled by the trampoline BEFORE the
>    allowlist and never reach `WinShell`.** They are the drain's plumbing, not
>    messages whose meaning is Smalltalk's, and they must behave identically
>    whether our pump dispatched them or a modal loop did. `WM_TIMER` also falls
>    through to `DefWindowProcW` afterwards, so any *other* timer (a `TIMERPROC`
>    in `lParam`, which WG4 may want) keeps working exactly as it would have.
> 10. **The drain must decline while `vm_busy()` or `depth() > 0`, and LEAVE THE
>     REQUEST STANDING.** Clearing it on a declined pass loses the work; the
>     heartbeat is what makes "declined" a delay rather than a drop.
>
> **D2 — tracking, and the three things the flag has to be that a flag is not.**
>
> 11. **`tracking` is a COUNTER, not a boolean.** A menu loop can open inside a
>     size loop, and a boolean would be cleared by the inner one's exit while the
>     outer was still dragging. `tracking_nests` is the test.
> 12. **It must be set in the trampoline BEFORE any guard**, because it has to
>     hold even when the VM is busy and the Smalltalk entry is declined: the
>     modal loop is starting whether or not Smalltalk gets to hear about it.
> 13. **`WM_EXITSIZEMOVE` must RE-POST**, because `request_drain` deliberately
>     does not post while tracking (a post during a drag would be dispatched by
>     the modal loop's own pump, immediately suppressed, and repeated per
>     mouse-move). One post on the way out is what makes "exactly one pass after"
>     true at all.
> 14. **Item 4's "exactly one runs after" is not assertable as one PASS, and the
>     thing that IS exactly one is the LAYOUT.** Paints request drains too
>     (see 17), so the gate asserts `layoutCount == 1` against the final size and
>     bounds the pass count. That is the honest form of the claim.
>
> **D3 — `WM_NOTIFY`, whose payload lifetime is the sprint's named trap.**
>
> 15. **`NMHDR.idFrom` is a `UINT_PTR` and must convert UNSIGNED.** `as i64`
>     turns an out-of-range id into a *plausible negative one* instead of
>     refusing it — which is the exact shape of bug Δ9 is about, arriving through
>     the conversion rather than through `SmallInt::new`. `control_id_range_checked`
>     caught it on its first run, against a fabricated `NMHDR`.
> 16. **The `NMHDR` read must happen INSIDE `dispatch_callback`, not before it.**
>     "Read the fields in the door" is right, but a raw pointer read outside the
>     recovery window is a dead process where inside it is a recovered fault
>     answering `DefWindowProcW`. The three-field read is the only pointer
>     dereference in this layer and it is inside P2's net.
> 17. **`NM_CUSTOMDRAW` is the storm class, arriving through an allowlisted
>     message.** It is a drawing callback wearing a notification's clothes and a
>     themed list view sends one per paint. It cost nothing here (4 in a whole
>     session; a pass is ~9 µs release) but it is the first thing WG4 should look
>     at if the drain ever looks busy, and it is why `WinShell` records it without
>     treating it as news.
> 18. **`WM_NOTIFY` needs its own selector.** The four-word signature has no room
>     for a decoded payload, so the door sends `window:notifyFrom:id:code:` —
>     four `SmallInteger`s, no pointer, no new marshalling.
>
> **D4 — layout, and two arithmetic traps that produce coordinates, not errors.**
>
> 19. **Smalltalk's binary operators are strictly left to right, and it cost a
>     coordinate.** `left + frame // 2` is `(left + frame) // 2`. The first
>     `clientOrigin` answered 112 where 216 was right; nothing about it looked
>     wrong and only the layout comparison said so. Every intermediate in that
>     method is now a named temporary. **This is a dialect-wide hazard and WG4
>     will write more arithmetic than WG3 did.**
> 20. **`GetClientRect` on a CHILD answers its size at (0,0) and says nothing
>     about where it is.** A control's position needs `GetWindowRect` in screen
>     coordinates minus the client area's own screen origin — and that origin has
>     to be MEASURED from the window and client rects the way `applyDips`
>     measures the frame, not read off `windowRect`'s left/top, which is wrong by
>     exactly the border on every window that has one.
> 21. **Layout must be computed in DIP space and scaled ONCE at the end**, or
>     "the same layout at 1.5× the DPI is 1.5× the layout" is only approximately
>     true — each band would round separately. WG1 established the DPI contract
>     as an equality; this is what keeps it one. As with WG1, the scaling is
>     exercised **synthetically** (this machine is 96 dpi): `rectsIn:by:dpi:` at
>     96 and at 144 is compared directly, which is a property of the arithmetic
>     rather than of the monitor.
>
> **D6 — the manifest, and how to know it took.**
>
> 22. **The pixel probe must NOT be the button's centre.** A push button's centre
>     is where its text is, and ClearType's subpixel antialiasing puts a strongly
>     coloured pixel there — **57,57,143 measured** — on themed and unthemed
>     alike, so a centre probe would pass this test for the wrong reason. One
>     sixth of the way across measures the FACE: **253,253,253** themed against
>     **240,240,240** (`GetSysColor(COLOR_BTNFACE)`) unthemed, and 243,243,243
>     for the window fill showing through, which is the second half of the
>     assertion. The manifest itself is embedded with two linker arguments
>     (`/MANIFEST:EMBED /MANIFESTINPUT:`) from a `build.rs`, not with a new build
>     dependency; DPI is deliberately left out of it so
>     `SetProcessDpiAwarenessContext` stays Smalltalk's decision.
>
> **Three things about the gate itself, which WG4 inherits wholesale.**
>
> * **Coalescing is invisible at low rate, and that is CORRECT.** One `gui
>   resize` per round trip lets the pump service each wake before the next
>   message arrives, so the ratio is 1.0 — and it *should* be: the drain is not
>   meant to skip work nothing is racing. The claim is about bursts, so the gate
>   needed `gui burst <n>` (N `SetWindowPos` calls with no pump turn between
>   them). **A gate that measured the ratio the obvious way would have reported
>   1.0 and concluded the sprint had failed.**
> * **Observing "recorded but not yet meant" needs D2's own lever.** Every
>   control request the script makes lets the pump turn and service the wake, so
>   the intermediate state is unobservable from a script — unless the drain is
>   held off. `gui track on` puts the door in exactly the state a window drag
>   puts it in, and item 6's four load-bearing lines (`btn-queued-in-door 1`,
>   `btn-clicks-in-door 0`, `btn-passes-in-door 0`, then `btn-clicks-after-drain
>   1`) are read inside it.
> * **The host's control drain is now a general facility, not a set of
>   per-feature verbs.** WG2 added `resize` because a `WM_SIZE` must originate
>   outside every VM entry; WG3 generalised it to `gui send [<id>] <msg> <wp>
>   <lp>` (any message, to the window or to a child by `GetDlgItem`), plus
>   `drain`, `track` and `burst`. WG4 should need no new host verb at all: the
>   guest builds the struct (it owns the arena), the host sends the message (it
>   is the only place outside every VM entry). `selectRequestFor:` answering an
>   arena ADDRESS that the host then points a `LVM_SETITEMSTATE` at is the
>   pattern.
>
> **What Rust needed, and why.** One `PostMessageW` extern in the core door (the
> wake; `drain_wake_is_post_not_send` exists because a send and a post have the
> same call shape and only one of them is safe). `GetDlgItem`/`SendMessageW`/
> `PeekMessageW(PM_REMOVE)` in the host, for the control drain's synthesised
> messages and for dispatching them before the verb answers. The allowlist grew
> from six messages to **eleven** (`WM_NOTIFY` plus the four modal-loop
> transitions), transcribed under Δ12's rule with `testWg3MessagesMatchWinkb` as
> the compensating control, and `testNotifyOffsetsMatchWinkb` doing the same job
> for the three `NMHDR` offsets the pointer read hard-codes. No new primitive:
> WG2's 272 and WG0's 268–271 were enough.
>
> **One thing this sprint did NOT fix, reported rather than absorbed.** `just
> gate-wg3` chains all the way back to `gate-p00: ci` → `ci: lint test` →
> `lint: cargo fmt --check`, and **`cargo fmt --check` fails at HEAD with 454
> diffs across the tree** — three of them in `win_gui/src/main.rs` alone, in
> WG1/WG2 code. So the full chain has been red at `lint` since before this
> sprint, for a reason no WG gate is about. WG3's own new code is `rustfmt`- and
> `clippy`-clean (`src/runtime/win_wndproc.rs` is now clean end to end), and the
> four WG gate bodies were run directly and are green; the tree-wide format
> sweep is a separate, mechanical change that would bury this sprint's diff and
> is left for whoever wants to own the churn.
>
> **Latency, for the record.** The door's `WM_SIZE` round trip measured **~540
> µs in debug** in this sprint's runs, matching WG2's own debug figure (540 925
> ns) to within noise and confirming its warning that a WG3 developer reading
> half a millisecond in a debug session should not conclude the door is
> unusable. A drain pass is one such entry, so at release rates a coalesced
> 200-message burst costs about **9 µs of Smalltalk**, against 200 × 8.8 µs =
> 1.8 ms if each message had done its own layout. That 200× is the number the
> whole architecture buys, and it is larger than the door's own 154× overhead —
> which is the quantitative reason flag-and-drain is worth its complexity.

---

> **OPEN REGRESSION at WG3's commit (2026-08-10) — bisected, not diagnosed.**
>
> `it_world::world_suite_at_sub_floor_threshold_survives_root_block_deopt`
> **fails** with WG3's changes applied. It dies with a *fatal* guest error —
> `does not understand add:` — after ~1,400 compiles.
>
> What is established, by measurement:
>
> - **It is not WG3's Smalltalk.** Commenting `../92_winui_controls.mst` and
>   `63_winui_controls_tests.mst` out of `tests.list` and re-running: still
>   fails.
> - **It is WG3's Rust.** `git stash` of the whole working set → the same
>   test **passes** at HEAD (WG2). Restore → fails again.
> - **It is sub-floor only.** The identical corpus at every *supported*
>   threshold is clean: `MACVM_JIT=off`, `threshold=20` and
>   `threshold=1000` each give **7827 run, 0 failed**. Only
>   `JitMode::Threshold(2)` — the sanctioned Rust backdoor this test uses
>   deliberately, because the shape it hunts is what fast compilation
>   produces — reproduces it.
> - WG3's core diff is three files: `embed.rs` (+51, the D5 guard at six
>   entries), `win_wndproc.rs` (+659, the door/drain), `rusttcl/verbs.rs`
>   (+45, host verbs). **`deopt_trap.rs` is untouched**, so
>   `claim_jmp_slot` itself did not change.
>
> The prime suspect is therefore **D5** — `host_entry_guard()` now wraps six
> top-level entries (`embed.rs` lines 918, 973, 1146, 1206, 1253, 1300),
> taken *before* `claim_jmp_slot()`. `VM_BUSY` is read in exactly one place
> (`win_wndproc.rs:349`, the door's decline test), which is why this is a
> suspect rather than a diagnosis: on that reading the guard should be
> inert in a test binary with no window. Something about adding it to those
> six sites is nevertheless the difference, and the next session should
> start by re-adding them one at a time.
>
> **Why this is committed red rather than hidden.** The failing test is the
> canary the root-block deopt fix left behind, and it is doing precisely its
> job — catching a sub-floor JIT/entry interaction that every supported
> configuration hides. Marking it `#[ignore]` would delete the only
> instrument that sees this class of bug, which is the opposite of what the
> last three defects in this family taught. WG3's own deliverables are
> verified independently and in the whole supported envelope.
