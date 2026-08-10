# Sprint WG1 — Test Plan

## Acceptance gate

Every item is machine-checkable — §3.1's capture channel exists precisely
so "is there a window and does it look right" stops being a human question.

1. `macvm-winui` starts; a **visible** top-level window exists;
   `IsWindow(hwnd)` true; `GetWindowTextW` reads back the title Smalltalk
   set.
2. **Same-thread invariant**: the thread that created the window is the
   thread pumping the loop — asserted once at startup with
   `GetCurrentThreadId` / `GetWindowThreadProcessId`, not assumed.
3. `MACVM_WINUI_CTL=<port>` + `gui connect` → `ping` → `pong`.
4. `gui snap out.png` writes a PNG whose **dimensions equal the client
   rect** (`GetClientRect` read through the FFI in the same session). A
   file that merely exists is not evidence; the size is.
5. `gui doit "WinShell setTitle: 'WG1-OK'."` → `sleep` → the titlebar
   really changed, read back via `GetWindowTextW`.
6. Clean shutdown: a doit posts `WM_CLOSE`; the window goes away, the loop
   ends, the process exits **0**. Not killed, not timed out.
7. DPI: the client rect matches the DIP size scaled by
   `GetDpiForWindow`/96 (proves `PER_MONITOR_AWARE_V2` took effect).
8. Suites unchanged from WG0's close: Rust **1086/0/10**, world **7650/0**,
   in **both** database states.

## Unit / in-language tests (`world/tests/61_winui_shell_tests.mst`)

Windows-guarded (`== #windows`) via the runner, layer file loaded first
per WG0's Δ 6.

| test | assertion |
|---|---|
| `testArenaGivesStableAddresses` | two allocations differ, neither moves across a forced `Smalltalk gcScavenge` — the WG0 Δ 5 rule, now pinned rather than remembered |
| `testUtf16InArena` | `nativeUtf16:` round-trips through the arena; astral (surrogate-pair) case included |
| `testApiConstantsResolve` | `WS_OVERLAPPEDWINDOW`, `SW_SHOW`, `DWMWA_*` answer from winkb; an unknown name raises naming the constant |
| `testMsgStructOffsets` | `MSG`'s offsets come from winkb and strictly increase; size cross-checks the last field's offset + its width (WG0's Δ 3 invariant shape) |
| `testDarkModePreferenceIsReadNotInvented` | the registry read answers 0/1 or fails cleanly to light — never a hardcoded constant |
| `testShellWithoutWindowFailsCleanly` | `WinShell setTitle:` before `openMain` is a clean guest error naming the call, not a crash on a null HWND |

Window-creating tests live in the **binary's** gate (items 1–7), not in
`it_world`: WG0's Δ 11 showed several VMs share one process there, and a
visible window with a message loop is not something to open inside a
parallel test harness.

## Stress / negative

- **`GetMessageW` = −1**: force it (post to a destroyed HWND) and assert
  the loop exits rather than spins — the pitfall made a test.
- Open/close the window ×10 in one process: no class-registration leak
  (`GetClassInfoW` says usable each time), no HWND reuse surprise.
- `snap` **before** the window exists → `ERR no window yet`, not a hang and
  not a zero-byte PNG.
- A guest fault inside `openMain` (forced) recovers via P2 and the process
  still exits 0 through the control channel — the loop outlives a VM
  fault, which is exactly why Rust holds the pump (D4).

## Non-goals

WndProc dispatch, controls, painting, menus, the primary VM, COM — WG2+.
No view switching (there are no views yet). The window's *contents* are
deliberately empty: WG1 proves the frame, the thread and the camera.
