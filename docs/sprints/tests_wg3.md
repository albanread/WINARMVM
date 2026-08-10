# Sprint WG3 — Test Plan

Every gate item is driven from the **host's control drain**, never from a
doit — WG2 Δ3 made that permanent: a message that reaches Smalltalk must
originate outside every VM entry, and `SetWindowPos` from inside `exec` is
correctly refused by the guard. Counts are **relationships and deltas**, not
frozen integers (WG2 Δ14); tallies live in Smalltalk, because the vendored
RUSTTCL has no `for`/`%`/`eq` (Δ13).

## Acceptance gate

### Part 1 — the drain, before any control exists

1. **Transparency again.** With the drain installed and no control created,
   WG2's entire gate passes unchanged — window, title, DPI equality, snap
   dimensions and centre pixel, `lastSizeSeen` tracking, exit 0.
2. **Flags are serviced.** A `WM_SIZE` sets a flag; the next drain pass
   clears it and records the settled size. `WinShell lastLayoutSize` equals
   `GetClientRect` after the burst.
3. **Coalescing is real and measured.** Under a burst of N resizes,
   `drainPasses < sizeCount` — the ratio reported, not asserted to a
   constant. A ratio of 1.0 means the drain is not coalescing and the
   sprint's central claim is false.
4. **Tracking suppresses.** Simulate `WM_ENTERSIZEMOVE` → resize ×M →
   `WM_EXITSIZEMOVE`: **zero** drain passes run while tracking, and exactly
   one runs after, against the final size.
5. **The drain never re-enters.** A flag set *during* a drain pass is
   serviced by the heartbeat on a later pass, and the pass count does not
   run away (assert bounded, not exact).

### Part 2 — controls and layout

6. A Smalltalk-created `BUTTON` exists (`IsWindow` on its handle) and a
   synthesised click (`WM_COMMAND` with its id, posted from the drain
   context) sets a flag whose *meaning* runs in the next pass — proven by
   Smalltalk state, not by a Rust counter.
7. `WinLayout` places two controls; after a resize + drain their rects are
   the layout's computed values, read back with `GetWindowRect`.
8. **Themed, by pixel.** The snap shows a v6-themed button — assert a pixel
   inside the button differs from the unthemed system-face colour. A
   manifest that failed to embed is otherwise invisible.
9. `SysListView32` created and populated; a `WM_NOTIFY` arrives and its
   `NMHDR` fields are read **in the door** (the pointer dies on return) and
   surface as Smalltalk values in the next drain.
10. DPI: every control rect equals its layout value scaled by
    `GetDpiForWindow/96` — an equality, as WG1 established.

### Part 3 — unchanged

11. Rust **≥1099/0/10** both winkb-DB states, world **≥7762/0**,
    `macvm-gui` 104/0. `BusyGuard`'s move (D5) must not change any count.

## Unit tests (Rust)

| test | assertion |
|---|---|
| `drain_wake_is_post_not_send` | the wake is `PostMessageW`; a `SendMessageW` would re-enter synchronously |
| `tracking_flag_suppresses_drain` | flag set → drain requests are recorded but not serviced; cleared → exactly one pass |
| `busy_guard_is_inside_eval` | D5: a direct `VmHandle::eval` sets the busy flag without the caller doing it (the source-test backstop stays) |
| `control_id_range_checked` | an out-of-range id is refused before `SmallInt::new` sees it (Δ9) |

## In-language tests (`world/tests/63_winui_controls_tests.mst`)

| test | assertion |
|---|---|
| `testDrainClearsFlags` | flags set → `drainPass` → flags clear, work recorded once |
| `testLayoutComputesRects` | `WinLayout` on a known client size answers the expected rects — pure, no window needed |
| `testLayoutScalesWithDpi` | the same layout at 96 vs 144 dpi scales exactly |
| `testControlWithoutWindowFailsCleanly` | creating a control before `openMain` is a clean guest error naming the call |

## Stress / negative

- 500 resizes: coalescing ratio reported; arena does not grow without
  bound; depth guard 0 at the end; `snap` still correct mid-burst.
- Click storm on a button (200 synthesised `WM_COMMAND`s): every one either
  coalesces or is serviced; none lost silently, none double-counted.
- A raising handler inside a drain pass: the pass completes, the flag is
  cleared (no infinite retry), and the next pass runs normally.
- A fault inside a drain pass: P2 recovers, the window lives, later passes
  still run.

## Non-goals

The shell's chrome, the view switcher, the Transcript, the metrics
cluster, menus, painting beyond what themed controls do themselves, the
primary VM, COM.
