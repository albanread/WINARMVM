# Sprint P1 — Test Plan

## Acceptance gate

On this machine, natively:

1. S9's smoke trio executes from the new region: emit `(x+1)*2` → call →
   correct result; an internal branch; a call to a Rust `extern "C"` fn.
2. **Patch-and-rerun proves the flush**: publish a blob, execute it, patch
   a `Branch26` (and a literal-pool word) under a `JitWriteGuard`,
   re-execute, observe the NEW behavior. Run the patch loop ≥ 1000
   iterations alternating targets — a stale-icache bug is probabilistic
   and one pass proves little.
3. `corpus_replay` green on target.
4. Every P0 `#[ignore = "P1/P2"]` test whose only missing piece was the
   loader is un-ignored and green; the remainder now say `#[ignore = "P2"]`.
5. `cargo test` + clippy clean; `just gate-p01` (chains gate-p00) passes.

## Unit tests

| Test | Module | Assertion | Rationale |
|---|---|---|---|
| `ret_blob_executes` | `vendor::wfasm::native_winarm64` | a hand-written `ret` (0xD65F03C0) placed + flushed + called returns | smallest end-to-end; isolates loader from encoder |
| `region_is_page_rounded_native` | same | region cap is a multiple of `GetSystemInfo` page size (4096); no 0x4000 constant reachable | MIGRATION §3.4 page delta |
| `extern_resolution_kernel32` | same | `GetProcAddress(GetModuleHandleA("kernel32"), "GetTickCount64")` resolves and the veneer path calls it | host-extern shape (WINVM's test, A64 veneer instead of movabs) |
| `far_target_goes_through_veneer` | `relocpatch` (existing) | out-of-±128 MB target patches to a `movz/movk x16; br x16` veneer | already exists on macOS; runs unchanged here — its passing IS the "no new relocpatch" claim |
| `guard_pair_signature_matches_macos` | compile-time | the fn pair's types are identical across both native modules (a `const _: fn(*const u8, usize) = …` assignment per side) | guard.rs must not need per-OS call shapes |
| `write_without_guard_is_impossible` | `codecache` (audit) | D3 sweep recorded: all region writes flow through `JitWriteGuard` | the RWX-no-toggle risk's paper trail |

## Integration/golden tests

- `tests/it_codecache.rs` (existing S9 suite): green in full — publish
  protocol, write-protect round-trip (now a no-op + real flush), literal
  pool patching.
- The 1000-iteration patch-flip loop lives here (gate item 2), tagged
  `#[test]` not `#[bench]`; runtime budget ~seconds.

## In-language tests

None — no Smalltalk reaches the JIT until P3. (The world suite still runs
in the gate via the gate-p00 chain, unchanged.)

## Stress/negative tests

- **Deliberate stale-icache reproducer, then fixed**: one test builds a
  private region, writes blob A, executes, writes blob B **without**
  invalidating, and documents that the result is UNDEFINED (may return A's
  answer) — then invalidates and asserts B. Marked `#[ignore]` by default
  (its first half is timing-dependent by nature); its value is executable
  documentation of WHY the pair exists. The always-run assertion is the
  second half.
- Publish into a full region: `alloc` returns the existing error path (no
  new failure mode on Windows); test that it does.
- Free + realloc reuse: existing S9/S12 free-list tests — unchanged, but
  their passing on target is part of the gate.

## Non-goals

- No `brk`/trap execution (P2 — a trap in P1 is a process kill, and no test
  may plant one). No tier-up, no Smalltalk-driven compilation (P3). No
  perf measurement of flush cost (post-P3, via the existing
  `MACVM_GUARD_COUNT=1` counters).
