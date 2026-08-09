# Sprint P5 — Test Plan

## Acceptance gate

1. A guest-Smalltalk FFI call to a real Win32 import succeeds end to end
   through pragma → winkb (or pragma-fallback) → trampoline → execution:
   `GetTickCount64` returns a plausible, monotonic value across two calls.
2. Every MODELLED classifier row has a passing pinning test that executes
   a real function of that shape; every REFUSED row has a test asserting
   a clean, reason-naming guest-visible error (never a garbage call).
3. `Time class>>now` and `Date class>>today` answer correctly (cross-check
   against the host clock within tolerance); the world file's Windows
   branch is the only world diff (flagged for upstream cherry-pick).
4. The four world FFI files are settled per the D5 table; the world
   runner's bookkeeping names each settlement; total counts reconcile
   against the Mac run with the settlements applied.
5. Suite green with the DB PRESENT and with the DB ABSENT (renamed aside
   in the test env) — the fallback contract is load-bearing.
6. All of the above under `MACVM_JIT=threshold=1` (FFI under compiled
   callers — P3's habit inherited).
7. `just gate-p05` (chains gate-p03) passes.

## Unit tests

| Test | Module | Assertion | Rationale |
|---|---|---|---|
| `db_missing_is_not_an_error` | `runtime::winkb` | every lookup → `WinkbError::DbMissing`; pragma fallback proceeds | the module's founding contract; also the no-DB CI story |
| `classifier_scalar_rows` | `runtime::winkb` | g/f scalars classified; `extern "system"`≡`extern "C"` assert on read | D2 rows 1–2 |
| `classifier_struct_rows` | `runtime::winkb` | ≤8 B → one reg; 9–16 B → two; >16 B ret → x8 pointer; params refused (v1) | D2 rows 3–6 |
| `classifier_refuses_variadic` | `runtime::winkb` | variadic import → refusal naming the reason | the booby-trap row stays closed until modelled |
| `pin_struct16_roundtrip` | FFI tests | local `extern "C"` returning 16-byte struct: both halves intact via trampoline | execution-pinned ABI (the x64 lesson, ARM64 form) |
| `pin_hfa_refused_or_correct` | FFI tests | HFA either refused (v1) or, if modelled, round-trips a `{f64;f64}` through v-regs | whichever D2 decision lands, pinned |
| `pin_out_param_pointer` | FFI tests | `QueryPerformanceCounter`/`GetSystemTimeAsFileTime` fill the buffer | pointer-arg shape on real imports |
| `filetime_conversion` | world (`.mst`) | known FILETIME → known date/time in Smalltalk | D4's epoch math, guest-side |

## Integration/golden tests

- The un-gated `ffi_alien` world file (settlement row 1) — Alien byte
  access + kernel32 targets.
- `win_io` twin if the D5 posix_io settlement lands it (open/write/read/
  close a temp file, guest-side).
- Existing `dispatch_ffi_primitive` tests: the clean-guest-fatal path for
  UNRESOLVED imports still works (a typo'd function name is a report,
  not a crash — rides P2's recovery).

## In-language tests

- `world/tests/` additions per settlement (the wall-clock assertions,
  alien exercises) — counted into the reconciliation of gate item 4.

## Stress/negative tests

- FFI call that faults (deliberate bad pointer through an out-param
  import) recovers via P2's foreign-AV path and reports — run ×100 for
  slot-reuse soundness.
- Resolver against a DELIBERATELY wrong DB row (test fixture DB): the
  refusal/mismatch path, not a garbage call (guards the "DB is data, not
  gospel" boundary).
- Wall-clock across a timezone-irrelevant boundary: two `Time now` calls
  straddling a second tick are ordered (cheap monotonic sanity; DST/UTC
  policy is the world file's documented choice, tested as documented).

## Non-goals

- COM object lifecycle (only static vtable slot math is modelled — no
  instantiation tests), callbacks, winsock, Accelerate parity (settled
  out in D5), variadic execution (refused in v1; its pinning test arrives
  with its implementation).
- DB build/refresh automation (documented command, manual artifact).
