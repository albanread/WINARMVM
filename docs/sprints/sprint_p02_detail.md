# Sprint P2 — Trap layer: VEH for `brk`, AArch64 setjmp/longjmp, recovery

Objective: the deopt/assert trap path and guest-fatal recovery work on
Windows ARM64 — a `brk #0xDExx` in compiled code round-trips through a
Vectored Exception Handler back into the VM's existing trampolines, and a
DNU/`error:`/foreign-fault recovers via a hand-written **non-unwinding**
setjmp/longjmp instead of killing the process. Implements MIGRATION.md §3.2
+ §3.3. This is the port's highest-risk sprint: all of it is
signal-handler-adjacent code where "compiles and usually works" is the
failure mode. WINVM's M2 + guest-fatal-recovery entries are the worked
example; its two "validate the mechanism in isolation first" findings are
binding here.

## Prerequisites

- P1 green: executable A64 code region, publish/patch, real icache flush.
- Existing, unchanged, REUSED (not rewritten): `decode_deopt_brk` +
  `brk_word` + the `0xDE00..=0xDE02` namespace (`codecache/deopt_trap.rs`);
  the per-cache trampoline registry (same file — "the range that contains
  the pc names the cache, and that cache's own trampoline is guaranteed
  live"); the A64 trampoline builders (`build_uncommon_trampoline` etc.)
  whose contract is **trap pc arrives in x16**; PROBE's dossier logic
  (`runtime/probe.rs`) minus its context-capture entry.
- Reference: `WINVM/src/codecache/deopt_trap.rs` — `veh_trap_handler`,
  `handle_win_fault`, `winvm_setjmp`/`winvm_longjmp` (global_asm at ~:254),
  the jmp-buf slot machinery (~:1552–1580), `capture_regs_win`.

## Deliverables

- `ARM64_NT_CONTEXT` + VEH declarations, by hand (`extern "system"`), in
  the Windows module of `codecache/deopt_trap.rs`.
- `veh_trap_handler` for ARM64 — classify → redirect → resume.
- `winvm_setjmp` / `winvm_longjmp` in AArch64 `global_asm!` (keep the
  symbol names — every call site and WINVM's documentation carry over).
- `handle_win_fault` — foreign-AV recovery through the same longjmp.
- `capture_regs_win` ARM64 variant feeding PROBE; dossier disassembly via
  the existing `disasm_a64.rs` (NOT iced-x86).
- Two isolated probe test suites (D1 discipline) + the VM-wired tests.
- P0's gated signal/`embed::tests` groups un-gated via Windows twins.
- `just gate-p02`.

## Design

### D1. Probe-first discipline (binding)

Both mechanisms are validated **in isolation before anything in the VM
calls them** — WINVM did exactly this and caught `RtlCaptureContext` being
unusable *before* it touched the VM:

- **Trap probe**: hand-place `brk #0xDE00` + a capture trampoline in a
  private region (P1 machinery), arm the VEH, call, assert: trampoline ran,
  x16 held the brk's pc, execution resumed. This is WINVM's
  `veh_redirect_smoke`, A64 flavor.
- **Jump probe**: return-twice semantics; a ~60-frame-deep longjmp with
  **zero `Drop`s run** (drop-counter type); callee-saved GPR + d8–d15
  integrity across the jump (plant sentinels, verify after second return).

### D2. The VEH — classify → redirect → resume

Registration: `AddVectoredExceptionHandler(1, handler)` once at VM init
(WINVM's arrangement, first-handler position). Handler algorithm:

1. `code = ExceptionRecord.ExceptionCode`. Interested in exactly
   `STATUS_BREAKPOINT` (0x8000_0003) here and the fault set in D4;
   everything else → `EXCEPTION_CONTINUE_SEARCH` immediately.
2. `pc = Context.Pc`. On ARM64 the kernel reports **Pc AT the `brk`
   instruction** and `brk` does not auto-advance — there is no x64
   "imm at Rip+1" rewind arithmetic. The imm is **in the instruction
   word**: read `*(pc as *const u32)` — but ONLY after a code-cache range
   check (the registry lookup doubles as it): a breakpoint outside any
   registered cache is foreign; never dereference an arbitrary pc.
3. `decode_deopt_brk(word)` — reused verbatim. `None` → CONTINUE_SEARCH:
   this transparently passes through Rust's `abort()` (`brk #1`), Windows'
   `__debugbreak` (`brk #0xF000`), and a debugger's own breakpoints — the
   x64 side got this "for free" and so do we, by imm-range refusal.
4. `0xDE02` (`TRAP_ASSERT`) → PROBE: capture the ARM64 context
   (`capture_regs_win`), emit the dossier, terminate — mirroring the macOS
   fatal branch.
5. `0xDE00`/`0xDE01` → **redirect**: `Context.X16 = pc` (the stash — the
   A64 trampolines' documented input register, `mov lr, x16` /
   `mov x1, x16` in their bodies); `Context.Pc = trampoline` for the
   owning cache (registry lookup from step 2); return
   `EXCEPTION_CONTINUE_EXECUTION`. **Only Pc and X16 are written**; the
   trampoline itself saves/uses everything else — the handler rewrites the
   minimum, exactly like both siblings.

`ARM64_NT_CONTEXT` is declared by hand with the fields this file touches
named and the rest reserved-padded (`ContextFlags`, `Cpsr`, `X[31]` as
x0–x28+Fp+Lr, `Sp`, `Pc`, `V[32]` 128-bit, `Fpcr`, `Fpsr`, debug regs
tail). Layout source: SDK `winnt.h`; add a `const _:` size assert
(`size_of::<ARM64_NT_CONTEXT>() == 0x390`) so a mis-declared struct fails
at compile time, not as register soup.

### D3. `winvm_setjmp` / `winvm_longjmp` — AArch64 `global_asm!`

Same names, same contract as WINVM x64 (its long doc comment carries over
nearly verbatim — it documents WHY, which is ISA-independent):

- **Buffer layout** (one `#[repr(C)]` struct, single source of truth used
  by asm offsets via `const` — no magic numbers): x19–x28 (10), fp, lr —
  as the **caller's return address**, see below — sp (post-return), d8–d15
  (8) = 21 × 8 bytes; round to 176 for 16-alignment.
- `winvm_setjmp(buf)`: stores x19–x28, fp; stores **lr** (which IS the
  caller's continuation pc — A64 keeps it in a register, so the x64
  subtlety "load the return address off the stack" becomes a plain reg
  store); stores the CALLER's sp (i.e. current sp — nothing pushed);
  d8–d15; returns 0.
- `winvm_longjmp(buf, val)`: reloads all of the above, `mov sp, <saved>`,
  `mov x0, val` (forced nonzero), `br <saved lr>` — returning "from"
  setjmp a second time. No frame is built; nothing between the two frames
  is touched; zero `Drop`s run — that is the point (the standing rule:
  never unwind through JIT frames; ARM64-Windows CRT `longjmp` unwinds,
  `catch_unwind` likewise — both remain forbidden).
- **PAC is off** in this project (existing stub docs note it): lr is
  stored/branched raw; no `paciasp`/`autiasp` pairing to preserve. If PAC
  ever turns on, this file is the first casualty — leave that sentence in
  the asm's header.
- The **`RtlCaptureContext`/`RtlRestoreContext` anti-pattern note carries
  verbatim** from WINVM: they capture the helper's own dead frame; setjmp
  must capture the caller's continuation; that is why real implementations
  are asm, including this one. Do not re-attempt.

Rust surface: same `extern "C"` fn pair + per-thread recovery-slot
machinery (`jmp_buf_ptr(i)`) as WINVM — that layer is OS-flavored, not
ISA-flavored, and ports as a patch.

### D4. `handle_win_fault` — foreign-AV recovery

WINVM's design carries whole: a genuinely foreign access violation (wild
deref in interpreter/FFI code — NOT a pc inside a registered code cache,
where PROBE's dossier is the right response) on a thread holding a
recovery slot is redirected by context rewrite into
`winvm_longjmp(slot, 1)` + `EXCEPTION_CONTINUE_EXECUTION` — reusing the
asm restore rather than duplicating the buffer layout in the handler
(WINVM's own stated reason). On ARM64 the rewrite is:
`Context.X0 = slot; Context.X1 = 1; Context.Pc = winvm_longjmp` (first two
argument registers instead of RCX/RDX). `STATUS_STACK_OVERFLOW` stays
**excluded** — the guard page is consumed by then; respawn is the right
response (WINVM's documented, deliberate divergence; keep its comment).

### D5. PROBE on ARM64

`capture_regs_win` reads the ARM64 context into PROBE's register dossier
(x0–x28, fp, lr, sp, pc, Cpsr; d-regs optional in v1 — the dossier's
annotated-register pass is GPR-driven). The disassembly window around the
faulting pc uses the existing `disasm_a64.rs` — delete nothing, add an
entry point if its current one is macOS-context-shaped. The dossier's
range-validation discipline ("the VM is the SUSPECT — every raw pointer
range-checked before deref") is untouched and already OS-neutral.

## Implementation order

1. D3 jump probe first (no VM coupling at all): asm + isolated tests.
   **Nothing proceeds until return-twice / 60-frame / sentinel tests pass.**
2. D2 trap probe: context struct + size assert, VEH registration, the
   capture-trampoline smoke — isolated, still no VM coupling.
3. Wire the VEH classify/redirect against the real per-cache registry +
   real trampolines; the existing macOS-side trap tests get Windows twins
   (same scenarios, VEH instead of SIGTRAP).
4. Port the recovery-slot layer + `raise_guest_fatal` path onto the new
   longjmp; un-gate `embed::tests` — the embedded-VmHandle DNU test is the
   flagship (guest error → message out, process lives).
5. D4 foreign-AV branch + its test (a real, controlled AV recovered with
   the faulting address read back).
6. D5 PROBE capture + one forced-`TRAP_ASSERT` dossier test (asserting the
   dossier PRINTS with sane pc/fp, not asserting its full content).
7. `just gate-p02: gate-p01` + the suites above.

## Pitfalls

- **Everything in a VEH runs on the faulting thread at fault depth.** No
  allocation, no locks that the faulting code might hold, no Smalltalk.
  The existing macOS handler already obeys this; keep its shape, don't
  "improve" it with Rust conveniences.
- **The reentrancy guard carries over** (a fault inside the handler/PROBE
  must terminate, not recurse) — WINVM/macOS both have it; wire it before
  the first dossier test, or a bad dossier test hangs the suite.
- **Never read the imm before the range check** (D2 step 2 ordering). A
  foreign breakpoint at an unmapped-adjacent pc must not fault the
  handler.
- **First-position VEH sees debugger breakpoints too.** The imm-range
  refusal handles it, but debugging THIS code under a debugger means the
  debugger's own `brk #0xF000` traffic flows through our handler — keep
  the handler's foreign path allocation-free and fast, and expect
  breakpoint-heavy debug sessions to work (WINVM notes foreign int3
  passes "for free"; same property, keep it true).
- **`global_asm!` on aarch64-msvc**: section/symbol directives differ from
  Apple asm (`.globl` works; no leading-underscore mangling on Windows
  ARM64). The x64 file already navigated MSVC-flavored global_asm — mirror
  its directive set, not the macOS libc source's.
- **d8–d15 means the D views** (low 64 bits) — `stp d8, d9, …` not `q8`.
  Saving q-regs would be wrong-not-just-wasteful: the ABI only guarantees
  the low halves, and the buffer layout is contract.
- **Alt-stacks do not exist here** — macOS's `sigaltstack` machinery
  (CG0's per-thread alt-stacks) has no VEH counterpart and none is needed:
  the VEH runs on the faulting thread's (intact) stack, and stack
  overflow — the one case that needs an alternate stack — is excluded by
  D4. Gate the alt-stack code to macOS; do not emulate it.

## Interfaces for later sprints

- P3: with traps + recovery live, the JIT-off guard lifts; deopt paths are
  exercised by the existing stress modes unchanged.
- P4: `raise_guest_fatal` recovery is what makes a Workspace DNU survivable
  — the GUI's hard dependency on this sprint.
- P5: FFI faults land in D4's foreign branch; the FFI trampolines get
  fault coverage for free.

## Out of scope

- Tier-up/threshold runs (P3). OSR, deopt-stress sweeps at scale (P3 runs
  the modes; this sprint proves the mechanisms).
- Any change to trampoline BODIES, the brk namespace, or deopt metadata —
  all pre-existing and ISA-correct already.
- Windows structured unwind info (`RtlAddFunctionTable`) — same posture as
  WINVM: not needed while VEH + FP chains cover traps and GC; revisit only
  if C callbacks must ever unwind through JIT frames (they must not).
