# Sprint P0 — Test Plan

## Acceptance gate

All of the following on this machine, natively (`aarch64-pc-windows-msvc`):

1. `cargo build` completes with **zero warnings**; `cargo clippy
   --all-targets -- -D warnings` clean (WINVM's M0 bar).
2. `cargo test` green with the JIT off (the P0 default on Windows).
3. The world boots from `.mst` + SQLite; the full in-language suite runs
   with **the same test/assertion counts and zero failures as the Mac build
   of the same commit** (run it there; paste both counts side by side into
   the status entry). The four FFI-dependent world files are excluded on
   BOTH sides for this comparison.
4. DeltaBlue and Richards produce **correct results** interpreted; timings
   recorded (baseline only — no perf gate, standing rule 3).
5. Startup `-v` reports `arch=aarch64 os=windows`; the debug arch assert is
   present.
6. `just gate-p00` encodes 1–4 and passes.

## Unit tests

| Test | Module | Assertion | Rationale |
|---|---|---|---|
| `reservation_commit_decommit_roundtrip_windows` | `memory::reservation` | reserve → commit → write → decommit → re-commit reads **zero** bytes | `MEM_DECOMMIT`+re-`MEM_COMMIT` zeroes; pins the "contents unspecified" contract is (over-)satisfied — WINVM's own doc claim, now tested here |
| `reservation_page_size_is_native` | `memory::reservation` | reported page size == `GetSystemInfo().page_size` == 4096 | the Apple-16-KiB twin (P0 D3/D4); fails loudly if run emulated where assumptions drift |
| `probe_stack_bounds_contain_local` | `runtime::probe` | address of a stack local ∈ [limit, base) from the Windows bounds read | the exact property the crash dossier's walkback needs |
| `arch_assert_native_arm64` | `main`/`lib` | `cfg!(target_arch = "aarch64") && cfg!(windows)` | tripwire for x64-emulated builds (MIGRATION §6 risk) |
| existing suite | everywhere | unchanged results | the point of the sprint |

## Integration/golden tests

- `tests/it_world.rs` (existing): full world suite, JIT off. The gate's
  count-comparison against the Mac run is manual-but-recorded (two command
  lines + two pasted summaries in the status entry).
- Golden corpus (existing `tests/golden/`): all pass interpreted —
  transcript outputs are platform-independent (LF pinned by
  `.gitattributes`; if any golden fails on CRLF grounds, the fix is the
  attributes file, never the golden).
- Both benches (`world/bench/`): correct results asserted (their own
  self-checks), timings appended to the status entry.

## In-language tests

None new. The existing `world/tests/*.mst` suite IS the gate; its count
parity with the Mac run is the cross-platform differential.

## Stress/negative tests

- `MACVM_GC_STRESS=1 cargo test` and `MACVM_GC_STRESS=full cargo test` —
  both stress modes exist from S7/S8 and are OS-independent; they must be
  green interpreted before any JIT work starts (they localize P1/P2
  breakage later: if a stress mode fails after P2, the OS layer is the
  suspect, not the GC).
- Clean-fail check: one test per gated mac-only prim group (Cocoa,
  AppleScript, gamepane) asserting the guest-fatal message fires rather
  than a crash or a silent nil.
- `#[ignore = "P1/P2"]`-marked tier-1 tests: `cargo test -- --ignored`
  is expected to fail at this sprint — run it once and record the failure
  mode (it should be "no loader", not a compile error; a compile error
  means D2#8's stub seam leaked).

## Non-goals

- No JIT-path testing (P1's gate), no trap/recovery testing (P2's), no
  perf targets (P3 records, S15 rules still apply), no GUI (P4), no FFI
  (P5). Guest-fatal recovery specifically is ALLOWED to abort the process
  in P0 — WINVM shipped M0/M1 in the same state; P2 owns fixing it.
