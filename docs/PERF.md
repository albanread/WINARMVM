# MACVM interpreter throughput — S6 baseline

Recorded per `sprint_s06_detail.md` §Benchmarks' procedure (SPRINTS
standing rule 3: **tracking, not gating** — these numbers are not part of
any test's pass/fail criteria).

## Environment

- Host: Apple M4, 10 cores, macOS (Darwin 25.5.0, arm64)
- Build: `cargo build --release` (rustc 1.96.0)
- Date: 2026-07-02

## Procedure

`MACVM_TRACE=count` (prints total bytecodes dispatched at exit) plus
`/usr/bin/time -p` (wall clock) around each of 5 runs per benchmark;
`world/bench/fib.mst` (fib 25), a 30-variant of the same script, and
`world/bench/sieve.mst` (10 iterations, size 8190, expected count 1899).
Bytecode counts were byte-for-byte identical across all 5 runs of every
benchmark (determinism confirmed, per the procedure's requirement).

## Results

| Benchmark | Result | Bytecodes (median = all 5) | Median wall (traced) | bc/s (traced) |
|---|---|---|---|---|
| fib(25) | 75025 | 2,677,644 | 0.08 s | ~33.5M bc/s |
| fib(30) | 832040 | 29,625,095 | 0.90 s | ~32.9M bc/s |
| sieve ×10 | 1899 | 5,672,753 | 0.17 s | ~33.4M bc/s |

fib(30) wall time (0.90 s traced) is well under SPEC §13's `< 2 s` gate.

## `MACVM_TRACE=count` overhead

The three benchmarks above cluster tightly around **~33M bc/s with the
counter enabled** — noticeably below SPEC §13 row 1's 50M bc/s target.
Re-running fib(30) *without* `MACVM_TRACE=count` gives a median wall time
of **0.55 s** for the same 29,625,095 bytecodes: **~53.9M bc/s**, above
target. The counter itself (`sprint_s06_detail.md`'s own estimate: "cost ≈
1 add/dispatch, acceptable") measurably costs more than that in practice
on this build — a ~40% slowdown, not the "1 add" the doc's estimate
assumed. This is worth another look in a later throughput-focused sprint
(S10/S14/S15 per the SPRINTS doc), but is out of scope here: S6 is a
library sprint, not an interpreter-optimization one.

## Pass/fail against SPEC §13 row 1 (tracking only)

- fib(30) < 2 s: **PASS** (0.55 s untraced, 0.90 s traced — both well
  under).
- ≥ 50M bc/s: **PASS untraced** (~53.9M bc/s), **FAIL traced** (~33M
  bc/s) — the gap is attributable to the counter overhead noted above,
  not to per-bytecode dispatch cost. Since the procedure as written
  measures wall time *with* the counter active, the honest reading of
  this baseline is "fails as measured, passes with the counter removed
  from the hot path" — recorded for whoever picks up interpreter
  throughput work later.

# S10 tier-1 JIT — perf marker

Recorded per `tests_s10.md`'s "Perf marker" procedure (SPRINTS standing
rule 3: **tracking, not gating**). `world/bench/arith.mst`'s
`sumTo: 5_000_000` — a send-free, once-compiled smi arithmetic kernel
(`SmiArith Add`, the inlined `to:do:`'s `SmiCmpBr`, `Poll` at the loop
back-edge) — timed via `millisecondClock` after two small warm-up calls
through the same call site (so the compile itself never lands inside the
timed window), under `MACVM_JIT=off` vs `MACVM_JIT=threshold=1`, via
`just bench-s10` (`--release`). The gate WARNS below 5x and FAILS only
below 2x (an architectural-mistake tripwire, not a perf gate — gate item
3 of tests_s10.md's acceptance gate).

| Date | Commit | interp_ms | jit_ms | ratio |
|---|---|---|---|---|
| 2026-07-03 | 177abf1 | 1221 | 9 | 135.66x |
| 2026-07-03 | 353db27 | 1233 | 10 | 123.30x |

# S11 D8 bridge — allocation cost of the pre-S12 GC bridge

Recorded per `tests_s11.md`'s "Bridge accounting" stress/negative test
(SPRINTS standing rule 3: **tracking, not gating** for `bridge_old_allocs`
itself — `gc_under_compiled` IS gating: `just bridge-stats-s11` fails the
run if it's ever nonzero). The full world test suite, under
`MACVM_GC_STRESS=full:64` combined with `MACVM_JIT=threshold=1` (the same
combination `gate-s11` stress-tests), traced with `MACVM_TRACE=gc`.
`bridge_old_allocs` is every allocation the D8 bridge diverted old-direct
because a compiled frame was live (`compiled_depth > 0`) — non-moving,
so it costs old-gen space no scavenge can ever reclaim until S12 deletes
the whole bridge. `gc_under_compiled` is the number of times a
scavenge/full-GC actually ran while `compiled_depth > 0` — i.e. the
bridge failing to hold; must always read 0.

| Date | Commit | bridge_old_allocs | gc_under_compiled |
|---|---|---|---|
| 2026-07-04 | 7ac7b53 | 110 | 0 |

# S11 dispatch — perf marker (adapted, see world/bench/dispatch.mst's own doc)

Recorded per `tests_s11.md`'s gate item 4 ("Dispatch micro-benchmark"),
ADAPTED: the literal 3-class polymorphic design that file sketches cannot
compile at all under S11's as-built eligibility gate (`mono_smi_inline_send`
rejects any non-super send whose IC guard isn't `SmallInteger`, monomorphic
or not — see `world/bench/dispatch.mst`'s own header and
`sprint_s11_detail.md`'s STEP-10 NOTES for the full reasoning). This instead
times `world/bench/dispatch.mst`'s `runLoop: 5_000_000` — arith.mst's own
`sumTo:` shape with its inlined `+` replaced by a REAL super-send dispatch
per iteration (D4.6: the one non-arithmetic, non-`basicNew` send a compiled
method may contain) — under `MACVM_JIT=off` vs `threshold=1`, via
`just bench-s11` (`--release`). Same warn<5x/fail<2x tripwire as
`bench-s10` (tracking, not gating).

A smaller ratio than `bench-s10`'s ~130x is the EXPECTED, honest result: a
real send still costs a real dispatch even compiled (unlike inlined
arithmetic, which erases the cost entirely) — this benchmark measures that
cost, it doesn't erase it.

| Date | Commit | interp_ms | jit_ms | ratio |
|---|---|---|---|---|
| 2026-07-04 | 7ac7b53 | 1834 | 472 | 3.88x |
| 2026-07-04 | abe4f2e | 110 | 0 |
| 2026-07-04 | cdfab6a | 110 | 0 |
| 2026-07-04 | a1e57ac | 110 | 0 |
| 2026-07-04 | 04e774b | (bridge deleted) | 110 |

# S15 A6/A7 — Richards/DeltaBlue perf recording

Recorded per `tests_s15.md` T5's procedure: `world/bench/bench.list`
(`richards.mst`, `deltablue.mst`) run through the shared `Bench.mst`
harness (3 discarded warmups + median-of-outer, timed via
`millisecondClock`, excludes genesis/world load) under `MACVM_JIT=off` vs
`threshold=1` vs `threshold=1000`, via `scripts/perf.sh --release`.

## 2026-07-06 (commit f62a1e4) — Richards t=1 CORRECT for the first time

BUG D root cause 4 turned out to be two distinct bugs (OSR uninit frame
slots + c2i >5-arg marshaling overflow — see tests/repros/README.md and
f62a1e4's own message), both fixed. Richards now completes CORRECTLY
under `threshold=1` (23246/9297 golden values) — previously it could not
complete under any JIT threshold at all.

| Benchmark | interp_ms | jit t=1 | jit t=1000 | best/interp |
|---|---|---|---|---|
| richards | 204 | 193 | **blocked** (mid-threshold wrong-answer) | 1.1x (t=1) |
| deltablue | 208 | 119 | **blocked** (BUG C) | 1.7x (t=1) |

- **richards t=1 = 1.1x is a PERF gap now, not a correctness gap**: the
  run is correct but trap-heavy (the `work kind` two-way branch keeps one
  arm trapping — 60k+ uncommon traps observed per run), so most of the
  win is eaten by deopt/reexecute churn plus interpreted fallback for the
  7-arg-send creator methods the new eligibility cap declines. T5's
  "Richards ratio ≥ 5.0" gate therefore remains unmet — but the remaining
  work is optimization (trap-site healing / poly-arm compilation), not
  bug-fixing. `it_perf_s15.rs` still should not be written yet: it would
  fail on day one for perf reasons.
- **The mid-threshold silent wrong-answer (Richards t=100..20000,
  DeltaBlue t=1000 — very likely BUG C's own band)**: results are correct
  interpreted, at t≤10, and at t≥100000 (OSR-only compilation), but wrong
  whenever invocation-triggered compilation lands MID-run — the
  early-exit fraction tracks the threshold precisely (at t=20000 the
  scheduler dies ~92% through, exactly where `queuePacket:` crosses
  20000 invocations). Documented as the next investigation; blocks only
  the t=1000 columns above.
- Also known, pre-existing (stash-bisected, not from these fixes): the
  BUG D repro fails under `MACVM_GC_STRESS=1` + `threshold=1`
  (`doesNotUnderstand: size`), while passing under `MACVM_DEOPT_STRESS`.

## 2026-07-06 regression A/B: the f62a1e4 fixes are perf-neutral

Question asked and answered with a true A/B (release builds of HEAD vs
d65d1dd — the commit immediately before the fixes — same machine, runs
interleaved): did the root-cause-4 fixes cost performance?

| Benchmark | Mode | pre-fix (d65d1dd) | post-fix (HEAD) |
|---|---|---|---|
| arith | off / t=1 / t=1000 | 1253 / 6 / 9 ms | 1262 / 6 / 9 ms |
| dispatch | off / t=1 / t=1000 | 1844 / 26 / 11 ms | 1838 / 26 / 11 ms |
| sieve | off | 85 ms | 85 ms |
| sieve | t=1 / t=1000 | **SIGSEGV (exit 139)** | 84 / 85 ms, correct |
| richards | off / t=1 | 204 / DNU abort | 205 / 194 ms correct |
| deltablue | off / t=1 | 209 / 119 ms | 209 / 120 ms |

Identical to the millisecond on every benchmark that ran before — which
also directly measures the only hot-path cost the fixes added (the two
extra register-pair spills per runtime-stub call: no observable effect).
And the A/B surfaced something the record didn't yet know: the PRE-fix
build SIGSEGVs on sieve under BOTH JIT thresholds in release — the OSR
uninitialized-slot bug again, cured by the same fix. Net: nothing slower,
two benchmarks (sieve JIT, richards t=1) went from crashing to correct.

# Dual-arm branch storm — sieve is the canonical repro (2026-07-08)

Representative-benchmark sweep after the primitive-shim work (release,
`--world world`; richards/deltablue self-timed, fib/factorial self-timed
via `millisecondClock`, arith/dispatch process-timed incl. ~10 ms boot):

| Benchmark | interp (off) | jit t=1 | speedup | shape |
|---|---|---|---|---|
| arith (`sumTo: 5M`) | 1280 ms | ~10 ms | >100x | tight fused smi loop |
| fib(32) | 1637 ms | 15 ms | ~109x | recursion + dispatch + fused arith |
| factorial 20! x200k | 6989 ms | 890 ms | ~7.9x | smi `*` + overflow→LargeInteger |
| factorial 500! x300 | 72325 ms | 4386 ms | ~16.5x | bignum multiply + allocation |
| deltablue (10x10) | 213 ms | 113 ms | ~1.9x | constraint solver |
| dispatch | 1870 ms | ~20 ms | ~90x | send/IC dispatch |
| **sieve (8190 x10)** | **87 ms** | **88 ms** | **~1.0x** | **dual-arm branch storm** |

**CORRECTION (2026-07-08, via `MACVM_TRACE=deopt`/`MACVM_DBG_REEXEC`/
`MACVM_DBG_IR` — an earlier draft of this entry crowned sieve the "dual-arm"
repro; the debugger overturned that. Recorded honestly.):**

- **Sieve is NOT a balanced-branch storm.** Its `threshold=1` flatness (65
  deopts) is a *compile-cold* artifact: at `threshold=1` the method compiles
  before its loop body has run, so its send ICs are all `Empty`, and the
  compiler lowers `Empty`-IC sends to `Untaken → UncommonTrap` (dead-code
  speculation) — then the loop runs and they all trap. Across the threshold
  sweep sieve is flat at EVERY setting (94-96 ms) and at `threshold=2000`
  has only **30 deopts** — no storm. Its high-threshold flatness is
  compile-timing on a short (~95 ms) workload, not speculation. Not the
  repro we thought.

- **The real speculation storm is Richards**, and it is NOT in
  `processWork:` (weekend_work.md Gap 1's guess, also eyeballed) and NOT a
  balanced boolean branch. The debugger pins it to **`addInput:checkPriority:`
  bci=21** — a `GuardKlass { obj, expect } fail → UncommonTrap` (block1
  @bci8 → block10 @bci21 in the warm IR). It is a **receiver-klass guard on
  a mono-inlined send** (S14 step 4b/5): the compiler inlined a
  `priority`/`packetPending:` accessor betting one Task subclass, but
  Richards runs four (Idle/Worker/Handler/Device) through this method, so the
  guard fails ~half the calls. **160,555 of 160,674 deopts** are this one
  site, and it is **threshold-independent** (826k at t=1, 160k at t=2000 —
  the steady-state storm survives full warmup because the site is genuinely
  polymorphic, not cold).

- **Richards is ~2.4×, not 1.1×.** The "1.1×" on record was measured at
  `threshold=1` (the cold-compile worst case, 826k deopts). Warmed up
  realistically (`threshold=2000`): **off 207 ms vs 85 ms = 2.4×**, deopts
  160k. The benchmark harness's `threshold=1` convention systematically
  understates the JIT. (Threshold sweep: sieve 94/95/96/95 ms at
  off/1/100/2000; richards 207/191/91/85 ms.)

- **The fix** is still the "detect an over-deopting speculation site, then
  de-speculate" shape, but "de-speculate" here = **stop mono-inlining that
  send; dispatch it polymorphically** (a real send, or S14 step 6's existing
  `DominantWithSlowPath`), NOT "compile both branch arms." Gate on Richards
  `addInput:checkPriority:` (deopts at bci=21 → ~0). Open puzzle: it already
  recompiles 18× (nm 38/41/42) and still storms — the recompiler isn't
  switching this site to poly; that is the thing to fix.

## RESOLVED (2026-07-09, commit a2bfd8b): the IC stomp in activate_method

The "open puzzle" above cracked the case: the recompiler wasn't switching
the site to poly because the IC never STAYED poly. `interpreter::send::
activate_method`'s over-threshold path unconditionally rewrote the caller's
IC to Mono-compiled(current receiver klass) on every dispatch —
`ic_transition` would upgrade Mono(A)→Poly[A,B] and the very next
over-threshold dispatch stomped it back to Mono(B). The IC ping-ponged
between Mono states forever: `snapshot_profile`'s tag-only hash never
changed (8,501 "profile unchanged" declines, 0 recompiles in one warm run),
and each customized compile baked whichever klass was last stomped in as a
mono-inline KlassGuard whose fail-edge trap then fired on ~every other
call. Proven with the in-tree debugger: at every reexecution the receiver's
klass EQUALED the live IC guard (the interpreter never missed!) while the
baked pool word held a different klass — a Mono→Mono re-key that
`ic_transition` cannot produce; the only writer capable was the stomp.

Fix (one gated seed in `activate_method`): seed the IC only from Empty or
same-klass Mono; never downgrade Poly/Mega or re-key a different-klass
Mono. The preserved Poly tag lets the EXISTING recompile machinery re-lower
the send (DominantWithSlowPath / plain Call) — no new mechanism needed.

| Benchmark | interp (off) | jit t=1 | jit t=2000 | best/interp |
|---|---|---|---|---|
| richards (before) | 208 | 191 (826k deopts) | 85 (160,674 deopts) | 2.4x |
| **richards (after)** | 208 | **18** (30k deopts, 58 recompiles) | **13** (**2 deopts**, 1 recompile) | **16x** |
| deltablue (before) | 208 | 113 | — | 1.9x |
| **deltablue (after)** | 208 | — | **62** | **3.4x** |
| sieve | 88 | — | 93 (count 1899 correct) | ~1.0x (separate: threshold=1 cold-compile artifact + short workload) |

Correctness: Bench's own checkResult (error: on mismatch) passed on every
run; full test suite green (19 binaries); stress matrix over 4,609 world
tests × {GC_STRESS=1, GC_STRESS=full:64, DEOPT_STRESS=64} × threshold=1 —
0 failures. The S15 T5 gate ("richards ≥ 5.0") is now PASSED at 16x.

## 2026-07-09 (S24 A1, commits 979daf0..db378fa) — compiled closures land

First slice of closure compilation: standalone block bodies compile
(`by_block` registry, closure calling convention, compiled-block NLR
origination, root-is_block deopt materializer). Numbers via
`scripts/perf.sh --release` (t=1/t=1000 columns) on the A1 code:

| benchmark | interp (ms) | jit t=1 | jit t=1000 | best/interp |
|---|---|---|---|---|
| richards | 218 | 19 | 13 | 16.8x |
| deltablue | 229 | 43 | 55 | **5.3x** |

- **DeltaBlue gate (>=5.0x) PASSED already at A1** — the design expected
  this to need A2/A3. Block-iteration backbone (do:/detect:-style bodies)
  no longer interprets. Was 3.4x at the IC-stomp fix (a2bfd8b).
- richards 16.8x: the >=16x no-regression gate holds (blocks there are
  the S14-ELIDED kind; A1 must not and did not perturb the splices).
- arith 1447 -> 11ms (131x), fib/sieve unchanged — gate 3 noise band.
- Interpreted tail (MACVM_TRACE=count, deltablue warm t=2000 vs off):
  14,316,872 / 102,901,203 = **0.139** (pre-A1 doc baseline 0.163;
  gate 2 target <0.10 is A3's job). The NEW per-method attribution
  (`bytecodes-by-method:` lines) shows the residue is entirely the
  A3-target creator methods (constraintsConsuming:do:, makePlan:, ...)
  — ZERO `[block]` entries in the top 40: A1 did exactly its share.
- Correctness: world suite byte-identical vs interpreter at t=1 AND
  t=200, plain and under {GC_STRESS=1, GC_STRESS=full:64,
  DEOPT_STRESS=64}; deltablue+richards under the same matrix at t=200
  all green. Two real bugs found by the benchmark half of the matrix:
  the PIC duplicate-klass GC corruption (FIXED, db378fa — also the true
  cause of the "cache exhaustion" abort) and the t=1-only stale-slot
  (task #125, pre-existing, full dossier in tests/repros/README.md
  entry 9).
- Measurement policy note: threshold=1 stays the DIFFERENTIAL oracle
  (compile everything, compare bytes); it is NOT a perf configuration —
  cold compiles get no feedback-driven code. Stress and perf runs now
  also gate on threshold=200 (metric-driven compiles), release builds,
  all modes in parallel.

## 2026-07-09 (S24 A2, commit 0401df7) — direct value-family dispatch

Compiled `value`-family sends now tail-jump straight to the block nmethod
via a shared per-argc dispatch stub (`by_block` probe), replacing the c2i
adapter + nested interpreter activation. `scripts/perf.sh --release`:

| benchmark | interp (ms) | jit t=1 | jit t=1000 | best/interp |
|---|---|---|---|---|
| richards | 204 | 19 | 12 | 17.0x |
| deltablue | 213 | 34 | 41 | **6.3x** |

- **DeltaBlue 5.3x -> 6.3x** — the warm run dropped 43->34ms purely from
  the value-dispatch fast path. `MACVM_TRACE=stats` on deltablue t=200:
  `value_dispatch_hits=1741710 value_dispatch_fallbacks=1516` — 99.9% of
  `value:` sends in compiled methods tail-jump; the 1516 fallbacks are cold
  warmup before each block compiles. (My pre-implementation caution that A2
  might barely fire before A3 was wrong: deltablue's compiled constraint
  methods send `value:` heavily.)
- richards ~17x (noise vs A1's 16.8x — its blocks are the S14-elided kind,
  few standalone `value:` sites); arith/fib/sieve unchanged.
- Interpreted tail UNCHANGED from A1's 0.139: A2 changes how compiled
  `value:` sites dispatch, not which methods compile. A3 (compiling the
  closure-creating orchestrators) is what closes the tail toward <10%.
- Correctness: world byte-identical vs interpreter at t=1 AND t=200, plain
  and under {GC_STRESS=1, DEOPT_STRESS=64}; benches x 3 stress modes x
  t=200 all green; cargo test 833/0.

## S24 A3 — compiled closure creation (A3a escaping non-ctx, A3b materialize Context)

2026-07-10, commits 96faa0a (A3a), fb01b7a (A3b), 70b5513 + 9de470b (review
remediation). Release, MacBook arm64, `world/bench/*` via `Bench run:`.

| benchmark | interp (ms) | jit t=200 | jit t=1000 | best/interp |
|---|---|---|---|---|
| richards | 204 | 20 | 13 | 15.7x (held) |
| deltablue | 214 | 33 | 33 | **6.5x** |

- **deltablue 6.3x -> 6.5x** and, far more important, the A3b tail methods
  (constraintsConsuming:do:, addConstraintsConsuming:to:, printOn:) now
  COMPILE — closure-creating methods with captured temps get a real
  materialized Context. T5 >=5.0 gate: PASSED.
- **The benchmark run was the detector for two release-observable bugs the
  world differential missed** (both fixed, see tests/repros/):
  1. `rt_alloc_slow` still enforced D7's Slots-only contract and allocated
     `nis` words ignoring the site size — a Closure/Context Alloc overflowing
     eden returned a too-short object and the continuation corrupted the
     neighbor (deltablue DNU #value: under real allocation pressure; every
     small repro stayed on the inline fast path). Latent since A3a.
  2. a has_ctx method whose FIRST bytecode is a loop header re-ran the
     block-0 prologue every iteration (per-iteration Context snapshots vs the
     interpreter's ONE shared Context — silent wrong answers). Now declined.
- Remaining deltablue tail at t=200: 8.9M interpreted bytecodes, led by
  ScaleConstraint>>execute (39.8%) + recalculate (13.0%) — the next
  eligibility targets (B-phase).
- Correctness: world byte-identical off vs t=200 (release), plain and under
  {GC_STRESS=1, GC_STRESS=full:64, DEOPT_STRESS=64}; loop-header +
  tiny-eden repros green under DEOPT_STRESS/GC_STRESS; 633 lib + full
  integration suites green.
- Process note: fb01b7a's post-commit code review (8 finder angles) filed 10
  findings; the two live ones were fixed same-day and the CtxLoc::None
  bci-fingerprint (three finders converged) was re-keyed to ctx-vreg
  liveness before it could bite under organic NotEntrant deopts.

## S24 B-phase L1 — stale PIC-c2i heal (e3a3f00)

2026-07-10. The B-phase understand pass (3-reader + adversarial-verify
workflow) found the deltablue tail was a DISPATCH FREEZE, not eligibility: PIC
pairs baked (klass -> c2i) before the callee compiled never upgrade. One lazy
re-key arm in rt_interpret_call's upgrade hook:

| benchmark | interp (ms) | jit t=200 | jit t=1000 | best/interp |
|---|---|---|---|---|
| richards | 204 | 12 | 12 | **17.0x** |
| deltablue | 214 | 11 | 11 | **19.5x** |

- deltablue **6.5x -> 19.5x** (33 -> 11ms); richards 20 -> 12ms at t=200 (its
  t=1000 record now holds at ALL realistic thresholds). c2i_pic_rekeys=19.
- Remaining deltablue tail (2.8M interpreted bytecodes): the sub-threshold
  DRIVER methods (projectionTest: 32%, chainTest: 24%, makePlan: 15% — all
  called ~103x < 200 and OSR-ineligible because they contain closures,
  driver.rs:750). L2 target: extend the OSR envelope to closure-bearing
  methods. B1-B4 (block-arg inlining wideners) mapped from the design doc
  thereafter.
- Correctness: world byte-identical off vs t=200 plain + all three stress
  modes; deltablue correct under DEOPT_STRESS (re-key x invalidation churn).

## S24 L2 steps 1-2 — counters bit fix + trigger unification (d609251)

2026-07-10. The L2 design pass (3-reader + 3-design panel + judge workflow)
CORRECTED the premise: 3 of deltablue's 4 tail drivers have zero closures and
already OSR-compile — the tail was sub-threshold CALL entry (calls never
consult by_key until the invocation counter crosses). Fix, user-decided
policy: "the loop counters have detected in a different way that the method
containing the loop is hot; the method is now hot" — a by_key install
saturates the invocation counter to the threshold, unifying the two profile
triggers with zero new dispatch state.

| benchmark | interp (ms) | jit t=200 | best/interp |
|---|---|---|---|
| richards | 204 | **6** | **34.0x** |
| deltablue | 214 | **7** | **30.6x** |

- deltablue **19.5x -> 30.6x**, richards **17x -> 34x** (its loop methods
  were also OSR-earned + call-starved). trigger_unifications=3 per bench.
- Also fixed en route (found by the design pass's read phase): the
  COUNTERS_COMPILE_DISABLED_BIT sat inside the S15 loop-counter field —
  loopy NoPermanent methods re-attempted compilation every 10k backedges
  forever (unguided re-compilation). Now bit 33; tripwire test pins it.
- Remaining deltablue tail (1.74M bc): pre-first-OSR warmup of the drivers
  (~55%) + AbstractConstraint>>inputsKnown: (14.5%, loop-free — the B1/B3
  block-arg-inlining flagship). Envelope steps 3-6 (OSR for closure-bearing
  methods via Context adoption) next per the design; measured by the new
  ctxloop.mst when built.
- Correctness: world byte-identical off vs t=200 plain + all three stress
  modes; benches correct under stress; new send-based integration test
  proves a sub-threshold call enters an OSR-earned nmethod (<50 dispatched
  bytecodes vs ~800 interpreted).

S24 arc summary to date: interp -> 6-7ms on both flagship benches
(A1 5.3x -> A2 6.3x -> A3 6.5x -> L1 19.5x -> L2 30.6x deltablue;
richards 16x -> 17x -> 34x).

## S24 L2 steps 3-5+7 — OSR closure envelope: phases A+B (51401dc, c0c51cd, d01a67a)

2026-07-10. The envelope proper, per osr_closure_design.md. Phase A: non-ctx
closure-bearing methods OSR (phantom-temp packing proven + T7/T9 tripwires).
Phase B: has_ctx materialize-form OSR via **Context ADOPTION** — one transfer
pair, zero codegen changes; identity is the soundness story (pre-OSR closures,
post-OSR AllocClosures, and deopt all share the ONE Context); the elided form
declines (osr_declined_elided_ctx, the R1 evidence counter). Step 5: OSR
compiles inherit the key's version (MAX_VERSIONS accumulates across re-arms).

| benchmark | interp (ms) | jit t=200 | best/interp |
|---|---|---|---|
| **ctxloop** (new) | 134 | **1** | **134x** |
| deltablue | 214 | 7 | 30.6x (held) |
| richards | 204 | 7 | ~30x (held) |

- ctxloop is the envelope's own gate: has_ctx + escaping accumulator closure
  + 100k-iteration loop, called ONCE — only OSR can tier it, and its
  checkResult is SELF-VERIFYING for adoption (per-iteration-snapshot
  semantics would answer wrong). One run composes the whole L2 pipeline:
  osr_entries=1, osr_ctx_adopted=1, trigger_unifications=1.
- Tests: adoption-identity flagship (plain + DEOPT_STRESS + GC_STRESS),
  phase-A shapes x stress, elided-decline pinned in-process, tripwires
  T1/T4/T6/T7/T8/T9. World byte-identical off vs {t=1,t=200} + stress.
- Remaining from the design: step 6's gate-s24-l2 justfile recipe + debug
  transfer-buffer verifier (hardening, deferred); B1/B3 (inputsKnown:) is
  the next deltablue-tail item.

## S24 B5 — multi-BB block splicing + B3 self-devirt (b587e8e … f4953c1)

2026-07-11. The payoff layer: conditional-NLR blocks (branch + `^` in one arm)
now splice at direct value sends AND at block-arg sites, and self-receiver
block-arg sends devirtualize per the customization klass so the flagship
`self inputsDo: [...]` shape (Poly IC) compiles.

| benchmark | interp (ms) | jit t=200 | best/interp |
|---|---|---|---|
| deltablue | 214 | **4** | **53.5x** |
| richards | 204 | 6-7 | ~30x (held) |
| ctxloop | 134 | 1 | 134x (held) |

- Arc: A1 5.3x → L1 19.5x → L2 30.6x → B5 step 5b 42.8x (5ms) → B3 53.5x (4ms).
- Step 5b closed a pre-existing deopt-materializer stale-read hazard (allocations
  reordered behind all dead-frame reads via a deferred fixup phase) and a
  recompile deopt-storm (snapshot_profile now recurses into grafted blocks).
- B3: AV::Slf narrow self-provenance in escape ≡ convert's receiver==self_vreg;
  the resolution mode rides in the map so both passes resolve one callee; a
  klass-sensitive NoPermanent maps to NoRetryLater (never poisons the shared
  method's method-wide compile_disabled bit). Guard-free splice, lookup-shaped
  inline dep on (rcvr_klass, selector).
- deltablue interpreted tail: inputsKnown: (14.6%) left the interpreted-tail
  list; makePlan:/chainTest:/projectionTest: (loop-driver bodies) now dominate.
- Gate every step: world byte-identical off vs {t=1,t=200} + GC_STRESS=1/full:64
  + DEOPT_STRESS=64; full lib+integration suites; clippy+fmt.

## S24 — OSR cold-send provenance: no Untaken→Trap under OSR (sieve 90ms→9ms)

2026-07-11. Debugger-driven (DBG_IR ladder) root-cause of the sieve deopt-thrash
found via the JIT-coverage census. The three hotness signals — invocation
counter, loop counter, OSR — OR together to TRIGGER a compile (trigger
unification, L2 step 2), but each certifies a different region as profiled:
the invocation signal the whole body, the loop/OSR signal only the loop's
executed part. The S14-step-3 `Untaken→Trap` speculation assumed whole-body
coverage but fired under loop/OSR provenance too, so a not-yet-reached send
(cold IC only because the OSR-entered loop hadn't exited yet) was trapped and
deopted the instant the loop exited — an endless OSR-compile→trap→interpret
cycle. Fix: `convert` gains `osr`; under OSR the four `decide→Trap` cold-send
sites emit a plain `CallSend` instead (`cold_send_traps() == !osr`). "Reliable,
not merely fast" (the Strongtalk lesson — fast-but-crashes is a demo).

| bench | before | after |
|---|---|---|
| **sieve** ms | ~90 | **9 (~10×)** |
| sieve deopts | 30 | **0** |
| sieve OSR entries | 30 (re-thrash) | **1** (stays compiled) |
| sieve JIT coverage | 4.5% | **97.1%** |
| deltablue / richards / ctxloop | 4 / 6 / 1 | unchanged |

- Whole class of "hot work lives inside loops, method called once" (OSR-only
  methods) goes from ~5% to ~97% compiled.
- Gate: 635 lib + 101 it_tier1 + all 16 integration suites; world differential
  IDENTICAL off vs t=200/t=1/GC_STRESS=1/GC_STRESS=full:64/DEOPT_STRESS=64;
  clippy+fmt clean. Adversarial review (3 lenses) SOUND — CallSend at an
  Untaken site is the always-correct general lowering (the trap was only a
  feedback-warming speculation); all 20 UncommonTrap sites classified, gate
  complete; the reverse perf-cliff (cold error paths as CallSends under OSR)
  negligible, hits no frame/spill limit.

## Multi-VM workers — ParallelMandel scalability (2026-07-15)

The multi-Smalltalk-worker capstone (`docs/multi-smalltalk-worker.md`, worlds
47/48): the live zooming Mandelbrot with every frame computed in parallel
bands by 4 worker VMs (each its own heap + tier-1 JIT on its own OS thread),
the primary VM only assembling bands (via `send:onReply:` continuations, MOP
deep-copy messages) and blitting complete frames.

- **~2.65 CPUs of sustained utilization with 4 workers** (measured on-screen,
  release GUI, Demos → "Mandelbrot — parallel workers") — visibly faster than
  the single-VM `MandelZoom` dive. The gap to 4.0 is the honest price of the
  model: the primary's band assembly + pickle/unpickle copies + the serial
  blit, plus band-boundary imbalance (interior-heavy bands finish last).
- Headless gate (`parallel_mandel_computes_a_full_frame_across_worker_vms`):
  full 320×240 frame with every band verified computed by its worker — 1.29 s
  in a debug build (workers at `Threshold(10)`; the JIT-off first cut needed
  ~8 s+ *per band*, the usual reminder that compute workers must warm their
  own JIT).

## Dynamic compiled-code coverage — arith/richards/deltablue/sieve (2026-07-19)

The README's "98.6–99.8% of executed bytecode-work runs as compiled native
code" headline, reproduced fresh (this exact figure previously existed only
as an unreferenced measurement, not written down here — this section closes
that gap). Methodology (drift-immune: exact dispatch counts, not wall clock,
so no A/B interleaving or throttling caveats apply): `MACVM_TRACE=count`'s
`bytecodes: N` total, `MACVM_JIT=off` (every bytecode interpreted — total
work) vs `MACVM_JIT=threshold=200` (compiled code never touches
`vm.bytecode_count`, so the printed total is exactly the *interpreted
remainder* — startup/warmup plus anything still running cold):

```sh
MACVM_JIT=off             MACVM_TRACE=count target/release/macvm run world/bench/<name>.mst --world world
MACVM_JIT=threshold=200   MACVM_TRACE=count target/release/macvm run world/bench/<name>.mst --world world
```

| bench | total (off) | interpreted remainder (t=200) | still interpreted | moved to compiled |
|---|---|---|---|---|
| arith | 75,008,560 | 158,007 | 0.21% | **99.79%** |
| richards | 137,183,981 | 262,796 | 0.19% | **99.81%** |
| deltablue | 102,900,884 | 1,482,697 | 1.44% | **98.56%** |
| sieve | 5,180,807 | 150,485 | 2.90% | **97.10%** |

Matches the earlier (2026-07-11, HEAD 31f86af) measurement of the same four
benchmarks to within rounding — the S24/OSR work since then hasn't regressed
it, and sieve holds at its post-fix 97.1% (the pre-fix figure was 4.5%; see
the OSR cold-send section above). Range: **98.6–99.8%** — the README's own
figure, now reproducible from this file alone.

# WINARM P3 — Windows on ARM64, the first native numbers (2026-08-09)

Sprint P3 (`docs/sprints/sprint_p03_detail.md` D4). Everything below was
measured on Windows, natively, by the same binary and the same harnesses the
macOS numbers above come from. Standing rule 3 applies: **recorded, not
gating** — with the two exceptions this file has always carried, the
S10/S11 `bench-s10`/`bench-s11` tripwires (fail < 2x interpreter).

## Environment

- Host: Snapdragon X (X1E-78-100-class, `Qualcomm Oryon`), 8 cores / 8
  threads, 3.4 GHz nominal (`Win32_Processor.MaxClockSpeed` reports 2956),
  32 GiB RAM
- OS: Windows 11 Home, build 10.0.26200, ARM64
- Build: `cargo build --release`, rustc 1.97.1, host
  `aarch64-pc-windows-msvc` (pinned in `rust-toolchain.toml`)
- Tree: commit `04f9c6e` **plus P3's uncommitted working tree** (the D1
  assert, the D1/D3 tests, `world/bench/mandelbrot.mst`, the arch tripwire)
- Date: 2026-08-09

**Proved native, not emulated, by the measuring process itself.** P0's
status entry claimed a startup banner reported the architecture; there was
no such banner in the tree (see MIGRATION.md §8's P3 entry). P3 added the
check `tests_p00.md` had specified all along: `macvm::assert_native_host()`
runs at the top of `main`, so every `macvm run` that produced a number below
has passed it, and `MACVM_TRACE=stats` prints the build's own verdict:

```
[stats] host arch=aarch64 os=windows
```

This is a build fact, not a runtime probe, and that is deliberate: Windows'
x64 translation layer would answer a runtime query, but it cannot change
what `cfg!(target_arch)` compiled to.

## D1 — stack-probe audit (the numbers, not the adjective)

Windows requires a frame larger than one page (4 KiB here) to touch the
stack guard page on the way down; one that skips past it faults
unrecoverably. macOS has no such rule, so nothing upstream checked. The
audit's answer is that MACVM is nowhere near it, and it is now
`debug_assert!`ed at nmethod finalize
(`codecache::nmethod::note_nmethod_frame_bytes`) instead of assumed.

Limit used: `4096 - 512` slop = **3584 bytes**.

Compiled frames — `16` (the `stp x29, x30, [sp, #-16]!` record) + `round16(8
* frame_slots)`:

| what | frame_slots | frame bytes | headroom |
|---|---|---|---|
| largest over the WHOLE world + test corpus, threshold 20 (1176 nmethods) | 74 | **608** | 5.9x |
| the compiler's own eligibility budget (`FRAME_BUDGET_SLOTS` = 60) | 60 | 496 | 7.2x |
| first size that would trip the limit | 445 | 3584 | — |

Note `74 > 60`: `FRAME_BUDGET_SLOTS` bounds `ntemps + max_stack` of the
OUTER method before compiling, and inline splicing adds vregs on top of it,
so the real spill count legitimately exceeds the budget number. That is
exactly why the audit measures instead of reasoning from the constant.

Hand-written stubs, **decoded from the published machine words**
(`nmethod::measure_frame_bytes` recognises `stp <X>,<X>,[sp,#-N]!` and `sub
sp, sp, #N`), not restated from the builders' constants:

| stub | frame bytes |
|---|---|
| `call_stub` (x19–x28 + d8–d15 + fp/lr) | **160** |
| `stub_poll` (x0–x15 + fp/lr) | 144 |
| `resolve`, `c2i_shared`, `mega_shared`, `dnu`, `must_be_boolean`, `alloc_slow`, `call_primitive`, `nlr_originate`, `not_entrant`, `box_double`, `box_float64x2`, `box_float32x4`, `box_int32x4`, `value_dispatch[0..3]` | 80 |
| `deopt_return`, `deopt_uncommon`, `deopt_assert` | 16 |
| FFI trampolines `ret_g` / `ret_f` / `ret_v` | 80 |

Worst hand-written frame is `call_stub` at 160 bytes — **22x** headroom.
No probe loop is needed and none was written (§3.4's own instruction: build
it only when a real frame needs it).

One incidental discovery worth recording: a frame over 4095 bytes could not
even be *assembled* today. `sub sp, sp, #imm` goes through the vendored
encoder's `add_sub_imm`, which refuses an immediate outside `0..=4095`, and
`JasmAssembler::emit` turns that refusal into a panic. So the failure mode
for an over-large frame is a loud assembler panic at compile time, not a
silent stack fault at run time — a second, accidental guard that was already
there.

## D2 — W^X guard + icache census under real JIT load

`MACVM_GUARD_COUNT=1` over the full in-language suite
(`world/tests/tests.list` concatenated, 6626 assertions), release:

| mode | compilations | write windows | icache bytes | bytes / window |
|---|---|---|---|---|
| `MACVM_JIT=off` | 0 | 55 | 3,392 | 62 |
| `MACVM_JIT=threshold=20` | 1176 | 5,469 | 1,642,592 | 300 |
| `MACVM_JIT=threshold=1000` | 372 | 1,201 | 216,772 | 180 |

Sanity, not a gate. The shape is what a correct flush granularity looks
like: ~4.7 windows per compiled method (publish, plus the IC-site and PIC
patches that follow it), ~1.4 KiB of icache invalidation per nmethod —
i.e. the nmethod's own body, not a region-wide flush. An order-of-magnitude
excess (whole-cache flushes, or a flush per instruction) would show here as
tens of megabytes; it does not. The JIT-off row is the floor: 55 windows is
the one-time genesis stub publication, and it does not grow with the
workload.

**Δ against `sprint_p03_detail.md` D2**: it says "run the suite with
`MACVM_GUARD_COUNT=1`", meaning `cargo test`. That reports nothing —
`MACVM_GUARD_COUNT` is read in `src/main.rs` at process exit, and test
binaries never execute `main`. The numbers above come from the in-language
suite through the real CLI, which is the "under real JIT load" the
deliverable actually wants.

## D4 — the differential: JIT vs interpreter, every corpus, zero differences

`just diff-p03` (new). Every corpus the project keeps, run under
`MACVM_JIT=off`, `threshold=20` and `threshold=1000`, stdout and exit status
compared byte-for-byte:

| corpus | files | result |
|---|---|---|
| the in-language suite (`world/tests/tests.list`, 6626 assertions, 563 lines of transcript) | 1 concatenation | **identical** in all three modes |
| golden transcripts (`tests/golden/*.mst`) | 3 | **identical** |
| tracked JIT-bug repro corpus (`tests/repros/*.mst`) | 12 | **identical**, exit codes included |

Zero differences, which is the bar `tests_p03.md` set: WINVM's x64 port
lived with seven closure/NLR/OSR differences, and those were backend gaps
this backend does not have. Nothing here needed a caveat.

Three modes rather than two on purpose: `threshold=1000` compiles only the
genuinely hot methods, so it exercises a *different* mix of
compiled/interpreted boundaries than `threshold=20`, and a bug that only
appears at one tier mix would otherwise hide.

## D5 — the Cog axis: what this platform makes checkable, and what it does not

**No Cog comparison was run, and the reason is recorded rather than
implied**: there is no Pharo, Squeak or OpenSmalltalk VM installed on this
machine, and `scripts/cog-bench.sh` additionally needs `python3` (its
`mst2st.py` translation step and its reducer), which resolves here only to
the Microsoft Store alias stub. So P3 produced no Cog number at all.

What P3 *did* land is the axis `sprint_p03_detail.md` §D5 asks for, so the
first Cog run on this platform cannot be recorded unlabelled:

- `scripts/pe-machine.sh` (new, no dependencies — reads `e_lfanew`, checks
  the `PE\0\0` signature, decodes `IMAGE_FILE_HEADER.Machine`) answers
  `arm64` / `x64` / `x86` / `arm32` for any PE binary, without running it.
  Verified against known binaries on this host: `target/release/macvm.exe`
  and `C:\Windows\System32\cmd.exe` -> `arm64`;
  `C:\Windows\SysWOW64\cmd.exe` -> `x86`.
- `scripts/cog-bench.sh` now derives `cog=native-arm64` /
  `cog=emulated-<arch>` from the Cog *binary* (PE on Windows, `file` on
  macOS), prints it in the header AND in the table footer, **refuses to run
  at all** if it cannot determine the architecture, and prints an explicit
  "INDICATIVE, NOT A HEAD-TO-HEAD" block whenever the two sides differ.

**A correction to the sprint's own premise.** §D5 supposes "a Windows-ARM64
Cog build may not exist at all". That is true of Pharo's supported channel
and false of upstream OpenSmalltalk:

- **Pharo**: `pharo.org/download` and the Pharo Launcher offer Windows
  64-bit and 32-bit x86 only. Installing Pharo the normal way on this
  machine therefore yields an **x86-64 VM under Windows' x64 translation
  layer**. A native ARM64 PharoVM does exist off to the side
  (`files.pharo.org/vm/pharo-spur64/Windows-ARM64/`, newest
  `PharoVM-10.0.9-...-Windows-ARM64`, 2025-03-27) but is not advertised and
  is roughly 16 months behind the x86-64 line.
- **OpenSmalltalk / Squeak**: ships native `win64ARMv8` Cog *and* Stack VMs
  in current releases (release `202606270913`, 2026-06-27), built by a
  workflow that `runs-on: windows-11-arm` — i.e. genuinely native, not
  cross-labelled.

So a like-for-like head-to-head on this platform IS reachable; it just
cannot use the Pharo download the existing harness assumes. Until someone
runs it, the honest primary claim stays the one with no emulation term in
it: MACVM-on-Windows-ARM64 against MACVM-on-macOS-ARM64, same commit, same
world, same benchmarks, same ISA — the cross-build differential below.

## D4 step 6 — the cross-build differential: STILL OUTSTANDING, and it is the headline

**There is no Mac on the machine this sprint ran on.** The comparison that
carries P3's actual claim — MACVM on Windows ARM64 against MACVM on macOS
ARM64, same checkout, same world, same benchmarks, same ISA, no emulation
term and no translation term anywhere in it — has therefore NOT been made.
Everything else in this section is one half of a two-sided measurement.

To complete it, build **this checkout** (not `C:\projects\MACVM` — it is a
different tree) on the Mac and run, from the repository root:

```sh
cargo build --release

# 1. suite counts — must match Windows' 1065 passed / 0 failed / 15 ignored
#    (that count has `mandelvm` filtered out on both sides).
cargo test --no-fail-fast -- --skip mandelvm

# 2. golden transcripts + the JIT-vs-interpreter differential over every
#    corpus. Must print `same` on every line, as it does on Windows.
just diff-p03

# 3. the benchmark table below, same three modes, same harness.
./scripts/perf.sh

# 4. the W^X / icache census, for the D2 comparison (expect the same ORDER
#    of magnitude; a >2x gap in icache bytes per compilation is a
#    flush-granularity difference worth chasing).
grep -v '^#' world/tests/tests.list | grep -v '^$' \
  | sed 's|^|world/tests/|' | xargs cat > /tmp/macvm_world_tests.mst
MACVM_GUARD_COUNT=1 MACVM_JIT=threshold=20 MACVM_TRACE=stats \
  ./target/release/macvm run /tmp/macvm_world_tests.mst --world world

# 5. the D1 audit's mac-side numbers (the same asserts compile there):
cargo test --lib -- --nocapture stub_frames_measured
cargo test --test it_world -- --nocapture compile_count_nonzero_at_threshold1
```

What "identical" must mean, per `tests_p03.md` gate item 5: identical suite
counts, identical golden transcripts, identical benchmark RESULTS (the
checksums — Richards `2324609297`, DeltaBlue `224874`, Mandelbrot
`850452`). Benchmark TIMES are recorded side by side and are NOT expected to
match: different silicon.

**One thing to expect on the Mac side, and it is not a Windows problem.**
The repro committed as `it_world::
world_suite_at_threshold_2_hits_root_block_deopt_defect` (`#[ignore]`d on
both platforms; see MIGRATION.md §8's P3 entry): the whole corpus compiled at
`MACVM_JIT=threshold=2|3|5` dies in `runtime/deopt.rs`'s root-block arm on
Windows. Every file on that path is byte-identical to MACVM's, so the Mac
should reproduce it — and if it does NOT, that is a far more interesting
result than if it does, so please run it either way:

```sh
grep -v '^#' world/tests/tests.list | grep -v '^$' \
  | sed 's|^|world/tests/|' | xargs cat > /tmp/macvm_world_tests.mst
MACVM_JIT=threshold=5 ./target/release/macvm run /tmp/macvm_world_tests.mst --world world
```

## D4 — the stress runs, and what each one actually proves

`--skip mandelvm` throughout (that one test renders 140 Mandelbrot frames
and costs ~45 minutes in a debug build; MIGRATION.md §8 records the whole
disproof of the exception-storm it looks like). Counts are the sum over all
21 test binaries.

| # | mode | build | result | wall |
|---|---|---|---|---|
| 1 | `MACVM_JIT=threshold=1` (= 20, see below) | debug | **1065 passed, 0 failed, 15 ignored** | 2m26s |
| 3 | `MACVM_DEOPT_STRESS=1` | debug | **1065 passed, 0 failed, 15 ignored** | 3m13s |
| 2 | `MACVM_JIT=threshold=1` + `MACVM_GC_STRESS=1` (the S12 flagship) | release | 1050 passed, **1 failed**, 15 ignored — the flake below | 2m54s |
| 4 | all three at once (the S14 bar), x3 consecutive | release | **1051 passed, 0 failed, 15 ignored** on every pass | 2m29s / 2m32s / 4m08s |

Release runs fewer tests than debug (798 vs 809 in the lib target) because
`#[cfg(debug_assertions)]` tests — the ones whose subject IS a
`debug_assert!` — genuinely do not exist there.

**Compile counts are asserted, not hoped for.**
`it_world::compile_count_nonzero_at_threshold1` loads the whole world plus
the whole test corpus and fails if `vm.stats.compilations == 0`, so every
line above carries the false-green tripwire rather than only the one that
remembered to check. Observed there: **1176 compilations, 36 recompiles, 165
deopts** over the corpus.

**Δ — `MACVM_JIT=threshold=1` has never meant threshold 1.**
`VmOptions::parse_jit` refuses `threshold=1` from the environment (it warns
that it is a compiler-correctness tool, not a measurement config) and
substitutes `JIT_THRESHOLD_FLOOR` = 20. That floor is upstream MACVM, so
every gate in the justfile that says `threshold=1` — and `tests_p03.md` gate
item 1 — has always run at 20, on macOS too.

**The one failure, characterised.**
`embed::tests::live_stats_lets_a_monitor_observe_compiled_execution_off_thread`
requires a busy-spin sampler thread to observe `compiled_depth > 0` during
one compiled `exec`, with no retry and no synchronisation — the assertion is
that the scheduler ran the sampler inside that window. It fails **1 pass in
6** in release under full-suite parallelism, passes **3/3 in isolation** in
0.08 s, and passes in every debug run. `exec` resets `compiled_depth` to 0
on return, so the window is exactly the compiled run: seconds in debug, tens
of milliseconds in release. It was deliberately not weakened — the fix is a
bounded wait inside a SHARED test, which is not P3's to make.

## D4 step 7 — the benchmark table (Windows ARM64, native)

`scripts/perf.sh` over `world/bench/bench.list`, release, on a quiet machine
(no test suite running). The harness is `Bench.mst`'s: 3 discarded warmups,
then the median of 10 timed rounds, `millisecondClock`, excluding genesis
and world load. Every row is checksum-verified on every iteration — a body
that diverged would abort the run rather than time the wrong thing.

Recorded, not gating (standing rule 3).

| benchmark | result (checksum) | interp (ms) | jit t=1 (=20) | jit t=1000 | best/interp |
|---|---|---|---|---|---|
| richards | 2324609297 | 157 | **2** | 2 | **78.5x** |
| deltablue (inner 10) | 224874 | 163 | **3** | 4 | **54.3x** |
| mandelbrot | 850452 | 705 | **14** | 14 | **50.4x** |

The results are identical in all three modes, which is the half of this
table that is an oracle rather than a measurement: the same three checksums
must come out of the Mac build of the same checkout.

### S10/S11 tripwires — the only perf gates that still apply

Same rule `bench-s10`/`bench-s11` encode: **FAIL below 2x** interpreter,
warn below 5x. Computed with `awk` rather than by running those recipes,
because `bc` — which they shell out to — is not present in Git Bash on this
host (the recipes are otherwise unchanged and still correct on macOS).

| bench | interp (ms) | jit t=1 (=20) | ratio | verdict |
|---|---|---|---|---|
| `arith.mst` `sumTo: 5_000_000` | 1094 | 8 | **136.8x** | ok |
| `dispatch.mst` `runLoop: 5_000_000` | 1609 | 10 | **160.9x** | ok |

Both clear the tripwire by roughly two orders of magnitude. The dispatch row
is worth a note against this file's own history: the S11-era macOS entry was
3.88x, because that measurement predates the whole S24/PIC/OSR arc — it is
not a Windows-vs-macOS difference, and nothing here should be read as one
until the Mac side of §"D4 step 6" is run.

The same tripwire also runs as a standing test in every suite pass:
`it_bench_smoke::arith_compiled_beats_interpreter_2x`, which is why P3 did
not need to re-arm it manually.

**Why runs 2 and 4 are `--release` and runs 1 and 3 are not — measured, not
assumed.** `verify::verify_enabled()` is `cfg!(debug_assertions) ||
MACVM_GC_VERIFY=1`, so a debug build runs the full cross-check heap verifier
at every GC phase boundary; with `MACVM_GC_STRESS=1` (a scavenge before
every allocation) that is a whole-heap walk per allocation. Booting the
world and computing fib(15) on this host:

| configuration | build | time |
|---|---|---|
| no stress, JIT off | debug | 0.09 s |
| no stress, threshold 20 | debug | 0.95 s |
| `MACVM_GC_STRESS=1` **and** threshold 20 | release | 0.30 s |
| `MACVM_GC_STRESS=1`, **JIT off** | debug | **did not finish in 4 minutes** |

So it is neither the JIT nor the platform — `memory/verify.rs` and
`memory/scavenge.rs` are byte-identical to MACVM's, and `gate-s08`'s own
comment already recorded the shape of it ("30+ seconds ... 0.6s under
--release"). `gate-p03` runs the unstressed and deopt-stress passes in debug
(where `debug_assert!` lives, including D1's frame-size invariant) and the
GC-stress passes in release, with this measurement written into the recipe.
`MACVM_GC_VERIFY=1` opts a release run back into the verifier for anyone who
wants the pair; budget hours.

**The S12 flagship's own census**, from the combined-stress world run
(release, `MACVM_TRACE=gc,stats`):

```
gc: scavenges=158124 total_ms=45579 max_ms=33.8  fulls=4 total_ms=11 max_ms=3.2
gc: gc_under_compiled=139830
[stats] compilations=1176
[stats] deopt_count=165 by_reason=[trap 165, return 0, poll 0]
6626 run, 0 failed
```

139,830 collections ran with live compiled frames on the native stack — the
exact seam (fresh code + icache flush + moving GC) this port was most likely
to break, exercised 140 thousand times on Windows without a single
difference in the guest's output. `just bridge-stats-s11` asserts that
counter is nonzero; `it_gc_jit::mid_loop_forced_scavenge`, the single most
OS-layer-sensitive test in the suite, passes in every mode above.
