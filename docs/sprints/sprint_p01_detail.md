# Sprint P1 — JIT substrate: loader, W^X, and a REAL icache flush

Objective: MACVM's existing A64 machine code executes from a `VirtualAlloc`'d
code region on Windows ARM64, published and patched through the existing
`JitWriteGuard` protocol, with instruction-cache invalidation that is
**load-bearing** (split I/D caches — this is the one place Windows-on-ARM64
is macOS-shaped, not x64-shaped). Implements MIGRATION.md §3.1. The S9 gate
(emit / execute / patch / re-run) is the acceptance shape, re-run on this
target. No trap handling (P2), no tier-up (P3).

## Prerequisites

- P0 green: crate builds/tests on Windows ARM64, JIT off; the D2#8 stub pair
  (`// P0: replaced in P1`) marks exactly the seam this sprint fills.
- Existing, unchanged: the vendored A64 encoder + `relocpatch.rs`
  (`Branch26` in place, `movz/movk x16; br x16` veneers for far targets),
  `codecache/` region management, `guard.rs`'s `JitWriteGuard` with its P9
  Drop order, the S9 test suite.
- Reference: `WINVM/src/vendor/wfasm/native_windows.rs` (`WinJit`) — the
  x64 sibling this file mirrors; `native_macos.rs` — the macOS sibling whose
  MACVM-added surface (`region_raw`, standalone `jit_write_protect` /
  `icache_invalidate`) defines the exact API `codecache` consumes.

## Deliverables

- `src/vendor/wfasm/native_winarm64.rs` — the Windows-ARM64 loader
  (~250 LoC), vendor-header discipline observed (it is a NEW sibling, not a
  vendored copy — header says so, pointing at both siblings).
- `src/vendor/wfasm/mod.rs` — os/arch dispatch: `#[cfg(all(windows,
  target_arch = "aarch64"))] pub mod native_winarm64;` re-exported under the
  name `codecache` already imports (the WINVM trick: alias so call sites
  stay identical).
- `src/codecache/guard.rs` — import switched to the dispatched module; P0's
  stub pair deleted.
- Corpus + S9 tests green on target; new tests below.
- `just gate-p01`.

## Design

### D1. `WinA64Jit` (mirroring `MacJit`'s consumed surface exactly)

```rust
pub struct WinA64Jit {
    region: *mut u8,
    cap: usize,
    // …the same build/place/finalize fields MacJit carries, minus
    // the writable/per-thread-toggle state (no such state on Windows)
}
```

- `with_capacity(cap)`: one `VirtualAlloc(null, cap, MEM_RESERVE |
  MEM_COMMIT, PAGE_EXECUTE_READWRITE)`; round `cap` to the **native page
  size from `GetSystemInfo`** (4 KiB — never a hardcoded 0x4000; P0's audit
  keeps it that way). Committing the whole region up front matches both
  siblings (the code cache does its own bump/free management inside).
- kernel32 imports by hand (`extern "system"`): `VirtualAlloc`,
  `VirtualFree`, `GetCurrentProcess`, `FlushInstructionCache`,
  `GetSystemInfo`, `LoadLibraryA`, `GetModuleHandleA`, `GetProcAddress` —
  the same list WINVM declares; no new crate.
- `region_raw()` — the MACVM addition `CodeCache` manages the region
  through; identical signature.
- Host-extern resolution (`dlsym_resolve` analogue): `GetModuleHandleA` /
  `LoadLibraryA` + `GetProcAddress` — WINVM's exact shape.
- Relocation: **the vendored A64 `relocpatch.rs` unchanged.** `Branch26`
  patched in place when in range; out-of-range and absolute targets go
  through the existing veneer emitter (`movz/movk x16; br x16`,
  `VENEER_LEN`). There is NO `patch_relocs_x64` analogue to write — the A64
  path predates the port. Near-host `VirtualAlloc2` placement (WINVM Phase
  3i) is explicitly NOT taken in v1: veneers already make far targets
  correct, `bl` reaches ±128 MB within the one region, and the win it bought
  x64 (`rel32` everywhere) has no A64 counterpart worth the API-availability
  dance. Recorded as a perf option for post-P3 measurement.

### D2. The W^X pair — where this target is macOS-shaped and where x64-shaped

