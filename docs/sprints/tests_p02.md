# Sprint P2 — Test Plan

## Acceptance gate

1. **Trap round-trip** (the M2-shape gate): an emitted `brk #0xDE00` site →
   VEH → x16 stash verified → trampoline → resume — against the REAL
   handler and a real published blob.
2. **Jump probes**: return-twice; ≥ 50-frame longjmp with **zero `Drop`s**;
   callee-saved GPR + d8–d15 sentinel integrity across the jump.
3. **Foreign-AV recovery**: a controlled wild deref on a thread with a
   recovery slot recovers via the VEH → longjmp path, faulting address read
   back; `STATUS_STACK_OVERFLOW` is NOT caught (assert the exclusion is
   present in code, not by overflowing in-process).
4. **Guest-fatal recovery**: embedded `VmHandle` catches a DNU as a
   message; the process serves a SECOND request after (WINVM's
   STILL-ALIVE/AND-AGAIN shape, headless).
5. P0's gated signal-layer + `embed::tests` groups run green on Windows via
   their twins; remaining `#[ignore = "P2"]` marks removed.
6. `cargo test` + clippy clean; `just gate-p02` (chains gate-p01) passes.

## Unit tests

| Test | Module | Assertion | Rationale |
|---|---|---|---|
| `arm64_context_size_pinned` | `codecache::deopt_trap` | `size_of::<ARM64_NT_CONTEXT>() == 0x390` (+ field-offset asserts for Pc/Sp/X16/Fp/Lr) | a mis-declared CONTEXT is register soup at fault time; fail at compile/test instead |
| `veh_redirect_smoke_a64` | same | brk site → capture trampoline records (pc, x16 == pc) → resume | D1 trap probe — the isolated mechanism proof |
| `foreign_brk_continues_search` | same | `brk #1`-shaped and `#0xF000`-shaped words at a NON-cache pc are not consumed (handler returns CONTINUE_SEARCH; test observes via a nested `__try`-equivalent harness or a second, later VEH that counts) | imm/range refusal — Rust aborts and debuggers must keep working |
| `decode_deopt_brk` (existing) | same | unchanged | reused verbatim — its passing is the "no new codec" claim |
| `setjmp_returns_twice` | same | first call 0; after longjmp(…, 7) returns 7 | D1 jump probe |
| `longjmp_runs_zero_drops_60_frames` | same | drop-counter type: 0 drops across a 60-frame jump | THE non-unwinding property — the reason this code exists |
| `callee_saved_survive_jump` | same | x19–x28/fp + d8–d15 sentinels intact after second return | buffer completeness incl. the FP halves |
| `av_recovery_reads_fault_addr` | same | controlled AV → recovered → recorded faulting address == the bad pointer | D4 end to end |
| `assert_trap_prints_dossier` | `runtime::probe` | forced `TRAP_ASSERT` in a subprocess: exit is the dossier path's code and stderr contains the pc line + a disasm window | PROBE alive on ARM64 with `disasm_a64` |

## Integration/golden tests

- Windows twins of the macOS trap tests in `deopt_trap`'s existing suite
  (same scenarios, VEH transport): trap-site decode against emitted blobs,
  trampoline redirect with the per-cache registry, capture-mode tests.
- `embed::tests` un-gated: the DNU-recovery flagship (gate item 4) plus the
  existing embedded-eval round-trips.
- Subprocess isolation: dossier/terminate tests and the AV test run in
  subprocesses (existing harness pattern — a recovered AV in-process is
  fine, a dossier exit is not).

## In-language tests

None new — guest-visible behavior (`error:`, DNU) is already covered by the
world suite; what changed is that on Windows it now RECOVERS. The world
suite's error-path tests passing on Windows is part of gate item 5's
un-gating.

## Stress/negative tests

- Repeat gate item 1's round-trip ×1000 (flush + trap interplay under
  churn).
- Two threads each with a recovery slot faulting concurrently, both
  recover (the CG0 concern, VEH flavor — no alt-stacks involved).
- Re-entrancy guard: a fault forced INSIDE the dossier path (test hook)
  terminates rather than recurses (subprocess test).
- Handler-under-debugger note recorded (manual): suite runs under WinDbg
  attach with foreign breakpoints flowing — no hangs. Manual because CI
  has no debugger; record the run once in the status entry.

## Non-goals

- No `threshold=` tier-up runs, no deopt-STRESS sweeps (P3 — the modes
  exist; this sprint proves mechanisms, P3 turns them on at scale).
- No FFI-fault matrix (P5 inherits D4 coverage).
- No attempt to recover stack overflow (excluded by design — D4).
