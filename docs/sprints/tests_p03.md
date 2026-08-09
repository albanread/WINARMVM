# Sprint P3 — Test Plan

## Acceptance gate

All on this machine, natively, at the same commit as a Mac comparison run:

1. `MACVM_JIT=threshold=1 cargo test` green, with nonzero compile counts
   recorded (a run that compiled nothing proves nothing).
2. `MACVM_JIT=threshold=1 MACVM_GC_STRESS=1 cargo test` green — the S12
   flagship on Windows ARM64.
3. `MACVM_DEOPT_STRESS=1` green; then all three modes combined green (the
   S14 bar).
4. World suite green under 1–3; JIT-vs-interpreter differential over the
   golden + repro corpus: **zero differences** (unlike WINVM's x64 seven —
   this backend is complete; any diff is a port regression and blocks).
5. Mac cross-check: identical suite counts, identical golden transcripts,
   identical bench RESULTS; timings recorded in `docs/PERF.md`'s new
   Windows-ARM64 table.
6. S10/S11 perf tripwires still pass (fail < 2× interpreter on their
   micro-benches); no other perf gating.
7. Zero `#[ignore = "P1"]`/`"P2"` marks remain (grep in the gate script).
8. `just gate-p03` (chains gate-p02) encodes 1–4 + 7.

## Unit tests

| Test | Module | Assertion | Rationale |
|---|---|---|---|
| `struct16_return_in_x0_x1` | `codecache` (stubs tests) | emitted code calls a Rust `extern "C"` returning a 16-byte struct; both halves intact | D3 — turns the "hazard absent on ARM64" claim into a test (the exact `rt_poll` site that bit x64) |
| `rt_set_nonscalar_grep_pinned` | meta (build/test script) | `rt_*` extern set contains exactly the known non-scalar returns (currently: `rt_poll`) | WINVM's exhaustiveness check, re-run since the set may have grown |
| `frame_size_under_probe_limit` | `compiler`/`codecache` finalize | `debug_assert!(frame_bytes < 4096 - SLOP)` present + a test compiling the LARGEST corpus method asserts headroom recorded | D1 stack-probe audit, made permanent |
| `stub_frames_measured` | stubs tests | each hand-written stub's frame size asserted < limit (table in status entry) | same audit, stub side |
| `compile_count_nonzero_at_threshold1` | `it_world` harness | stats channel reports nmethods > 0 after the suite | the false-green tripwire |

## Integration/golden tests

- Existing tier-1, PIC, deopt, OSR, GC-under-compiled-frames suites — the
  whole S10–S15 inventory — now running natively. No new files; the gate
  IS their un-ignored, stress-crossed execution.
- Golden corpus differential harness (existing): run in both JIT modes,
  diff per method.
- Bench harness (`world/bench/`): results asserted, times recorded.

## In-language tests

None new. The in-language suite under combined stress is the deliverable.

## Stress/negative tests

- The three-mode combination ×3 consecutive runs (flaky-catcher: WINVM's
  card-boundary flake was found by parallel-run variance; give variance a
  chance to speak).
- `MACVM_GUARD_COUNT=1` run recorded (D2): guard acquires + icache bytes
  within ~2× of the Mac's same-commit numbers, else investigate before
  gating.
- One forced mid-loop scavenge with compiled frames on stack (existing S12
  test) explicitly re-verified in the log — it is the single most
  OS-layer-sensitive test in the suite (moving GC + fresh code + flush).

## Non-goals

- Performance improvement work (record-only, standing rule 3, tripwires
  excepted).
- GUI/FFI coverage (P4/P5).
- Multi-VM workers stress beyond the existing suite (the worker layer is
  OS-neutral channels + threads and is covered by the normal suite here;
  a dedicated Windows soak is post-P5 hardening if wanted).
