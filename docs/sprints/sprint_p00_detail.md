# Sprint P0 — Toolchain, seed, and interpreter-only Windows ARM64

Objective: `cargo build` and the full test suite run green on
`aarch64-pc-windows-msvc` with the JIT off — a working interpreted Smalltalk
on Windows ARM64, every OS seam gated the way WINVM's M0/M1 proved, no
compiler back-end work of any kind. Implements MIGRATION.md §2 (component
disposition), §3.4 (page-size audit), §3.5 (cfg untangling), §4 (repo
strategy). WINVM's `MIGRATION.md` §6 status log entries for M0/M1 are the
worked example of exactly this sprint on x64.

## Prerequisites

- This checkout (WINARM = current MACVM seed). No sprint precedes P0.
- Reference checkouts readable at `C:\projects\WINVM` and
  `C:\projects\MACVM`.
- Nothing else — this sprint *creates* the toolchain.

## Deliverables

- Working toolchain, versions recorded in this sprint's status entry:
  `rustup` with host `aarch64-pc-windows-msvc`, VS 18 ARM64 C++ build tools
  + Windows SDK verified via an actual link, `just` available for the gate
  recipes.
- Repo hygiene: branch `winarm64-port`; `MACVM` added as `upstream` remote;
  `.gitattributes` pinning `* text=auto eol=lf` (WINVM lesson — scripted
  edits stay byte-clean).
- Workspace surgery in `Cargo.toml` (§D1).
- OS-seam patches (§D2) — the port's entire code change for this sprint.
- Test gating (§D3) with a tracking table in this file's status entry.
- `just gate-p00` recipe.

## Design

### D1. Workspace surgery

Follow WINVM's manifest (its header comments document each decision):

- `members = [".", "image_store", "ffi_gen"]` — `gui` joins in P4;
  `cocoa_gui`, `abc_player`, `gui/mock_vm` (if gui-coupled) leave until
  their Windows story exists. `default-members = ["."]` stays.
- MacGamePane path deps must not be *declared* on Windows — a cargo path
  dependency's manifest must be readable for the workspace to resolve AT
  ALL, even optional and target-gated (WINVM M6 finding). WINVM's
  `gui/Cargo.toml` records how to restore them; same note here.
- Keep package name `macvm`, `default-run`, both bins (`asm_preview` is
  arch-correct on this target — it drives the A64 assembler).
- `iced-x86` is NOT taken (x64-only need; `disasm_a64.rs` serves PROBE
  here).