```rust
pub fn jit_write_protect(_exec: bool) { /* no-op — see below */ }

pub fn icache_invalidate(start: *const u8, len: usize) {
    unsafe {
        FlushInstructionCache(GetCurrentProcess(), start as *const c_void, len);
        core::arch::asm!("isb sy", options(nostack, preserves_flags));
    }
}
```

- **`jit_write_protect` is a no-op**, exactly like WINVM x64: the region is
  RWX for its lifetime; Windows has no `MAP_JIT`/per-thread toggle. The
  RW↔RX `VirtualProtect` hygiene upgrade stays possible later without
  touching call sites (guard.rs already brackets writes) — same note as
  WINVM's loader carries. It must remain **cheap and infallible**: guard.rs
  calls it on every publish/patch cycle.
- **`icache_invalidate` is REAL here** — the sprint's reason to exist.
  `FlushInstructionCache` performs the D-cache clean + I-cache invalidate
  (and the kernel broadcasts to other cores); the local `isb sy` discards
  this thread's own prefetched instructions before it executes freshly
  written code. x64 WINVM calls the same API pro forma; on ARM64 omitting
  either half produces the classic stale-icache failure: rare,
  unreproducible, wrong-instruction execution. macOS's
  `sys_icache_invalidate` bundled both concerns; here they are explicit.
- guard.rs's P9 **Drop order is unchanged and still correct**: "exec-mode
  flip first, then invalidate noted ranges" — the flip is now free, the
  invalidation is the substance. Do not reorder, do not remove the ordering
  comment; annotate it with the Windows reading instead (one sentence).
- `GUARD_ACQUIRES` / `GUARD_ICACHE_BYTES` counters keep working untouched —
  they meter the same events.

### D3. Publish protocol audit

