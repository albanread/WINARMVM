# Sprint WG0 — Test Plan

## Acceptance gate

All headless, all driven through the existing `macvm` CLI on Windows ARM64,
DB present:

1. `GetSystemMetrics(SM_CXSCREEN)` answers a plausible positive integer
   from a Smalltalk doit, `SM_CXSCREEN` itself resolved from winkb's
   constants table (not transcribed).
2. The `WNDCLASSW` is built in an `Alien` at winkb-queried field offsets;
   `RegisterClassW` succeeds (or already-exists on re-run, tolerated with
   the class still usable); `CreateWindowExW` (hidden) answers a real
   HWND; `IsWindow` true → `DestroyWindow` true → `IsWindow` false.
3. `UnregisterClassW` cleanup runs even when an intermediate step fails
   (ensure-style), so the suite is re-runnable in one process.
4. The world suite is unchanged on macOS and on Windows-without-the-list
   (`winui.list` is additive; `world.list` untouched — byte-identical
   base world).
5. **DB absent**: the probe's tests SKIP with a named reason (announced,
   never silent — the `posix-only:` discipline); nothing fails.
6. Core suite counts unchanged from P5's close.

## Unit / in-language tests (`world/tests/60_winui_probe_tests.mst`)

| test | assertion |
|---|---|
| `testConstantResolves` | `WinProbe constant: 'SM_CXSCREEN'` = the known value 0 (landmark); an unknown name raises the naming error |
| `testUtf16RoundTrip` | `'macVM' asUtf16Alien` length = 12 bytes incl. NUL; code units read back correct |
| `testMetricsCall` | screen width > 0, = the value from a second call (stability) |
| `testBeep` | `MessageBeep(MB_OK)` answers true (audible side effect not asserted) |
| `testStructOffsets` | all 16 `WNDCLASSW` field-offset queries answer; total size and `lpfnWndProc`@8 match the two locally-checkable invariants |
| `testWindowLifecycle` | the D1 step-4–6 ladder end to end, with cleanup |
| `testRefPrints` | the `WinRef` sketch prints handle + liveness honestly |

## Stress/negative

- The full ladder ×20 in one process (re-registration tolerance is the
  point — pitfall #1 made a test).
- A deliberately wrong struct size (test-only mangled Alien) makes
  `RegisterClassW` fail and the failure is a clean guest error naming the
  call, not a crash — rides P2's recovery.

## Non-goals

Visible windows, message pumping, WndProc dispatch, controls, DWM — WG1+.
No `win_gui` crate changes; no Rust code at all if P5's surface suffices
(the sprint is deliberately a WORLD-side proof).
