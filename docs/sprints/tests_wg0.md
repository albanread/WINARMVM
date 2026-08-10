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

---

> **Δ (2026-08-10, WG0 — BUILT).** Corrections to the rows above; the
> reasoning behind each is in `sprint_wg0_detail.md`'s own Δ.
>
> | row | as planned | as built |
> |---|---|---|
> | `testStructOffsets` | "all **16** `WNDCLASSW` field-offset queries answer; total size and `lpfnWndProc`@8" | `WNDCLASSW` has **ten** members / 72 bytes. All ten answer; the two invariants are `lpfnWndProc`@8 and **size == `lpszClassName`'s offset + 8** — `types.size_bits` cross-checked against `struct_fields.byte_offset`, two independent columns, which is why a size QUERY (primitive 271) had to exist. Offsets are also asserted strictly increasing. No offset is compared to a hardcoded number. |
> | stress: "a deliberately wrong struct size makes `RegisterClassW` fail" | — | unwritable: `WNDCLASSW` has no `cbSize` to mangle (that is `WNDCLASSEXW`). Replaced by a NULL `lpszClassName`, which `RegisterClassW` rejects; the test asserts a clean guest error naming the call and that the next real registration still succeeds. |
> | `testUtf16RoundTrip` | length 12 incl. NUL, units read back | as written, plus the empty string (terminator only) and a **surrogate-pair** case (U+1F600 → D83D DE00), since a Windows title's source is UTF-8 and the astral path is the half that rots unseen. |
> | `testRefPrints` | "prints handle + liveness honestly" | split in two: a Win32-free half (a non-`HWND` kind prints no liveness claim and RAISES when asked for one) and the live/dead half against a real window. |
> | Non-goals: "no Rust code at all if P5's surface suffices" | — | it did not suffice. P5 exposed winkb to the RESOLVER only; the constants and struct-layout tables were unreachable from Smalltalk. Four primitives (268–271) and one winkb function (`lookup_struct_size`) were added, carrying no policy. This is the sprint's headline finding. |
> | Gate item 6, "core suite counts unchanged" | — | the Rust test count is unchanged (no Rust test target was added); the in-language world suite grows by WG0's assertions on Windows only, and is byte-identical on macOS and in the DB-absent state up to the announced skip. |
> | gate item 2, "already-exists on re-run, tolerated with the class still usable" | tolerate by reading `GetLastError` | tolerate by ASKING — `GetClassInfoW`. The error-code version passes standalone and fails under the JIT (it read 203, `ERROR_ENVVAR_NOT_FOUND`): compilation, W^X `VirtualProtect` and code-cache `VirtualAlloc` all run between two guest FFI sends and reset the thread's last error. `GetLastError` is a diagnostic in this VM, never a control input. |
> | the ×20 stress | "the full ladder ×20 in one process" | as written — and it found a second thing: several VMs run in ONE process under `cargo test`, and a window class is per-process, so the probe class name is now unique per VM (stable within one, so the ×20 re-registration is still real). |