`CodeCache::publish` / `patch_branch26_at` / `patch_pool_word` /
`write_branch26_at` all funnel through `JitWriteGuard` + the pair above —
S9's design did the isolation work already. The audit (grep, not rewrite):
no OTHER path writes to the region (`region_raw` consumers), and every
guard `note()`s before writing (guard.rs's own doc rule). Expected result:
zero changes; record the sweep.

### D4. mod.rs dispatch

```rust
#[cfg(target_os = "macos")] pub mod native_macos;
#[cfg(all(windows, target_arch = "aarch64"))] pub mod native_winarm64;
#[cfg(all(windows, target_arch = "aarch64"))]
pub use native_winarm64 as native_jit;   // or re-export the fn pair —
#[cfg(target_os = "macos")]
pub use native_macos as native_jit;      // match whichever import shape
                                         // guard.rs/codecache already use
```

Keep the WINVM aliasing idea (call sites never change) but let the actual
re-export shape follow what `guard.rs` and `codecache/mod.rs` import today —
minimal-diff beats symmetric-looking.

> **Δ (2026-08-09, measured after P0). Most of D1 is dead code; do not
> build it.** This sprint was scoped at "~250 LoC" on the assumption that
> the Windows loader owes the same surface `MacJit` exposes. It does not.
> Counting real call sites outside `src/vendor/`:
>
> | method | call sites outside `vendor/` |
> |---|---|
> | `load_module`, `finalize`, `define_extern`, `build_and_load` | **0** |
> | `region_raw` | 1 (`codecache/mod.rs`) |
> | `with_capacity` | 43 |
> | `dlsym_resolve` | 3 — all of them FFI (P5's work, not this sprint's) |
>
> `MacJit` carries the build/load/finalize protocol because it is a *vendored
> JASM file* and that is JASM's own API. MACVM deliberately bypasses it —
> S9's own vendoring note says the standalone `region_raw` + W^X functions
> were added "so `CodeCache` can manage the region and W^X state directly
> instead of going through `MacJit`'s own build/load/finalize protocol". So
> `CodeCache` owns allocation, publication and patching; it holds
> `NativeJit` only to own the region's lifetime. Implementing that protocol
> for Windows would produce code with no caller on either platform.
>
> **P1's real content, therefore:** the region + W^X pair (landed in P0), the
> S9 acceptance gate re-proven on this target, and nothing else.
> `dlsym_resolve` moves to **P5**, where its three consumers live. The
> `// WINARM (P1)` marker in `native_winarm64.rs` should be rewritten to say
> "deliberately absent, see this Δ" rather than "lands here".
>
> **Gate status, measured on target:** items 1, 3 and 4 already pass —
> `smoke_arith` / `smoke_internal_branch` / `smoke_call_rust_extern`,
> `patch_and_rerun_branch26`, `literal_pool_patch_rerun`,
> `veneer_fallback_forced`, `publish_is_position_independent`,
> `freelist_reuse_executes`, `two_blobs_one_guard`, and
> `corpus_replay_aarch64_matches_golden`. Item 2's churn loop was added
> (`patch_flip_churn_stays_coherent`).
>
> **And a negative result that matters more than the test does.** The churn
> loop was written to prove the mandatory icache maintenance. It cannot.
> Commenting out **both** `FlushInstructionCache` and `isb sy` leaves it —
> and both other patch tests — still passing on this host. A missing flush is
> simply not observable through this path on this machine, so **no test here
> can be cited as evidence the flush works**, and the D2 pitfall's "no test
> failure ≠ correct" warning is now a measurement rather than a caution.
> Keep both halves regardless: the requirement is architectural (split I/D
> caches; the ARM ARM mandates clean + invalidate + context synchronization
> before executing modified instructions), and tolerance on one core today
> says nothing about another core, later silicon, or a thread resuming
> elsewhere after preemption. `tests_p01.md`'s prescribed
> "deliberate stale-icache reproducer" should be struck for the same reason:
> it cannot be built reliably here.

## Implementation order

1. `native_winarm64.rs` skeleton: region alloc/free + the W^X pair + unit
   test that a `ret`-only blob written into the region executes (the
   smallest possible end-to-end: write 0xD65F03C0, flush, call).
2. mod.rs dispatch + guard.rs import switch; delete P0's stubs; `cargo
   test` — S9's guard tests come alive.
3. Host-extern resolution + veneer path: port the S9 smoke trio (emit
   `(x+1)*2`; internal branch; call a Rust `extern "C"` fn) — these tests
   exist; un-`#[ignore]` the ones P0 marked where their only missing piece
   was the loader.
4. Patch-and-rerun: the S9 literal-pool word patch + `Branch26` retarget
   tests on target (the practical proof the flush works — a stale icache
   fails exactly here).
5. Corpus: `corpus_replay` green on target (encoder is pure Rust and
   already passed on macOS — this run proves nothing about encoding and
   everything about the harness being honest on the new OS).
6. `just gate-p01: gate-p00` + the above.

## Pitfalls

- **The `isb` is not optional.** `FlushInstructionCache` alone leaves THIS
  thread's pipeline holding pre-write fetches. The failure is
  probabilistic; no test failure ≠ correct. The pair goes in together on
  day one, not after a bug hunt — this pitfall is the sprint's headline.
- **`asm!("isb sy")` requires no feature gates on aarch64-msvc**, but keep
  it inside the `#[cfg(all(windows, target_arch = "aarch64"))]` module so
  x64-hosted `cargo check` of the workspace (if ever done) never sees it.
- **Do not copy `native_windows.rs`'s x64 content** (rel32 stubs,
  `movabs`), and do not vendor-header it as if from JASM — it is a new
  MACVM-family file; its header cites the two siblings it mirrors.
- **RWX + no toggle means a wild write can corrupt code silently.** macOS
  had the per-thread toggle as a tripwire. Mitigation is discipline already
  in place (guard-only writes, D3 audit) — but PROBE's code-range dossier
  (P2) is the detection story; note the reduced protection in the loader
  header the way WINVM's does.
- **Allocation granularity 64 KiB vs page 4 KiB**: `with_capacity` rounds
  to pages; the base VirtualAlloc returns is granularity-aligned
  automatically. Don't add alignment math the API already does.
- **`FlushInstructionCache`'s len is bytes, not words**; guard `note()`
  already carries byte lengths. The macOS fn had the same signature — no
  unit slip possible if the pair's signature matches `native_macos.rs`
  exactly (it must).

## Interfaces for later sprints

- P2 consumes: an executable code region + working publish/patch, to plant
  real `brk` sites and trampolines into.
- P3 consumes: everything (tier-up just starts calling `compile`).
- The loader's `LoadLibraryA`/`GetProcAddress` resolution is P5's FFI
  substrate (same fns WINVM's winkb path uses).

## Out of scope

- Trap handling of ANY kind — a `brk` executed in P1 kills the process;
  that is P2's opening state.
- Tier-up wiring, `threshold=` runs (P3).
- W^X hygiene (`VirtualProtect` RW↔RX) and near-image placement — recorded
  perf/hardening options, post-P3.
