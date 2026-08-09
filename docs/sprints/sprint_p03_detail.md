# Sprint P3 — Tier-1 alive on Windows ARM64: the differential gate

Objective: the full adaptive VM — tier-1 compilation, PICs, deopt, OSR,
moving GC under compiled frames — runs on Windows ARM64 with **every stress
gate the project has** green, and the results diff clean against the Mac
build of the same commit. This sprint writes almost no new code: it lifts
P0's JIT-off guard, runs everything, audits the three ABI deltas that could
bite silently, and records the first native-ARM64-Windows performance
numbers. Implements MIGRATION.md §3.4 (audits) and the Phase P differential
thesis (§5).

## Prerequisites

- P2 green (traps + recovery). P1's loader. P0's interpreted baseline.
- The ENTIRE compiler/codecache stack — unchanged from MACVM and already
  compiling since P0 (`#[ignore]`d tests aside).

## Deliverables

- P0's JIT-off Windows guard removed; `MACVM_JIT` semantics identical to
  macOS.
- All remaining `#[ignore = "P1/P2"]`/`"P2"` marks removed (grep proves
  zero left).
- Three audits (D1–D3) recorded in this file's status entry.
- `docs/PERF.md` gains a Windows-ARM64 section (D4).
- `just gate-p03` — the combined-stress gate.

## Design

### D1. Stack-probe audit (MS ABI: >1-page frames must probe)

Windows requires touching guard pages in order; a frame larger than a page
(4 KiB) that skips past the guard page faults unrecoverably. Audit, not
rework: enumerate maximum compiled frame size — `frame_slots` bound from
regalloc + fixed prologue overhead + RootSpill area; then:

- Add `debug_assert!(frame_bytes < 4096 - SLOP)` at nmethod finalize time
  (SLOP ≈ 512 for callee pushes/alignment), so the invariant is CHECKED
  from now on, not assumed.
- Expected result: comfortably under (spill counts are small; the
  interpreter owns big frames and the OS-thread stack handles those
  normally). If any site exceeds it: emit an explicit probe loop in the
  prologue (one `ldr wzr, [sp, #-page]` per page walked) — but do not
  build that until a real frame needs it.
- The hand-written stubs (call stub, adapters, FFI trampolines) get the
  same one-time measurement; record each frame size in the audit table.

### D2. Page-size + flush audit under JIT load

P0/P1 audited statically; this re-checks under real load: run the suite
with `MACVM_GUARD_COUNT=1` once and record acquires/bytes (sanity: numbers
comparable to the Mac run's — an order-of-magnitude excess means a
flush-granularity bug); assert no 16-KiB assumption resurfaced via the
gated test's Windows twin (P0).

### D3. Struct-return pinning (the hazard that ISN'T here — prove it)

Both ABIs return ≤16-byte structs in x0:x1 (`rt_poll`'s `PollOutcome` —
the exact site that bit WINVM with x64's hidden pointer). Port WINVM's
pinning test as-is: call a real Rust `extern "C"` fn returning a 16-byte
struct from emitted code, assert both fields intact. Cheap, and it turns
an ABI assumption into a test. Exhaustiveness note carries: `rt_poll` is
the only non-scalar return in the `rt_*` set (WINVM checked; re-grep here
since the set may have grown — record the grep).

### D4. Runs, diffs, and PERF.md

1. `cargo test` at `MACVM_JIT=threshold=1` (everything eligible compiles).
2. The flagship: `threshold=1` + `MACVM_GC_STRESS=1` simultaneously (the
   S12 gate, now on Windows).
3. `MACVM_DEOPT_STRESS=1` (S13's gate).
4. All three COMBINED (the S14 bar).
5. World suite under 1–4; golden + repro corpus differential
   (JIT-vs-interpreter per method — the existing harness) — zero
   differences expected: the seven closure/NLR/OSR differences WINVM lives
   with were x64-backend gaps; **this backend has no such gaps.** Any
   difference here is a REGRESSION against the port thesis and blocks the
   gate.
6. Cross-build differential: same commit on the Mac — suite counts, golden
   transcripts, bench RESULTS identical; bench TIMES recorded side by side.
7. `docs/PERF.md`: new "Windows ARM64" table — DeltaBlue, Richards,
   Mandelbrot (the SIMD kernels are live — `target_arch`-gated, they
   compiled since P0), interpreted vs `threshold=1` vs default-threshold,
   against the Mac numbers for the same commit. Same-ISA hardware-class
   comparison is a first for this family; expect same order of magnitude,
   diagnose >2× gaps (candidates: flush frequency D2, allocator/OS noise,
   scheduler) — but per standing rule 3, RECORD, don't gate on perf...
   with one exception: the S10/S11-era tripwires (fail < 2× interpreter)
   still apply and now run on this target.

## Implementation order

1. D3 pinning test (fastest, catches ABI surprise before bulk runs).
2. Lift the JIT-off guard; remove `#[ignore]`s; `threshold=1` plain run;
   fix what falls out (expected: nothing to little — the honest place
   surprises would live is loader/trap glue, which P1/P2 gated).
3. D1 audit + assert; D2 counters run.
4. D4 runs 1–5; then 6–7 with the Mac side.
5. `just gate-p03: gate-p02` + the combined-stress recipe.

## Pitfalls

- **A green suite that never compiled anything is a false green** — assert
  compile COUNTS in the `threshold=1` runs (the `-v`/stats channel already
  reports nmethod counts; the gate records them nonzero and comparable to
  the Mac's).
- **Do not chase perf during the gate.** Record, file, move on (standing
  rule 3). The port's claim is correctness-parity first.
- **Emulation tripwire again**: benches are only meaningful native — the
  P0 arch assert is required passing in the same process that produces
  PERF.md numbers.
- If a stress combination fails HERE but passes on the Mac at the same
  commit, suspect the OS layer in this order: icache flush granularity
  (P1), VEH interplay (P2), reservation commit semantics (P0) — the
  compiler is the least likely suspect for the first time in this
  lineage's porting history.

## Interfaces for later sprints

- P4's metrics pane reads the same stats the D4 runs exercised.
- P5's FFI works under JIT-on; its tests inherit the combined-stress
  habit.

## Out of scope

- New optimization work, near-image placement, W^X hygiene (recorded
  options).
- OSR/closure semantics work of any kind — nothing arch- or OS-specific
  remains there by construction (same backend, same metadata).