- Defer `[target.'cfg(windows)'.dependencies] rusqlite` to P5 (winkb is P5
  scope; keep P0's dependency delta zero).

### D2. OS-seam patches — the complete list

Binding rule (MIGRATION.md §2): current files are the base; WINVM
contributes **diffs**, not file copies — its shared-file copies are 2½
weeks stale. For each seam below, open WINVM's version of the same file,
locate its `// WINVM:` additions, and re-apply them here (they were written
to be portable across arch — reservation/probe/vm_state contain no x64
code).

| # | File | WINVM reference | The patch |
|---|---|---|---|
| 1 | `build.rs` | `WINVM/build.rs` | gate the objc-shim `cc` compile + `-lobjc` link to `#[cfg(target_os = "macos")]` (build scripts run on the host = the target here, so the cfg is correct on both machines) |
| 2 | `src/memory/reservation.rs` | `WINVM/src/memory/reservation.rs` | add the `#[cfg(windows)] mod win` (VirtualAlloc/VirtualFree/GetSystemInfo, `MEM_RESERVE`+`PAGE_NOACCESS` reserve, `MEM_COMMIT`\|`PAGE_READWRITE` commit, `MEM_DECOMMIT` decommit — zeroed pages exceed the contract) behind the SAME `Reservation` API; unix path keeps mmap |
| 3 | `src/runtime/probe.rs` | `WINVM/src/runtime/probe.rs` | thread stack bounds: `pthread_get_stackaddr_np`/`pthread_get_stacksize_np` (probe.rs:662) get a Windows sibling (TEB `StackBase`/`StackLimit` via `NtCurrentTeb`, or `VirtualQuery` on an address of a local — take whichever WINVM used) |
| 4 | `src/runtime/vm_state.rs` | WINVM same file | the process/thread-exit seam (`ExitThread` analogue of `pthread_exit`) |
| 5 | `src/codecache/deopt_trap.rs` | WINVM same file | gate the Mach/signal layer (`sigaction`, alt-stacks, `sigsetjmp` imports) to `#[cfg(target_os = "macos")]`; portable thread-ids stay; **do NOT port the VEH here** — that is P2's whole scope; until then guest-fatal on Windows may abort (WINVM M0 shipped exactly this state) |
| 6 | Cocoa bridge (`src/runtime/objc_bridge.rs`, objc prims, AppleScript, gamepane prims) | WINVM's gating | clean-fail Windows stubs: each mac-only prim guest-fatals with a clear message rather than failing to compile |
| 7 | `src/runtime/alien.rs` | WINVM's gating | the POSIX FFI demo path (`mmap`/`getpid` tests) gates to macOS until P5 |
| 8 | `src/vendor/wfasm/mod.rs` | WINVM same file | `native_macos` already `#[cfg(target_os = "macos")]`-gated internally; ensure the module list compiles on Windows with NO native loader yet (P1 adds `native_winarm64`); `codecache/guard.rs`'s import of `jit_write_protect`/`icache_invalidate` therefore needs a temporary Windows stub pair (no-op + no-op) **clearly marked `// P0: replaced in P1`** so the crate links with the JIT off |
| 9 | `src/main.rs` / `src/lib.rs` | WINVM same files | whatever residual mac-only wiring the compile surfaces (WINVM's M0 list: "Cocoa bridge + prims gated with clean-fail Windows stubs") |

JIT posture in P0: `JitMode::Off` semantics must be the effective default on
Windows until P2 lands (compile guard: tier-up entry refuses on
`cfg(windows)` with a logged reason, removed in P3). The A64 emitters
COMPILE (pure Rust) — they are simply never called.

### D3. Test gating — tracked, not silent

WINVM's M2 entry enumerates its gated set; WINARM's differs (arch tests stay
ALIVE here). Gate ONLY what is OS-coupled, and record every gate in the
status entry with its un-gating sprint:

| Test group | Gate to | Comes back |
|---|---|---|
| signal-based fault recovery + `sigsetjmp` tests | `target_os = "macos"` | P2 (as VEH/winjmp twins) |
| embedded-VmHandle integration (`embed::tests`) | `target_os = "macos"` | P2 |
| real-FFI tests (`mmap`/`getpid`), alien POSIX | `target_os = "macos"` | P5 (as Win32 twins) |
| Apple 16-KiB page commit assertion | `target_os = "macos"` | never (a Windows twin asserts 4-KiB behavior instead, this sprint) |
| Cocoa bridge / cocoa_main_hop / cocoa_delegate harness tests | `target_os = "macos"` | never (mac-only feature) |
| tier-1 compile+execute tests | **NOT gated** (arch-correct here) | n/a — expected to FAIL to run only because the loader/traps are absent; mark `#[ignore = "P1/P2"]` individually, never cfg them out |

The distinction in the last row is deliberate: `#[ignore]` keeps them
compiling against the real emitters (drift shows up immediately) and the
string names the sprint that removes it.

### D4. Page-size audit

`rg -n "0x4000|16384|16 KiB"` across `src/` (excluding `vendor/wfasm/a64`
encoder tables, which are instruction encodings, not page math). Known
sites: `vendor/wfasm/native_macos.rs::PAGE` (already mac-gated — fine); the
gated commit assertion (D3). Anything else found: fix to query the
reservation's page size. Record the sweep result even if empty.

### D5. Runtime arch assertion

`-v` startup output and the world banner gain
`arch=aarch64 os=windows` sourced from `cfg!` — and a debug assert that
`cfg!(target_arch = "aarch64")` holds, so an accidental x64-emulated build
(this machine runs x64 binaries transparently) announces itself instead of
silently benchmarking emulation. MIGRATION.md §6 risk table names this.

## Implementation order

1. Toolchain: install rustup (host `aarch64-pc-windows-msvc`); `rustc -vV`
   records the host triple; build + run a hello-world with a `cc`-crate dep
   to prove VS ARM64 C++ tools + SDK end to end. **Stop and fix the
   toolchain if this fails — nothing else is worth attempting.**
2. Repo hygiene: branch, upstream remote, `.gitattributes`.
3. D1 workspace surgery; `cargo metadata` succeeds.
4. D2 patches in the numbered order (each keeps `cargo check` progressing —
   expect the error count to fall monotonically; build.rs first so the objc
   shim stops blocking everything).
5. D3 gating + D4 audit + D5 assertion.
6. `cargo build` clean (zero warnings — WINVM's M0 bar), `cargo clippy`
   clean, `cargo test` green, world boots, full world suite interpreted.
7. Record interpreted DeltaBlue ×10 / Richards ×10 timings in the status
   entry (WINVM M1's comparable record was 126 ms / 1135 ms on its x64
   host; these are the first **native-ARM64-Windows** numbers — a fresh
   baseline that P3's PERF.md section builds on).
8. `just gate-p00` recipe: `ci` + the world suite run.

## Pitfalls

- **WINVM file copies are stale.** The one rule that prevents silent
  regression of the Aug 2026 compiler gains: patches, never copies, for any
  file that exists in both repos (MIGRATION.md §2).
- **`libc` on Windows exposes CRT only, not Win32** — declare kernel32
  imports by hand (`extern "system"`), exactly as WINVM's reservation/loader
  do. No `windows-sys` dependency in the core crate.
- **build.rs cfg is HOST cfg.** Fine here (host == target on both machines),
  but say so in the comment — WINVM's does.
- **Allocation granularity is 64 KiB** — reservation base addresses align to
  it even though pages are 4 KiB. WINVM's reservation handles this; keep its
  math intact.
- **`cargo test` may hit path-length or LF issues on Windows** — the
  `.gitattributes` and keeping the checkout at `C:\projects\WINARM` (short
  root) preempt both.
- **Do not "fix" mac-only code to compile by deleting it.** Gate it. The
  upstream remote stays cherry-pickable in both directions only if shared
  files stay shaped alike.

## Interfaces for later sprints

- P1 replaces the D2#8 stub pair with `native_winarm64.rs` — the stub's
  marker comment is its work order.
- P2 owns everything gated in D3 rows 1–2 and the D2#5 residue.
- P3 removes the D2 JIT-off guard and the `#[ignore = "P1/P2"]` strings.
- P5 owns D3 row 3 and the winkb/rusqlite dependency delta.

## Out of scope

- Any JIT execution (P1), any trap handling (P2), GUI (P4), FFI (P5).
- Performance work of any kind — P0's timings are a baseline record only.
- CI hosting; `just` recipes remain the CI contract (CONVENTIONS §5).
