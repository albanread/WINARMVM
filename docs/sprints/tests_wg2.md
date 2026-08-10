# Sprint WG2 — Test Plan

## Acceptance gate

Scripted through `MACVM_WINUI_CTL` (WG1 Δ8: `eval` answers inline, so every
item below reads a value rather than sleeping and hoping).

1. **Transparency first.** With the door registered but every message
   `DefWindowProcW`'d, WG1's entire gate still passes unchanged — window,
   title, DPI equality, snap dimensions and the client-origin pixel, exit
   0. A door that changes behaviour before it does anything is a door with
   a bug in it.
2. **A message reaches Smalltalk.** `WM_SIZE` handled in `WinShell`: a
   scripted resize (`SetWindowPos` via FFI from a doit) changes a value
   Smalltalk computed — `WinShell lastSizeSeen` equals the new client
   rect, read back by `eval`.
3. **`WM_DESTROY` posts the quit from Smalltalk**, not from Rust: closing
   ends the loop and the process exits **0**.
4. **A raising handler does not break the window.** Force `WinShell` to
   `error:` inside a `WM_SIZE` handler; the resize still completes
   (`DefWindowProcW` answered), and the **next** message dispatches into
   Smalltalk normally — proven by `lastSizeSeen` updating on a second
   resize.
5. **A faulting handler does not break the process.** Force a wild deref
   inside a handler; P2 recovers, the window stays live, the control
   channel answers, and a subsequent resize still reaches Smalltalk.
6. **Nesting degrades safely.** A handler that calls `SendMessageW` on its
   own window sees the nested message answered by `DefWindowProcW`; the
   depth guard returns to zero afterwards (`eval` reads the counter
   through a debug verb, or a Rust unit test asserts it).
7. **Latency recorded** (D5): door round-trip vs the `DefWindowProcW`
   baseline, both warm, in the sprint Δ.
8. Suites unchanged: Rust **1086/0/10** both DB states, world ≥ **7723/0**,
   `macvm-gui` **104/0**.

## Unit tests (Rust, `win_gui`)

| test | assertion |
|---|---|
| `door_address_is_stable_and_nonzero` | the primitive answers the same non-zero address twice |
| `depth_guard_blocks_reentry` | a simulated nested entry takes the `DefWindowProcW` path without touching the VM |
| `depth_guard_drops_on_early_return` | the counter returns to 0 on the error path, not just the happy one — the leak this sprint most fears |
| `allowlist_is_a_closed_set` | a message not on D1's list never reaches the VM entry point (probe with a counter) |
| `naive_getmessage_predicate_is_wrong` | inherited from WG1; keeps the −1 bug from being "simplified" back in |

## In-language tests (`world/tests/62_winui_door_tests.mst`)

Windows-guarded; layer file loaded first (WG0 Δ6 ordering).

| test | assertion |
|---|---|
| `testUnhandledMessageAnswersDefault` | `WinShell` answers `#defwindowproc` for a message it does not handle — the two-sided allowlist agreeing in the safe direction |
| `testHandlerReturnsLResult` | a handled message answers an Integer, and the door's contract accepts it |
| `testHandlerStateSurvives` | handler-set class state is readable afterwards (proves it ran in the real VM, not a copy) |

## Stress / negative

- 200 scripted resizes in a row: `lastSizeSeen` tracks the final rect, the
  depth guard is 0 at the end, no leak in the arena.
- Raise inside a handler ×20 interleaved with good messages: every good one
  still dispatches.
- Fault inside a handler ×5: the process is alive after all five and exits
  0 on request.
- `snap` during a burst of dispatches: still a correct-sized PNG (the door
  must not starve the control channel's drain).

## Non-goals

Painting, controls, layout, menus, true re-entrant dispatch, any message
off D1's allowlist. WG2 proves the door; WG3 puts things behind it.
