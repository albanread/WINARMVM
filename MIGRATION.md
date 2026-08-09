# WINARM — Migration Design: MACVM → Windows on ARM64

**Goal.** Produce WINARM, an equivalent of [MACVM](https://github.com/albanread/MACVM)
(two-tier Smalltalk VM: baseline interpreter + adaptive optimizing JIT, generational
GC, direct-pointer object model, `.mst` source worlds) running natively on
**Windows 11 ARM64** (`aarch64-pc-windows-msvc`), on this machine.

**The decisive advantage.** Both halves of the port already exist in this
workshop, written by the same author, and this repo is seeded from the current
MACVM:

- **`C:\projects\MACVM`** (== this checkout's history) supplies the entire
  **AArch64 compiler**: `src/compiler/emit.rs`, `regalloc.rs`, the A64 stub /
  PIC / adapter / trap-site emitters in `src/codecache/`, `disasm_a64.rs`, and
  the vendored JASM A64 encoder (`src/vendor/wfasm/a64/`). The ISA does not
  change between macOS-on-AArch64 and Windows-on-AArch64 — **none of this is
  rewritten**.
- **`C:\projects\WINVM`** (the x86-64 Windows port, seeded from MACVM
  2026-07-21) supplies the **Windows OS layer**, already designed, built, and
  hardened once: `VirtualAlloc` reservation (`src/memory/reservation.rs`), the
  `WinJit` loader shape (`src/vendor/wfasm/native_windows.rs`), the Vectored
  Exception Handler trap layer, the hand-written **non-unwinding**
  setjmp/longjmp (`global_asm!` in `src/codecache/deopt_trap.rs`), the
  `winkb` FFI resolver (`src/runtime/winkb.rs`), the Win32 + WebView2 GUI
  shell (`gui/src/shell/win.rs`), and guest-fatal recovery. Its `MIGRATION.md`
  records every OS seam that was cut **and every convention-mismatch bug the
  cutting found** — this document leans on it throughout.

WINVM's core effort — rewriting the compiler back end for another ISA — **does
not exist in this port**. What remains is the OS layer (mostly proven in
WINVM, portable across arch) plus a small amount of genuinely new
AArch64-Windows glue: an icache-flushing JIT loader, a VEH that decodes A64
`brk`, and an AArch64 setjmp/longjmp. Rough shape: **~95 % carries; ~5 % is
new code, and most of that is a port-of-a-port.**

---

## 1. What this port is NOT (the WINVM contrast)

| | WINVM (x64) | WINARM (this port) |
|---|---|---|
| Compiler back end (`emit.rs`, `regalloc.rs`, ~6 k LoC) | full rewrite (two-address form, 7-reg pool, new aliasing analysis) | **unchanged** |
| Stubs/PICs/adapters/trap sites (`codecache/`, ~9 k LoC) | 14 + 3 + N hand-emitted routines re-written in x64 | **unchanged** (pure A64 emission) |
| Encoder | re-vendor `rasm` x64 | **unchanged** (vendored JASM A64) |
| Register model | re-mapped, pressure redesigned | **unchanged** (x28/x27/x26/x25 pinned, x29 FP chain, x16/x17 scratch) |
| Disassembler | `iced-x86` dependency added | **unchanged** (`disasm_a64.rs`); iced-x86 NOT taken |
| SIMD kernels | NEON `asm!` → SSE2 intrinsics | **unchanged** — `runtime/simd_kernels.rs` is already `#[cfg(target_arch = "aarch64")]`-gated (6 sites), not `target_os`-gated |
| Patch-site shapes | redesigned for variable-width ISA | **unchanged** (fixed-width 4-byte words, `Branch26` + veneers) |
| OS layer | designed from scratch (against JASM/WF66 substrate) | **taken from WINVM**, arch-neutral parts verbatim |

The four convention-mismatch bug classes WINVM's §8 documents (A64 stubs
installed on the wrong host, ICs patched as wrong-ISA words, the missing
`last_compiled_pc` store, the `rt_poll` hidden-pointer ABI) are all either
impossible here (one ISA end to end) or already pinned by WINVM's tests.

## 2. Component disposition

| Component | Source of truth | Action |
|---|---|---|
| `src/compiler/` (all), `src/codecache/` emitters, `src/vendor/wfasm/a64/` | **this repo** (current MACVM) | none |
| `src/bytecode/`, `src/frontend/`, `src/interpreter/`, `src/oops/`, GC logic in `src/memory/` | this repo | none |
| `src/runtime/simd_kernels.rs` | this repo | none (already arch-gated; P0 verifies it compiles) |
| `world/`, `image_store/`, `tests/`, `ffi_gen/` | this repo | none |
| `src/memory/reservation.rs` | **WINVM** (adds `#[cfg(windows)]` VirtualAlloc module behind the same `Reservation` API) | take WINVM's cfg additions |
| `src/runtime/probe.rs` stack-bounds read | WINVM (this repo's uses `pthread_get_stackaddr_np`/`pthread_get_stacksize_np`, probe.rs:662) | take WINVM's Windows path |
| `build.rs` | WINVM (gates the objc-shim compile to macOS) | take verbatim |
| `src/runtime/winkb.rs` + `[target.'cfg(windows)'.dependencies] rusqlite` | WINVM | take; **re-derive its ABI classification for ARM64** (§3.5) |
| `gui/src/shell/{mod,mac,win}.rs` seam | WINVM (M6) | re-extract on current gui code (this repo's `gui/` has NO shell seam and is newer than WINVM's — Monitor tab, debugger panes) |
| Cocoa bridge, `cocoa_gui/`, `abc_player/`, MacGamePane, AppleScript | this repo, **gated out** on Windows | WINVM's clean-fail stub pattern |
| JIT loader (`native_*`), W^X + icache | **NEW** — `native_winarm64.rs` (§3.1) | ~250 LoC, shaped like both siblings |
| Trap layer (VEH for `brk`) | **NEW** glue over existing pieces (§3.2) | VEH shape from WINVM + `decode_deopt_brk` from this repo |
| Non-unwinding setjmp/longjmp | **NEW** AArch64 `global_asm!` twin (§3.3) | ~60 instructions |

**Source-of-truth rule (binding).** WINVM was seeded 2026-07-21 and its copies
of shared files are **stale** (this repo carries the Aug 2026 regalloc/codegen
gains). Never overwrite a shared file with WINVM's copy — port WINVM's
**Windows-specific additions** over as patches onto current files. When a
WINVM file is Windows-only (`winkb.rs`, `shell/win.rs`, `native_windows.rs`
as a template), taking it wholesale is fine.

## 3. The five seams

### 3.1 W^X + icache — the one MANDATORY new piece

Today: `codecache/guard.rs` (`JitWriteGuard`, the sole write path into JIT
memory) imports `jit_write_protect` / `icache_invalidate` from
`vendor/wfasm/native_macos.rs`, which is `#![cfg(target_os = "macos")]`:
one `mmap(MAP_JIT)` RWX region, per-thread `pthread_jit_write_protect_np`
toggle, `sys_icache_invalidate`, `PAGE = 0x4000` (16 KiB).

Windows ARM64 (`native_winarm64.rs`, new, mirroring `WinJit`):

- One `VirtualAlloc` region, `PAGE_EXECUTE_READWRITE`, held RWX for its
  lifetime (no `MAP_JIT`, no per-thread toggle exists). `jit_write_protect`
  becomes a **no-op** — exactly as WINVM's x64 loader does it, with the same
  documented option of RW↔RX `VirtualProtect` hygiene later, since guard.rs
  call sites already bracket writes correctly.
- `icache_invalidate` = `FlushInstructionCache(GetCurrentProcess(), ptr, len)`
  **plus an `isb sy` on the calling thread**. On x64 WINVM calls flush *pro
  forma* (coherent I/D); on ARM64 it is **load-bearing** — split I/D caches,
  same as macOS. `FlushInstructionCache` does the D-clean + I-invalidate
  broadcast (kernel IPIs cover other cores); the local `isb` (inline
  `asm!("isb sy")`) discards this thread's already-fetched instructions
  before it executes freshly written code.
- guard.rs's P9 **Drop order carries unchanged**: exec-mode flip first (a
  no-op here), then invalidate noted ranges. The ordering comment stays true
  and costs nothing.
- Pages are **4 KiB** (allocation granularity 64 KiB). The vendored loader's
  `PAGE = 0x4000` constant and the Apple-16-KiB commit assertion (WINVM gated
  one such test) must not leak into the Windows path — P0 audits `0x4000`.
- A64 relocation patching reuses the **vendored `relocpatch.rs` as-is**
  (Branch26 in place; far targets via `movz/movk x16; br x16` veneers — the
  mechanism `native_macos.rs` already uses). WINVM's near-host
  `VirtualAlloc2` placement is a nice-to-have here, not a need: A64 always
  had the veneer path, `bl` reaches ±128 MB, and the code cache is one
  region. Skip it in v1; note it as a perf option.

### 3.2 Traps — VEH decodes A64 `brk`, and it is SIMPLER than x64

Both halves already exist:

- **This repo** owns the trap-site codec: `BRK_BASE = 0xD420_0000`, imm16 at
  bits 5–20, the claimed `0xDE00..=0xDE02` namespace
  (`TRAP_UNCOMMON`/`TRAP_STRESS`/`TRAP_ASSERT`), and `decode_deopt_brk`
  (`codecache/deopt_trap.rs`) which rejects every foreign `brk` — including
  Rust's `abort()` (`brk #1`) and Windows' own `__debugbreak`
  (`brk #0xF000`). **Reused verbatim.**
- **WINVM** owns the handler shape: `AddVectoredExceptionHandler`,
  `STATUS_BREAKPOINT` (0x8000_0003) classify → redirect → resume; foreign
  breakpoints pass through as `EXCEPTION_CONTINUE_SEARCH` (no `SIG_DFL`
  restore dance); a `handle_win_fault` branch that redirects a genuinely
  foreign access violation on a thread with a recovery slot into the
  longjmp (context rewrite + `EXCEPTION_CONTINUE_EXECUTION`);
  `STATUS_STACK_OVERFLOW` deliberately excluded (guard page already
  consumed; respawn is the right response).

New glue: the ARM64 `CONTEXT` (`ARM64_NT_CONTEXT`: `Cpsr`, `X0..X28`, `Fp`,
`Lr`, `Sp`, `Pc`, `V[32]`, `Fpcr`, `Fpsr` — declared by hand, `extern
"system"`, same discipline as WINVM's win structs). Read the u32 **at `Pc`**
(the kernel reports Pc AT the `brk`, and `brk` does not auto-advance — no x64
"imm lives at Rip+1" rewind subtlety), run `decode_deopt_brk`, stash the trap
pc in **`X16`** (the same register the macOS handler uses — the stub
trampolines expect it there: `mov lr, x16` / `mov x1, x16` in
`build_uncommon_trampoline` et al.), set `Pc` to the owning cache's trampoline
via the existing per-cache registry, continue execution. The macOS Mach/signal
layer gets `#[cfg(target_os = "macos")]` exactly as WINVM gated it.

> **Δ (2026-08-09) — MEASURED ON THIS HOST, and it corrects this section.**
> An earlier draft said the handler keys on `STATUS_BREAKPOINT`
> (0x8000_0003), by analogy with the x64 port's `int3`. **That is wrong for
> AArch64 Windows**, and a P2 built on it would register a handler that never
> fires. A standalone probe (`aarch64-pc-windows-msvc`, `rustc -O`) gives:
>
> | instruction | exception code |
> |---|---|
> | `brk #0xDE00` / `#0xDE01` / `#0xDE02` — MACVM's whole claimed namespace | **0xC000001D `STATUS_ILLEGAL_INSTRUCTION`** |
> | `brk #1` | 0xC000001D |
> | `udf #0` (an all-zero word) | 0xC000001D |
> | `brk #0xF000` (`__debugbreak`) | 0x8000_0003 `STATUS_BREAKPOINT` |
> | `std::process::abort()`, `brk #0xF003` | **never dispatched to a VEH at all** (`__fastfail`) |
>
> Three consequences for P2:
>
> 1. **Register for `STATUS_ILLEGAL_INSTRUCTION`.** Only Microsoft's own
>    `0xF000` immediate produces `STATUS_BREAKPOINT`; every trap this VM
>    emits arrives as 0xC000001D.
> 2. **The status code cannot discriminate.** A genuine `udf`, a jump into
>    zeroed code, and one of our `brk`s are indistinguishable by code alone —
>    so the handler *must* still read the word at `Pc` and run
>    `decode_deopt_brk`. That function is unchanged and is exactly the right
>    discriminator; only the status keyed on was wrong. This also means a
>    wild jump into unfilled code presents identically to a real trap, which
>    is worth knowing before debugging one.
> 3. **Rust's `abort()` is NOT `brk #1` here** — it is `__fastfail`, which
>    bypasses vectored handlers entirely. So it can never collide with our
>    namespace and P2 need not defend against it. (`deopt_trap.rs`'s
>    `siglongjmp` doc still claims otherwise; correct it there too.)
>
> The rest of the design validated end to end in the same probe: a VEH
> reached on 0xC000001D finds `ARM64_NT_CONTEXT.Pc` at offset **0x108**, sees
> `ExceptionAddress == Pc == the brk's own address` (confirming the
> "no rewind" claim above), decodes `word@Pc == 0xD43BC000 | (imm << 5)`
> exactly as `deopt_trap::brk_word` produces it, writes `X[16]` at offset
> **0x88**, sets `Pc`, and returns `EXCEPTION_CONTINUE_EXECUTION` — resuming
> cleanly for all three immediates. One caution: **x16 is IP0 and any
> intervening call or linker veneer clobbers it**, so the redirect must go
> straight from handler to trampoline with nothing in between (the design
> above already does; do not add a helper call there).
>
> Minimal repro for P2, no test harness involved — smi overflow in a warmed
> method is the shortest path to an uncommon trap:
> ```
> Object subclass: DeoptRepro [ DeoptRepro class >> f: n [ ^n + n ] ]
> "warm f: three times, then:"
> Transcript show: (DeoptRepro f: SmallInteger maxVal) printString.
> ```
> `MACVM_JIT=off` prints and exits 0; `MACVM_JIT=threshold=2` dies at once
> with 0xC000001D.

**Probe-first discipline (binding, learned twice in WINVM):** before wiring
the VEH into the VM, validate the raw mechanism in an isolated test — emitted
`brk 0xDE00` → VEH → capture trampoline → resume — exactly like WINVM's
`veh_redirect_smoke`. Same for §3.3's setjmp. Fault mechanisms are validated
in a scratchpad before they touch the VM.

### 3.3 Non-unwinding setjmp/longjmp — the AArch64 twin

The standing project rule: **never unwind through JIT frames.** Guest-fatal
recovery (a DNU, `self error:`, an FFI fault) longjmps from arbitrarily deep
interpreter+JIT frames back to `eval`; JIT frames carry no unwind info, so
anything that unwinds is UB. On Windows the CRT `longjmp` unwinds (on ARM64
too — mandatory unwind metadata), and `catch_unwind` is out for the same
reason. WINVM solved this with hand-written x64 `global_asm!`
(`winvm_setjmp`/`winvm_longjmp`, `deopt_trap.rs:254`): save the callee-saved
set + the **caller's** return address and post-return SP, then on longjmp
restore and `jmp` — zero `Drop`s run, and `restore_after_guest_fatal` resets
VM state afterward.

The AArch64 twin saves: **x19–x28, fp (x29), lr (x30), sp, d8–d15** (the
Windows-ARM64 nonvolatile set — identical to AAPCS64/Apple; only the low 64
bits of v8–v15 are preserved, which is exactly the `d` registers). Return
path: reload the set, `mov sp, …`, return-twice via `ret` with x0 = the
longjmp value. Two WINVM findings carry as warnings:

1. **`RtlCaptureContext`/`RtlRestoreContext` are subtly wrong** — they
   capture the helper's own (about-to-be-dead) frame; setjmp must capture the
   *caller's* continuation, which is why real implementations are asm.
2. The **foreign-fault path reuses the same asm restore** (VEH context
   rewrite into `winvm_longjmp(buf, 1)`) rather than duplicating the buffer
   layout in the handler.

PAC is off (this repo's own stubs already note it), so no `paciasp` pairing
issues; lr is saved/restored raw.

### 3.4 ABI deltas: Apple AAPCS64 → MS AAPCS64 (small, mostly already satisfied)

| Delta | Impact here | Status |
|---|---|---|
| **x18** is the TEB pointer, never touch | none — Apple reserves x18 too; `regalloc.rs:863` excludes it and `emit.rs` has a standing test asserting x18 never appears in emitted code | already satisfied; reword the "Darwin platform register" comment, keep the assert |
| **Variadic calls**: MS passes variadics in x0–x7 (floats in GPRs!); Apple passes them on the stack | FFI trampolines only — compiled Smalltalk never makes a variadic call. The only variadic-aware code today is the objc bridge (mac-gated) | P5 audit item for `winkb`-driven trampolines; rare in Win32 (wsprintf-class) |
| **Struct returns ≤16 B in x0:x1 on BOTH** | the x64 `rt_poll` hidden-pointer hazard class **does not exist** | keep WINVM's pinning-test pattern anyway (cheap, proves the assumption) |
| **No red zone** on Windows | none — grep confirms the emitter never relied on one (Apple's 128-byte zone unused) | verified 2026-08-09 |
| **Stack probes** required when a frame exceeds a page (4 KiB) | JIT frames are small (spill slots + RootSpill); the call stub and FFI frames are fixed-size | P3 audit: `debug_assert!(frame_bytes < 4096)` at emit time; emit an explicit probe loop only if ever exceeded |
| **Pages 4 KiB** (vs Apple 16 KiB), granularity 64 KiB | reservation + loader + any commit assertions | WINVM's reservation.rs already queries `GetSystemInfo`; P0 audits `0x4000` |
| Callee-saved set, frame layout, FP chains | identical (x19–x28, d8–d15, x29 chain) | none — stack walking, oop maps, deopt all carry |

### 3.5 cfg untangling + FFI

MACVM is deliberately mono-platform — outside `vendor/wfasm` there is not a
single `cfg(target_os = "macos")` in `src/`; POSIX/Mach/objc calls sit
ungated in `deopt_trap.rs`, `probe.rs`, `reservation.rs`, `alien.rs`, the
objc bridge, and `gui/`. The port introduces the discipline WINVM proved:
gate by **capability seam, not by sprinkling** — each OS-coupled file gets
one `#[cfg]`'d module or a sibling file selected in `mod.rs`, with clean-fail
Windows stubs for mac-only guest features (Cocoa prims guest-fatal cleanly).
Where WINVM conflated "not macOS ⇒ x86-64", the gates here must say what
they mean: arch gates (`target_arch = "aarch64"`) select A64 emitters — they
are **already correct** and true on this target — and OS gates
(`target_os`/`windows`) select the OS layer. `codecache/stubs.rs`'s
arch-dispatched `install` (a WINVM addition) picks the A64 builders
automatically here.

FFI: the `winkb` resolver (`windows_api.db`: ~18 k functions, ~46 k COM
methods with vtable indices, ~97 k constants) replaces `cocoa_data` as the
data source, taken from WINVM as-is — but its ABI *classifier* was modelled
on Win-x64 rules (refuses struct-by-value > 8 B: "hidden pointer"). ARM64
rules differ: ≤ 8 B in one reg, 9–16 B in two regs, **HFAs (2–4 floats) in
v0–v3**, > 16 B by pointer. P5 re-derives the classifier for ARM64, keeping
the module's stance: **refuse exactly what cannot be modelled, never guess**
— a misclassified float never faults, it silently passes garbage.
`Time class>>now` wall-clock arrives via `GetSystemTimeAsFileTime` through
this path (the VM's `millisecondClock` primitive is `Instant`-based and
portable — not a gap).

## 4. Repository strategy

House pattern (WF65→WF66, MACVM→WINVM): **WINARM is a new repo seeded from
current MACVM** (done — this checkout), MACVM kept as `upstream` so
portable-layer fixes cherry-pick both ways. WINVM is a **reference, not a
remote** — its contributions arrive as patches (§2's source-of-truth rule).
Module names/layout stay identical; the crate keeps the name `macvm` and the
`MACVM_*` env flags (WINVM kept both; every doc and justfile recipe keeps
working). `.gitattributes` pins LF from day one (WINVM lesson: scripted
edits stay byte-clean). `cocoa_gui`, `abc_player`, and the MacGamePane path
deps leave the workspace on Windows (WINVM's manifest shows the shape;
`gamepane` stays an off-by-default feature).

## 5. Sprint ladder

The port runs as **Phase P** in [`docs/SPRINTS.md`](docs/SPRINTS.md), with
per-sprint detail + test docs in [`docs/sprints/`](docs/sprints/) following
[`CONVENTIONS.md`](docs/sprints/CONVENTIONS.md):

| Sprint | Size | Scope | New code? |
|---|---|---|---|
| P0 toolchain + seed + interpreter-only | `M` | rustup/VS-ARM64 verified; OS seams gated; world boots; full suite interpreted | patches only |
| P1 JIT substrate | `M` | `native_winarm64.rs` loader, real icache flush, guard.rs wiring, A64 relocs | ~250 LoC |
| P2 trap layer | `L` | VEH for `brk`, AArch64 setjmp/longjmp, foreign-AV recovery, PROBE on ARM64 | the risky ~400 LoC |
| P3 tier-1 alive | `M` | full JIT + all stress gates + differential vs Mac; page/probe audits | audits |
| P4 GUI shell | `M` | shell seam re-extracted on current gui; WebView2 (ARM64-native runtime) | seam refactor |
| P5 FFI + world | `L` | winkb ARM64 classifier, trampolines, wall-clock, un-gate world FFI files | classifier |

Dependencies: P1→P2→P3 strictly; P4 needs P2 (GUI without DNU recovery dies
on the first Workspace typo — WINVM learned this in production); P5 needs
P1+P2. P0 blocks everything.

**The behavioral oracle is the Mac build of the SAME checkout** — same
world, same bytecode, same tests; outputs must match. Where WINVM diffed
against a cousin, WINARM diffs against its own twin.

## 6. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Toolchain gap: no rustc/cargo installed; VS 18's ARM64 C++ tools + SDK unverified | blocks P0 | P0 step 1 installs `rustup` (host `aarch64-pc-windows-msvc`) and proves link.exe/SDK with a hello-world + a `cc`-crate build (rusqlite `bundled` needs the C toolchain too) |
| icache flush subtly wrong (missing local `isb`, flush-before-write ordering) | rare, unreproducible crashes | keep guard.rs P9 order; patch-and-rerun tests from S9's gate re-run on target; stress = the existing suite at `threshold=1` |
| `brk` exception delivery differs from assumption (Pc placement, status code) | P2 rework | probe-first discipline (§3.2): raw mechanism validated in isolation before VM wiring |
| Hand-written AArch64 setjmp wrong in a corner (d-regs, sp restore) | silent corruption on recovery path | WINVM's isolated probe suite ports: return-twice, 60-frame jump with zero `Drop`s, int+FP integrity |
| WebView2 ARM64 runtime absent/x64-emulated | GUI phase | Win11-ARM ships a native ARM64 Evergreen runtime; P4 asserts the process arch at startup |
| `windows`/`webview2-com` crate pair on aarch64 msvc | GUI build | both publish aarch64-pc-windows-msvc support; pinned pair (0.62/0.39) carried from WINVM |
| winkb x64-modelled ABI classes mislead ARM64 trampolines | silent FFI garbage | §3.5: classifier re-derived; refuses HFA/variadic until modelled; pinning tests call real `extern "C"` fns |
| Emulated-x64 confusion (running x64 binaries transparently) | misleading benchmarks/tests | P0 gate asserts `target_arch == "aarch64"` at runtime and records it in `-v` output |

## 7. What deliberately does not change

- Bytecode ISA, `.mst` format, world sources, SQLite image store — identical;
  worlds and tests shared verbatim with MACVM.
- The A64 compiler, register model, patch-site shapes, oop maps, deopt
  metadata — identical (the whole point of this port).
- The no-`become:` rebuild-from-source philosophy; sub-second world rebuild
  stays the compatibility guarantee.
- Env flags (`MACVM_*`), crate name, module layout, justfile CI contract
  (gates extend as `gate-p00`…).
- The web GUI assets: WINVM proved the HTML/CSS/JS environment ports with
  **zero edits**; only the shell changes.

## 8. Status log

- **2026-08-09 — Design.** This document + Phase P sprint ladder
  (`docs/SPRINTS.md`, `docs/sprints/sprint_p0*_detail.md`, `tests_p0*.md`)
  written against verified source facts in both reference checkouts (file
  cites throughout).
- **2026-08-09 — P0 done. Interpreted Smalltalk runs natively on Windows
  ARM64: 992 passed, 0 failed, 65 ignored.** Toolchain was already present
  (rustc 1.97.1, host `aarch64-pc-windows-msvc`, VS 18 Professional); proved
  native rather than emulated by checking the PE machine type of a built
  binary (`AA64`), which the startup banner now also reports.
  - Build went 59 errors in 9 files → **0 errors, 0 warnings, all targets**.
    Nearly all of it one cause: on Windows `libc` exposes CRT functions only,
    so every seam is hand-declared `extern "system"` — no new dependency.
  - **The port thesis holds, and it is measured, not argued.** Compiled
    AArch64 executes correctly here: tier-1 tests compile and run real
    methods, `it_bench_smoke` requires compiled ≥ 2× interpreter and passes,
    and the FFI trampolines call real kernel32/CRT functions. The loader, the
    code region, relocation and the icache discipline are all sound. **Every
    one of the 65 gated tests is a trap, a recovery point, or an unresolvable
    POSIX symbol — not one is a codegen defect.**
  - Two plan corrections, both now folded in above: §3.1's stub pair (P0
    cannot defer the code region — stubs publish at genesis "regardless of
    `options.jit`"), and §3.2's exception code (the Δ above — the single most
    valuable thing this sprint produced for P2).
  - New: `Smalltalk platformName` (prim 266) and `wallClockMilliseconds`
    (267). The world asks the platform instead of assuming, so `world/*.mst`
    stays shared with MACVM verbatim rather than forked the way WINVM had to.
  - **Found in MACVM mainline, not caused by the port** — worth cherry-picking
    back: `tests/golden/s10_absDiff.lst.expected` is stale since 552831a
    (2026-08-08) and fails in any debug `cargo test`, on macOS too; `ir.rs`
    listed `RefCmpBr` in a no-op match arm it already handled 50 lines
    earlier; `emit.rs` cast a fn item straight to an integer. And the clone's
    `core.autocrlf=true` had rewritten `world/04_transcript.mst`'s newline
    literal to CRLF, so `Transcript cr` emitted CRLF — a checkout defect that
    changed guest-visible VM behaviour.
  - **Open, and not a port problem:** `cargo fmt --check` and `clippy -D
    warnings` fail under rustc 1.97.1 on files the port never touched
    (`src/types/`, `image_store/`, `src/oops/`), so `just ci` — and therefore
    the literal `gate-p00` — cannot pass without either reformatting ~43 files
    (a large, permanent divergence from MACVM that would fight every
    cherry-pick) or pinning a toolchain. Left as a decision, deliberately.
