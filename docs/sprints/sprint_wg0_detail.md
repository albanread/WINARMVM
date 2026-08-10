# Sprint WG0 — FFI probe: user32 by hand

Objective: prove the whole chain a Smalltalk-authored Windows UI stands on —
guest Smalltalk → winkb-resolved import → A64 FFI trampoline → a real
user32/kernel32 call — before any window, loop, or door exists. Ends with a
window class registered, a (hidden) window created and destroyed, and a
`WNDCLASSW` built byte-by-byte from winkb's struct offsets via `Alien`.
Implements `win_gui_design.md` §2.2 (the handle layer's floor) and §3 row
WG0. Deliberately tiny: every later WG sprint assumes this one's facts.

## Prerequisites

- **P5 landed**: `resolve_ffi_symbol` resolves through winkb (or
  LoadLibraryA fallback); the ARM64 classifier models `g` scalars,
  pointers — including function-pointer params as plain `g` (the addition
  sent to P5 for exactly this sprint) — and 9–16-byte structs; pinning
  tests include `GetSystemMetrics` and `MessageBeep`.
- Existing: `Alien` (IndexableBytes-backed, typed accessors), the FFI
  pragma syntax, `world/tests` SUnit-lite, the `platformName` guard
  pattern (P0), `load_list` (CG1, tested on Windows).

## Deliverables

- `world/90_winui_probe.mst` — the probe classes, **loaded only via a new
  `world/winui.list`** (the `cocoaui.list` mechanism verbatim): a
  `WinProbe` class carrying the FFI pragmas and the doits below. Nothing
  enters `world.list`; the base world stays byte-identical.
- `world/tests/60_winui_probe_tests.mst` + a `winui`-guarded entry in the
  runner (extend `99_run_all.mst`'s `platformName` pattern — these run
  ONLY on `#windows`).
- A `WinRef` **sketch only** (class + handle wrapping + `printOn:`) — the
  full protocol is WG2's; here it exists so the probe's window handle has
  an honest home.

## Design

### D1. The call ladder, in proving order

1. `GetSystemMetrics(SM_CXSCREEN)` — int in, int out, zero risk; already
   pinned by P5. First guest-visible user32 value.
2. `MessageBeep(MB_OK)` — BOOL semantics, side effect audible.
3. `GetModuleHandleW(NULL)` — the HINSTANCE every registration needs;
   NULL pointer arg, pointer out.
4. `RegisterClassW(&wc)` — the sprint's substance: a **16-entry struct by
   layout**. Build the `WNDCLASSW` in an `Alien`, each field written at
   the offset **queried from winkb's `struct_fields`** (never hardcoded —
   the test asserts the queried offsets against the two invariants we can
   check locally: total size, and `lpfnWndProc` at offset 8). For v1 the
   wndproc field takes **`DefWindowProcW`'s own address** (resolved like
   any import) — no Rust trampoline yet, no dispatch; WG2 owns that door.
5. `CreateWindowExW(...)` — created **hidden** (no `WS_VISIBLE`): WG0
   proves handles, not pixels; a visible window belongs to WG1 where a
   loop can serve it. Returns the HWND into the `WinRef` sketch.
6. `IsWindow(hwnd)` → true; `DestroyWindow(hwnd)` → true;
   `IsWindow(hwnd)` → false — the lifecycle round-trip that IS the gate.

### D2. Constants come from the database

`SM_CXSCREEN`, `MB_OK`, `WS_OVERLAPPEDWINDOW`, `CW_USEDEFAULT` — resolved
from winkb's 97k `constants` at world-load time, not transcribed into
Smalltalk source. One helper (`WinProbe class >> constant:`) with a
guest-visible error naming the constant when absent. Transcribing numbers
by hand is how a port drifts; the database is present, use it.

### D3. Wide strings

Win32 is UTF-16 (`...W` entries). The probe needs class-name and title
strings: one `String>>asUtf16Alien` helper (append the NUL terminator;
answer the Alien) written here, tested here, reused forever after. Do NOT
take the `...A` entries as a shortcut — the design doc's Windows-materials
stance includes the character set.

## Implementation order

1. `winui.list` + empty probe class; loads via `load_list`, world suite
   untouched.
2. D2 constants helper + its absence error, tested against known values.
3. D3 `asUtf16Alien`, round-trip tested (write, read back, length).
4. Ladder steps 1–3 as doits + tests.
5. Step 4–6: struct build, register, create hidden, lifecycle asserts.
6. Un-guard the world-runner entry; full suite both DB states (the
   probe's winkb lookups fail CLEANLY to pragma fallback when the DB is
   absent — which for these entries means the test SKIPS with a named
   reason, same announced-skip discipline as `posix-only:`).

## Pitfalls

- **`RegisterClassW` fails with `ERROR_CLASS_ALREADY_EXISTS` on re-run** —
  the probe unregisters (`UnregisterClassW`) in an ensure-style cleanup,
  and the test tolerates the already-exists error on the second
  registration by asserting the class is usable either way. Test suites
  re-run in one process; a probe that passes once and fails forever after
  is a trap for the next sprint.
- **`CreateWindowExW` on a non-main thread**: fine for a hidden,
  loop-less, immediately-destroyed window — but say so in the comment,
  because WG1 moves window ownership to the hosted main-thread VM and
  someone will wonder why WG0 got away without it.
- **GetLastError discipline** (P5's rule): read it immediately after the
  failing call in the SAME primitive round-trip if error detail is
  wanted; v1 asserts success and names the call on failure.
- The struct-offset test must not assert all 16 winkb offsets against
  hardcoded numbers — that would just re-transcribe the headers. Assert
  the two checkable invariants (size, one landmark offset) and that all
  16 queries ANSWER; trust the database for the rest — it is the design.

## Out of scope

- Any message loop, any visible window, any WndProc other than
  `DefWindowProcW` (WG1/WG2). Mica/DWM calls (WG1). Controls (WG3).
  The `win_gui` crate itself — WG0 is world-side only, driven through
  the existing `macvm` CLI.
