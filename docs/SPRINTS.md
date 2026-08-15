# MACVM Sprint Plan

A series of achievable, individually-testable sprints implementing
[`SPEC.md`](SPEC.md). Every sprint ends **green**: all prior tests still pass,
plus the sprint's own acceptance gate. Section references (§) are into SPEC.md.

**Per-sprint implementation guidance lives in [`sprints/`](sprints/)**: each
sprint has a `sprint_sNN_detail.md` (design advice for the implementing agent)
and a `tests_sNN.md` (test plan); [`sprints/CONVENTIONS.md`](sprints/CONVENTIONS.md)
binds naming, layout, and templates. SPEC.md §15 logs the amendments that came
out of writing them.

Phases: **A** object world & interpreter (S0–S6) → **B** garbage collection
(S7–S8) → **C** native code substrate (S9–S11) → **D** adaptive optimization
(S12–S15) → **E** stretch (S16+).

Sizing: S = a focused day or two, M = up to a week, L = 1–2 weeks of part-time
research pace. Order is dependency-driven; A and the JASM spike (S9) can
overlap if desired.

### S7.5 — Handle hardening (generational, validated, currency-level) `M`
Goal: close a **structural** flaw in the Handle/GC contract before S8 builds
more moving-GC machinery on top of it. Discovered S7-11 (2026-07-02): the
`Handle<T>` bug class (a bare oop-wrapper held across an allocation) was being
independently reintroduced across S7-9/S7-10/S7-11 — including inside fixes for
earlier instances and in the GC's own unit tests — because the *unsafe* type
(`Oop`/`KlassOop`/…) is the pervasive currency and `Handle<T>` an opt-in,
unvalidated, function-internal convenience (one `pub fn` signature total). No
amount of care fixes a bug class the API shape regenerates. Design of record:
**SPEC §7.6.1**, adopting the **Locus** sister VM's proven handle layer.
Which change fixes what (adversarial review, 2026-07-02 — do not repeat the
first draft's overclaim): the bugs actually hit were **bare oops, not misused
`Handle`s**, so **change 2 is the primary fix**; change 1 is a soundness
backstop + the prerequisite for change 2's type-safety, and catches only
handle-use-after-scope.
- Change 1 (do first — isolated to `memory/handles.rs`): persistent `gen`
  vector + `generation` field + **three-guard access** (in-range, not-vacated,
  gen-match) + **bump `gen` on vacate** (scope drop), not only on re-push —
  MACVM has no free-list/tombstone like Locus, so vacate-bump is what closes
  the false-negative. Debug-gated assert (handles are on the send hot path).
- Regression test (before change 2): reproduce the real failure — a helper that
  returns a bare oop past its scope no longer compiles once reshaped; a
  deterministic `stale_handle_panics`. A21's premise is that tests+stress alone
  were insufficient, so pin the fixed behavior with a test.
- Change 2 (the invasive part): reshape allocating functions
  (`install_method`, `MethodDictOop::insert`, `alloc_*`, `BytecodeBuilder`
  public surface) to take/return `Handle<T>`; ripple through **every** call
  site + tests. Ship an ergonomic klass-handle minting helper or the reshape
  gets resisted the way opt-in `Handle` was. **Reshape leaf allocators last**
  (they're called by `handles.rs`'s own machinery — circular otherwise).
- Change 4: primitives are a third, currently-unprotected surface
  (`prim_basic_new` holds `args[0]` across `alloc_slots`, safe only by the
  accident that klasses are old-gen). Bring them into the contract (hand
  `can_allocate` primitives handles, or enforce `prim_arg` re-read + lint).
- Explicitly NOT in scope: Locus's `newgc-core` collector (rejected,
  ref-analysis §5) and its conservative-scan MAGIC tag (MACVM uses precise
  oop-maps). See §7.6.1's "deliberately NOT imported."
- **Gate:** full suite green under `MACVM_GC_STRESS=1` *to completion* (the two
  hang-forever tests fixed in S7-11 were why this stress run had never actually
  finished — the precondition for trusting any GC gate); no `pub fn` that
  allocates internally takes/returns a bare oop-wrapper for a value held across
  the allocation; no `can_allocate` primitive reads its `args` copy after an
  alloc; `stale_handle_panics` + the bare-return-won't-compile test both
  present. Full reasoning: SPEC §7.6.1 + A21.

---

## Phase A — object world & interpreter

### S0 — Skeleton, tags, mark word `S`
Goal: the value representation, pinned and tested, plus CI habits.
- `Oop` + typed wrappers (§2.1, §2.5); mark-word pack/unpack (§2.2); smi
  arithmetic helpers with overflow detection.
- `cargo test` + `cargo clippy` clean; a `justfile`/script for test+stress runs.
- **Gate:** unit tests — tag round-trips, smi min/max/overflow edges, mark-word
  field isolation (set each field, assert others unchanged), forwarding-bit
  discrimination.

### S1 — Heap arena, allocation, genesis `M`
Goal: allocate real objects; the metaobject knot exists.
- Address-space reservation + eden bump allocator (no GC — abort when full)
  (§7.1–7.2); object formats & instance creation (§2.3); klass objects (§2.4);
  `Universe::genesis()` (§3.2 step 1); symbol table with interning; identity
  hash assignment; a debug object printer (`print_oop`).
- **Gate:** unit tests — genesis invariants (`nil.klass.name == #UndefinedObject`,
  metaclass chain closes: `Object class class == Metaclass`), symbol interning
  identity, indexable alloc + at/put via Rust API, alignment & size math.

### S2 — Bytecode + interpreter core (no sends) `M`
Goal: execute hand-assembled straight-line bytecode.
- Bytecode definition + a Rust `BytecodeBuilder` (test-only assembler);
  CompiledMethod objects (§4.4); disassembler; interpreter loop + frame layout
  (§5.1–5.2) for opcodes 00–12, 30–33, 40–41 (no sends, no closures).
- **Gate:** golden tests — disassembler output for builder-built methods;
  execute arithmetic/temp/jump kernels, assert result oop + final stack shape;
  stack-discipline asserts (frame teardown leaves caller's stack exact).

### S3 — Sends, ICs, primitives `L`
Goal: real message dispatch end-to-end.
- MethodDictionary + hierarchy lookup + lookup cache (§6.1); send/return
  protocol + IC side tables with full state lattice (§4.3, §5.3); DNU with
  `Message`; primitive mechanism + smi/oops/ByteArray groups (§10); invocation
  counters (counting only — no trigger yet).
- **Gate:** unit tests driving IC transitions explicitly (empty→mono→poly→mega,
  guard self-heal after method redefinition §6.2); golden programs: dispatch
  through a 3-class hierarchy, super sends, DNU trace; primitive failure →
  bytecode fallback path.

### S4 — Blocks, closures, NLR, ensure `L`
Goal: the hard Smalltalk semantics, correct before performance ever matters.
- CompiledBlock, `push_closure`, Contexts + captured-temp slots (§5.4); block
  `value` primitives; NLR with unwind + dead-home detection; `ensure:` /
  `ifCurtailed:` markers; `mustBeBoolean`.
- **Gate:** golden programs — counter closure (shared mutable capture), nested
  blocks 3 deep with ctx-temp access, NLR through 2 frames running `ensure:`
  blocks in order, `cannotReturn:` on escaped block, block re-entry after home
  return (non-NLR use is legal).

### S5 — Source compiler `L`
Goal: `.mst` in, running code out; hand-assembly retires.
- Lexer/parser (full expression grammar incl. cascades, literal arrays,
  pragmas-ignored) → AST → codegen (§4.5): name resolution, capture analysis,
  inlined control selectors, literal frames, IC tables; class-definition brace
  syntax + world.list loader (§1.2, §3.2 step 2); REPL + script runner
  (`macvm run foo.mst`).
- **Gate:** golden AST→bytecode listings for a corpus of methods (each opcode
  exercised); parse-error messages with line/col; end-to-end: `Point.mst`
  program prints expected transcript; every S2–S4 golden program re-expressed
  in source and passing.

### S6 — Core library + in-language test suite `L`
Goal: enough of a class library that the VM tests itself in Smalltalk.
- `world/`: Object, Boolean/True/False, UndefinedObject, Magnitude,
  SmallInteger (+ LargeInteger fallback arithmetic), Double, Character
  (flyweight), String/Symbol, Array/ByteArray, OrderedCollection, Dictionary,
  Association, Interval, WriteStream basics, Transcript (stdout).
- SUnit-lite (§12.3) + first ~200 assertions covering the library and S3–S4
  semantics.
- **Gate:** `cargo test` runs the in-language suite green; fib(25) and sieve
  run with correct answers; interpreter throughput measured & recorded
  (baseline for §13).

## Phase B — garbage collection

### S7 — Scavenger + write barrier `L`
Goal: the young generation collects; allocation is no longer mortal.
- Survivor spaces, Cheney scavenge with forwarding, age + adaptive tenuring
  (§7.3); card table + store barrier through the one store choke point (§7.4);
  old-gen object-start offset tables; Handle/HandleScope discipline retrofit
  over the runtime (§7.6); `MACVM_GC_STRESS=1` mode.
- **Gate:** entire S6 suite green under `MACVM_GC_STRESS=1` (scavenge every
  allocation); unit tests — forwarding, tenuring histogram math, card indexing,
  dirty-card scan finds exactly the recorded old→new refs; allocation-torture
  program (10M short-lived objects) completes in bounded memory.

### S8 — Full GC (mark-compact) `L`
Goal: unbounded-lifetime programs; heap grows and compacts.
- Worklist mark, slide compact, displaced-mark side map, reference rewrite,
  interpreter-stack fixup, lookup-cache flush (§7.5); old-gen segment growth;
  `gcFull` primitive; heap statistics.
- **Gate:** suite green under `MACVM_GC_STRESS=full`; compaction test —
  fragment old gen, full-GC, assert live set intact (checksum object graph
  before/after) and space reclaimed; identity-hash stability across compaction;
  soak: 1-hour churn run with flat memory ceiling.

## Phase C — native code substrate

### S9 — Vendor JASM + code cache spike `M`  *(can start any time after S0)*
Goal: emit and execute arm64 code from MACVM, trust intact.
- Vendor `wfasm` slice (a64 encoder, backend contract, `MacJit`, relocpatch) +
  its frozen 1,181-form corpus test (analysis §4.4); implement the `Assembler`
  trait over `encode::encode` (structured, no text); code cache with stub
  segments (§9); literal-pool emission + Oop reloc records.
- **Gate:** corpus test green in-tree; JIT smoke — emit `(x+1)*2`, an internal
  branch, a call to a Rust `extern "C"` fn, execute all three; write-protect
  toggle + icache-flush round-trip test; literal-pool word patch-and-rerun test.

### S10 — Compile straight-line methods `L`
Goal: tier 1 exists — the simplest methods run native.
- Bytecode→CFG decode + SSA-lite IR + lowering + linear scan + emit (§8.3) for
  send-free methods (arith, instvar/temp access, jumps, return); nmethod format
  + code table (§8.2); interpreter IC dispatches to nmethod ids; invocation-
  counter trigger + synchronous compile (§8.1); `MACVM_JIT=off|threshold=N`.
- **Gate:** differential — suite green with `threshold=1` (everything eligible
  compiles immediately); disasm golden for 3 reference methods; compiled
  arithmetic speed as a **tripwire** (warn < 5×, fail < 2× interpreter — perf
  is not gated before S15, rule 3); frame walk still prints mixed
  compiled/interpreted traces. Includes the nmethod literal-pool `oops_do`
  root hook (first oop-bearing nmethod lands here — SPEC §15 A10).

### S11 — Compiled sends, PICs, patching `L`
Goal: compiled code calls compiled code; the IC story is complete.
- Klass-guard prologue + verified entry (§8.2); mono call sites with
  InlineCache relocs + patch protocol (Branch26/veneer + icache flush); PIC
  stubs; megamorphic lookup stub; compiled→interpreter calls (and back) via
  adapter frames; runtime stubs (§9); allocation fast path in compiled code.
- **Gate:** suite green at `threshold=1`; IC-patch unit tests (mono→PIC→mega on
  live call sites, veneer path forced by far target); mixed-tier call matrix
  test (I→C, C→I, C→C, super, DNU from compiled); dispatch micro as a tripwire
  (warn < 5×, fail < 2× — rule 3). Ships a **temporary GC bridge** (old-direct
  allocation while compiled frames are live) so this gate runs before oop maps
  exist; S12's first commit deletes it (SPEC §15 A13).

### S12 — Moving GC under compiled frames `L`
Goal: the two hard subsystems coexist — the biggest integration risk, retired
before inlining lands.
- Oop maps at safepoints from regalloc oop-ness (§8.3, §8.5); compiled-frame
  stack walking (FP chain + PcDesc lookup); scavenge + full GC with compiled
  frames on stack (registers spilled at safepoints in v1 — no live-register
  maps yet, calls/allocs are the only safepoints); nmethod literal-pool oop
  update in GC; code-cache↔heap invariants.
- **Gate:** suite green with `threshold=1` **and** `MACVM_GC_STRESS=1`
  simultaneously (the flagship gate); unit tests — oop map decode, a compiled
  frame's spill slots relocated correctly across a forced scavenge mid-loop;
  full-GC moves an object referenced from an nmethod literal pool.

## Phase D — adaptive optimization

### S13 — Deoptimization `L`
Goal: the safety net that makes speculation legal.
- Scope descs + PcDescs emission (LEB128) (§8.7); uncommon trap (`brk`) +
  Mach/SIGTRAP handler → frame materialization onto the interpreter stack;
  not_entrant patching + lazy invalidation deopt on return; dependency index
  (klass/selector → nmethods) wired to method redefinition (§8.6);
  `MACVM_DEOPT_STRESS=1`.
- **Gate:** suite green under deopt-stress (every guard fires once; periodic
  invalidation); unit — scope-desc round-trip, materialized frame equals the
  frame the interpreter would have built (checked by executing both paths);
  redefine a method mid-loop and observe correct continuation.

### S14 — Type feedback, inlining, customization `L`
Goal: the actual point of the lineage — optimized sends.
- Feedback read from IC tables/PICs; inlining with budgets + levels;
  customization keyed nmethods; block inlining + Context elision; uncommon
  traps for cold IC states; recompilation policy (caller preference, levels
  1–4, version cap) (§8.1, §8.4).
- **Gate:** suite green under all three stress modes combined; fib/sieve ≥ 10×
  interpreter; `do:`/`inject:into:` loops show Context-elision (assert zero
  Context allocs in steady state via GC stats); recompilation-level ladder
  observable in `-v` logs and capped.

### S15 — OSR + performance hardening `M`
Goal: hot loops enter compiled code without waiting for re-invocation.
- Loop-counter OSR entries + interpreter-frame→compiled-frame conversion
  (§8.7); Richards + DeltaBlue ported to `world/bench/`; profile-guided fixes;
  performance-target table (§13) measured and recorded in `docs/PERF.md`.
- **Gate:** long-running-loop benchmark reaches compiled speed without method
  re-entry; Richards ≥ 5× interpreter; no regression in stress suites.

## Phase G — GUI track (parallel)

The MACVM user interface: the **Strongtalk live-HTML programming environment**
recreated in a native Cocoa window (WKWebView), built as the separate cargo
workspace member `gui/` (`macvm-gui`). Plan of record with visual ground truth
and decisions D-G1…D-G5: [`../gui/PLAN.md`](../gui/PLAN.md); the core-side
contract (VmHandle, TranscriptSink, mirrors, source registry, threading) is
SPEC §16. This track runs **concurrently** with the core phases; its gates
never block core sprints, and core stress gates never require the GUI.

| Phase | Size | Needs from core | Gate (from PLAN.md) |
|---|---|---|---|
| G0 static shell | `M` | nothing | start page + tour render period-faithful; nav + status bar live |
| G1 live-page runtime, stub host | `S` | nothing | doit click → transcript echo, full JS↔Rust round trip |
| G2 VM bridge | `M` | **S5** (eval/doit compile) + **S6** (Transcript, world) + SPEC §16.1–16.2 | original `startPage.html` fully live against MACVM |
| G3 outliner tools | `L` | S6 world + **W2/W3** (mirrors, ToolNode, HtmlWriter — see APPS.md) | browse Object hierarchy → class → category → method source, lazily |
| G4 code editing & find tools | `L` | **W4** (accept path) + source registry (A16); **S13 for JIT-on redefinition** (else `MACVM_JIT=off`) | tour's "little project" workflow end to end |
| G5 polish & parity sweep | `S` | — | every toolbar icon wired or consciously deferred |

Sequencing guidance: G0–G1 can start **immediately** (no VM dependency) and
are a natural parallel workstream during Phases A–B. If the GUI outruns the
core, extend G1's stub host with fixture data (PLAN.md §5) rather than
blocking. The S5/S6 sprint docs carry addenda for the two core-side
obligations this track adds (source registry; TranscriptSink routing).

## Phase CG — Cocoa GUI track (parallel, a second GUI mode)

A **native AppKit** programming environment as a second, flagged mode
(`macvm-cocoa`) alongside the WKWebView shell — the interface built *in
Smalltalk through the Cocoa bridge*, with a UI worker VM pinned to the main
thread (a dumb terminal) and the persistent primary VM on a background
thread. Design of record (adversarially reviewed, 2026-07-17):
[`cocoa_gui_design.md`](cocoa_gui_design.md); per-sprint detail + gates:
[`sprint_cocoa_gui.md`](sprint_cocoa_gui.md). Builds on the Cocoa bridge
C0–C5 (`cocoa_bridge_design.md`) + `perform:withArguments:` + the multi-VM
workers. New `cocoa_gui` workspace member; the view **models** stay shared
with Phase G (a second renderer, not a fork). Parallel to the core; no CG
gate blocks a core sprint.

| Sprint | Size | Needs | Gate (headless seam; on-screen = user) |
|---|---|---|---|
| CG0 signal-infra prereqs | `S` | core only | per-thread alt-stacks: 2 VMs faulting concurrently both recover; `ExitProcess`-on-main exits the process, not a zombie |
| CG1 hosted worker + wake + load_list | `M` | workers M0–M4 | register a worker on the *current* thread (no spawn), wake hook fires, drain+reply round-trips; `load_list` loads an extra world list |
| CG2 crate + boot + window (G0) | `M` | CG0, CG1, bridge C0–C5 | parked-main boot completes; one Smalltalk-built `NSWindow`; `[NSApp run]` from Rust; ⌘Q clean; main-thread guard rejects a background-VM AppKit send |
| CG3 C6 reverse dispatch (G1) | `L` | CG2, `perform:withArguments:` | a `MacvmTableSource` answers `numberOfRowsInTableView:` with a real int; a raising delegate returns the shape default + next call dispatches; a forced SIGSEGV in a callback recovers |
| CG4 protocol + workspace + primary-restart (G2) | `L` | CG3 | `#uiReq`/`#uiReply` round-trip; `(peer,corr)` non-collision; primary death → respawn → re-sync; ⌘P → `7` |
| CG5 app shell: toolbar, metrics, theme, view switcher (G2b) | `M` | CG4 | `ViewRegistry` register/switch/menu-build (pure unit test); `PrimarySupervisor::metrics()` returns real `VmMetrics` from a live primary; `MACVM_COCOA_SNAP` captures real on-screen PNGs |
| CG6 Workspace, properly: selection + inline print it (G2c) | `S` | CG5 | selection-range round-trip (only the selected substring reaches the primary); Print it splices at the captured insertion point even if the selection moved before the reply |
| CG7 ClassBrowser (G3) | `M` | CG6 | outline data-source answers the same rows the `htmlFragment` model does (differential); snapshot pickles clean (no class oop) |
| CG8 CodeView + Find (G4a) | `M` | CG7, W4 accept path | a `#saveMethod` round-trips through `image_store` byte-identically to the web path |
| CG9 UI restart-in-place (G4b) | `M` | CG3, CG2 | a scripted foreign fault Drops the old handle (no reservation / PROBE-registry leak across many restarts) + reboots; N/T backstop trips |
| CG10 worker bracket + GamePane + drain (G5) | `M` | CG5; GamePane render | `do:then:` round-trip; default-mode drain not delivered in a nested mode; UI live during a parallel dive |

Sequencing: **CG0 + CG1 are core-only** (soundness + infra; land before any
AppKit code). **CG2 + CG3** hold the real risk (top-level-entry callback
dispatch, the boot handshake, reverse dispatch); CG4+ are mapping over a
proven base. **CG5 (app shell) gates CG6–CG10** — every later view needs
somewhere to live, so it lands before the browser rather than implicitly
alongside it. CG9 can follow CG2+CG3 independently of the shell/views if
crash resilience is the priority.

## Phase W — world track (library + apps, parallel after S6)

The Smalltalk side beyond the S6 seed: the full library and the programming
tools, designed from a file-level survey of the real Strongtalk source.
Design docs: [`WORLD.md`](WORLD.md) (library, `.dlt→.mst` converter, load
order) and [`APPS.md`](APPS.md) (mirrors, tools, HtmlWriter, reflection
primitives R1–R5). Like Phase G, this track runs parallel to the core and
interleaves with it; every wave lands with in-language tests.

| Wave | Size | Contents | Needs |
|---|---|---|---|
| W0 image store | `S` | `image_store` crate: versioned SQLite class/method source database + `.mst` importer + GUI class-browser wiring ([`IMAGE.md`](IMAGE.md)) | nothing — `.mst` text already exists |
| W1 library wave 1 | `M` | `tools/dlt2mst` converter (WORLD §10); full collections protocol, Fraction, Character tables, ReadStream; two-pass world loader; Strongtalk test-oracle port | S6 |
| W2 reflection base | `M` | Mirror library (Object/Class/Method mirrors), R1+R2 primitives, HtmlWriter, ToolNode framework (APPS §2–§6) | S6 (+A16) |
| W3 tools wave 1 | `M` | Inspector, Workspace/eval wiring (R3), find tools (senders/implementors sweeps) | W2; pairs with G2/G3 |
| W4 tools wave 2 | `L` | Browser node suite (hierarchy/class/category/method), accept path, update protocol | W3; pairs with G4 |
| W5 exceptions + SUnit | `M` | ANSI exception layer (~10 classes, zero new VM features — WORLD §7), SUnit 3.1 convergence; adds exception tests to S13+ stress gates | S4 solid, W1 |
| W6 benchmark harvest | `S` | Richards (13 cls), DeltaBlue (12 cls), Stanford suite + seeded LCG, BenchmarkRunner-style harness in `world/bench/` | W1; **feeds S15** |
| W-debugger | `L` | StackTraceInspector + ActivationMirror + R5 primitives (single-step interpreter mode) | W4; schedule with Phase-E processes |

## Phase E — stretch (unordered)

- **S16 Snapshot/image** — save/load the world (root schema = Universe list;
  no Rust vtables in-heap makes this mechanical §3.2).
- **S17 Green processes + scheduler** (§11); `Processor yield`, delays.
- **S18 Exceptions** — ANSI `Exception` hierarchy over NLR/ensure.
- **S19 Splitting & advanced opts** — Self-style splitting, better regalloc.
- **S20 Guest-language Cocoa + POSIX/BSD bridge** — Smalltalk code sending
  messages to Cocoa objects via `objc_msgSend` (the MacModula2 pattern) and
  calling curated libc functions directly, both ABI-driven by `cocoa_data`
  (a sibling repo's shared SQLite mirror of the macOS Obj-C + POSIX surface).
  Full design in [`docs/FFI.md`](FFI.md) (written as a non-disruptive side
  track, alongside but independent of S11–S14) — Tier 1 is BUILT (S20 steps
  1–6); Tier 2 is also BUILT — the C0–C5 ladder of
  [`cocoa_bridge_design.md`](cocoa_bridge_design.md) shipped in full
  (ObjcRef ownership, DNU dispatch, main-thread hop, callbacks, and the
  CocoaPad capstone — a native Cocoa window built from a Workspace-style
  demo; prims 230–245): two tiers (dynamic
  `doesNotUnderstand:`-based Cocoa dispatch reusing S11's PIC machinery for
  caching; a direct compiler-primitive path for POSIX calls), an `Alien`-style
  byte-array-backed representation reusing existing `IndexableBytes`
  primitives, and a working, tested offline generator crate (`ffi_gen`)
  that emits real `.mst` bindings. Distinct from the Phase G
  GUI shell, which is Rust-side hand-rolled `dlopen`/`objc_msgSend`
  (`gui/src/objc.rs`) and needs none of this.
- **S21 Mixins** — Strongtalk's mixin model on the reserved klass slot.
- **S22 Weak refs + finalization; weak symbol table.**
- **S23 ASM methods** — a method whose entire body is hand-written native
  AArch64, not compiled Smalltalk. Full design in [`docs/ASM.md`](ASM.md)
  (written as a non-disruptive side track, same posture as S20): a new
  `<asm: 'text'>` pragma (string-wrapped so `.mst`'s bracket-matcher never
  sees ARM64 addressing-mode `[`/`]` unescaped), a precise register contract
  reusing the calling convention `docs/arm64.md` §3 already defines for
  compiled code, and a v1 restriction to leaf routines only (no allocation,
  no calls, no safepoints — what licenses skipping oop-maps entirely).
  Reuses the existing `Nmethod`/`CodeTable` install mechanism verbatim, just
  fed bytes from JASM's real text assembler (`wfasm::a64::assemble`,
  vendored S9, otherwise unused in the current codebase) instead of the
  tier-1 compiler's own pipeline. No Strongtalk precedent exists for this
  (checked directly against the source) — genuinely novel for this lineage.
  A working, tested preview tool (`asm_preview`, already built) proves the
  mechanism against real worked examples; the frontend parser and installer
  themselves are this sprint's still-to-build work.
- **Multi-Smalltalk workers** — **COMPLETE (M0–M4)**: primary/worker VM
  parallelism with copy-passing messages, no shared state. The primary VM
  spawns worker VMs (each its own heap/JIT/thread), exchanging deep-copied
  object graphs via the MOP pickle over channels (prims 220–228,
  `world/47_worker.mst`); async end to end (`send:onReply:` continuations,
  event-driven wake, zero polling); crash = an ordinary `#workerDied`
  message; worker transcripts forward `[wN]`-tagged. Zero changes to the VM
  core's execution model — each heap stays strictly single-threaded;
  orthogonal to S17 green processes. Capstone shipped: `ParallelMandel`
  (`world/48`, Demos menu) computes every frame of the zooming Mandelbrot in
  bands across 4 worker VMs — **~2.65 CPUs sustained**, visibly faster than
  the single-VM dive. Design + as-built amendments in
  [`multi-smalltalk-worker.md`](multi-smalltalk-worker.md).
- **Native game engine** — a retro game pane driven entirely from Smalltalk:
  a linked-in primitive group (ids 200–215) emits drawing/sprite/audio commands
  over a `GameSink` channel (mirroring `TranscriptSink`) that the GUI renders on
  a native Metal pane via the `MacGamePane` sister crate (Metal graphics +
  AVFoundation audio). Frame loop is a main-thread `NSTimer` pulling one
  `GameStep` per tick (the worker stays strictly serial); `run` returns
  immediately with the step-block GC-rooted in a class variable. Full design and
  the M0–M4 milestone ladder in [`gamepane_design.md`](gamepane_design.md);
  `world/43_gamepane.mst` (GamePane/Sprite/Sound/Tune) + the `Breakout`
  (`world/44`) and `MandelZoom` (`world/45`) demos, reachable from the GUI's
  native **Demos** menu. Same non-disruptive side-track posture as S20/S23.

## Phase P — Windows-ARM64 port track

Port the VM to **Windows 11 ARM64** (`aarch64-pc-windows-msvc`), combining
this repo's A64 compiler (unchanged — the ISA carries over) with the Windows
OS layer the x64 sibling already proved. Design of record:
[`../MIGRATION.md`](../MIGRATION.md) (the five seams, component disposition,
source-of-truth rules); WINVM's own `MIGRATION.md` is the sibling playbook it
leans on. Per-sprint detail + gates: `sprints/sprint_p0N_detail.md` +
`sprints/tests_p0N.md`. The behavioral oracle is the **Mac build of the same
checkout** — same world, same bytecode, same suite; outputs must match.

| Sprint | Size | Needs | Gate (restated in tests_p0N.md) |
|---|---|---|---|
| P0 toolchain + seed + interpreter-only | `M` | nothing | `cargo build` + `cargo test` green on Windows ARM64 with JIT off; world boots; full interpreted suite matches the Mac run; arch asserted `aarch64` at runtime |
| P1 JIT substrate (loader, W^X, icache) | `M` | P0 | S9-style smoke on target: emit/execute/patch-and-rerun through `JitWriteGuard` with real `FlushInstructionCache`+`isb`; vendored corpus green on target |
| P2 trap layer (VEH `brk`, setjmp, recovery) | `L` | P1 | `brk 0xDE00` → VEH → trampoline → resume round-trip; 50-frame longjmp with zero `Drop`s; foreign AV recovered; embedded-VmHandle DNU recovery; PROBE dossier on ARM64 |
| P3 tier-1 alive (the differential gate) | `M` | P2 | full suite green at `MACVM_JIT=threshold=1` **and** `MACVM_GC_STRESS=1` **and** `MACVM_DEOPT_STRESS=1`; benchmarks recorded in PERF.md vs the Mac's same-checkout numbers |
| P4 GUI shell (Win32 + WebView2) | `M` | P2 (+P3 for JIT-on) | class browser + workspace usable in the running app; transcript round-trip; DNU in Workspace recovers; native-ARM64 process asserted |
| P5 FFI + world gaps (winkb, ARM64 ABI) | `L` | P1, P2 | FFI smoke against real Win32 imports; ARM64 classifier unit-pinned (HFA/16-byte/by-ref); `Time now` wall-clock works; gated world FFI files resolved (enabled or explicitly re-scoped) |

Sequencing: P0→P1→P2→P3 is the spine, strictly ordered. P4 branches after P2
(a GUI without guest-fatal recovery dies on the first Workspace typo — WINVM
shipped that bug and documents it), and only its metrics/JIT panes want P3.
P5 branches after P2 and is independent of P3/P4. Mac-only tracks (CG, S20's
Cocoa tier, gamepane, abc_player) are **gated out, not ported** — each gets a
clean-fail Windows stub, WINVM's pattern.

## Phase WG — the Windows-native environment (parallel, after P5)

The Windows twin of Phase CG: the macVM environment as a **native Win32
window whose UI is written in Smalltalk** — `WinRef` handles over the
winkb-resolved FFI, a WndProc door dispatching messages into a UI worker VM
as top-level entries, view models shared with the web GUI, loaded as the
`winui.list` conditional world layer users can browse and edit live.
Design of record (incl. the gallery review it answers):
[`win_gui_design.md`](win_gui_design.md). Reference gallery:
`MACVM/docs/gallery/`. P2's trap/recovery layer was this track's CG0 and
is already landed; **P5's resolver is the gate to WG0**.

| Sprint | Size | Needs | Gate (headless; on-screen = snap verb) |
|---|---|---|---|
| WG0 FFI probe | `S` | P5 resolver | user32 round-trip from a Smalltalk doit (`RegisterClassW`/`CreateWindowExW`/`DestroyWindow`); `WNDCLASSW` built via Alien + winkb struct offsets |
| WG1 window + loop | `M` | WG0 | Mica + dark-titlebar top-level window; message loop owned by the hosted UI VM on main; `macvm-winui` bin; snap verb captures PNG |
| WG2 the WndProc door | `L` | WG1 | messages dispatch into `WinShell` as top-level entries; raising handler → `DefWindowProc` + next message dispatches; forced AV in a handler recovers (P2 layer); door latency measured |
| WG3 controls + layout | `M` | WG2 | **the flag-and-drain pass first** (§2.4a), then Smalltalk-created common controls whose notifications FLAG from the door and are serviced by the drain; `WM_SIZE` layout in Smalltalk; tracking-suppression across `WM_ENTERSIZEMOVE`/`WM_ENTERMENULOOP`; visual-styles v6 + PerMonitorV2 DPI 
| WG4 shell chrome | `M` | WG3 | view bar (Fluent glyphs + labels + accent underline), lazy views, docked Transcript, live metrics cluster, verb enablement by focus |
| WG5 Workspace + Browser | `L` | WG4 | syntax-coloured Workspace (Ctrl-D/Ctrl-P, ghost line); four-pane Browser on the shared model; Accept persists byte-identically to the web path |
| WG6 Outliner + Find + Editor | `M` | WG5 | live-reflection tree; Find lands selections in the Browser; File In / Add to World |
| WG7 Debugger + Monitor | `M` | WG5 | halt loop fronted natively (+F5/F10/F11); Monitor with column priority; primary restart-in-place |
| WG8 Screen memory + docs | `L` | WG7 | SM0's PIXEL plane on the existing D3D device (a `D3D11_USAGE_DYNAMIC` texture the guest stores into through an `Alien`, sampled beside the cell grid); SM4's palette-as-memory; runnable doc examples; every empty state teaches |
| WG9 SUnit + Tests tab | `M` | WG6 | `TestCase` in the world, a Tests tab, a headless runner — edit a class, run its tests, click a failure, end in the Debugger (upstream `6536294`) |
| WG10 Demo gallery | `M` | WG8, WG9 | Life, Julia, plasma on the pixel plane; FreeCell and Minesweeper on the text plane we already have; `docs/gallery-win/` captured |

Games/sound (D3D11/XAudio2) stay the recorded stretch — the Demos menu
greys with a reason naming the design doc, never silently.

**WG8 was `GDI-blit Canvas` until 2026-08-12** and is not any more. Upstream's
SM0–SM4 (`docs/shared_screen_memory_design.md`) moves bulk state OFF the
command channel and into memory the VM writes directly, and a blit is precisely
the three-copies-per-frame path it exists to delete. Building the blit first
would be building the thing we would then replace.

The review is `docs/sprints/upstream_review_2026-08-12.md`, and its finding is
worth carrying here: **WG6d already built SM1's text plane**, independently and
from the opposite direction — upstream reached it from bandwidth, this port
from correctness (two authorities computing one pixel, four shipped defects).
So Windows needs SM0, not SM1, and three views already run on the text plane.

**SM0 landed 2026-08-12 as the Canvas view** (`world/116_winui_pixels.mst`,
`gate-wg8`). The plane is a `D3D11_USAGE_DYNAMIC` BGRA texture whose mapped
pointer the guest stores into through an `Alien`; the renderer draws it beneath
the cell grid, and cells carrying `BG_TRANSPARENT` skip their background fill so
a text HUD composes over the pixels without either side knowing about the other.

It is worth recording HOW it landed, because the correction was the design. The
first cut drew its plasma into the *Editor's* pane — that pane already had a
renderer attached, so it was the shortest path to a coloured rectangle — and the
response to that was "a strange place to test pixels; we are meant to have a
canvas tab". Correct on both counts: this row always said Canvas, and a pixel
plane borrowed into somebody else's pane demonstrates a texture, not a view.
`gate-wg8` now leads with three assertions about a view *existing* — registered,
switchable, built — before it asserts anything about pixels at all.

**SM4 landed the same day** — an indexed plane plus a 256-word palette, both
memory the guest writes. Re-colouring the screen costs the palette's size, not
the screen's: 256 stores against 19,200 at 160x120. The Canvas runs both modes
and names the cost in its HUD, because the difference is not visible from the
picture.

**"Runnable doc examples" is meant literally.** [`winui-cookbook.md`](winui-cookbook.md)'s
fenced blocks are extracted by `scripts/cookbook-to-tcl.py` and *evaluated*
against a live window by `just gate-cookbook`. It caught a dead selector on its
first run, which is the entire argument for it: this port has already renamed a
call site, changed a stride from assumed to asked-for, and moved the Monitor
from an EDIT control to a cell grid — prose written before any of those would
still read plausibly today.

**Empty states now teach** — Debugger, Monitor and Find. The Debugger's was the
single word `RUNNING`: accurate, and useless to the one person who most needs it,
someone who opened the view to find out what it is for. Three notes worth
keeping, because each was a defect the change surfaced:

- `RUNNING` is a **load-bearing sentinel** (`isHalted` reads the report rather
  than keeping a second copy of the truth), so teaching text goes *after* it.
- The host pushes `RUNNING\n` at boot, so the guest's nil case never fired. The
  host states the fact; this side chooses the words.
- The text must be **ASCII**. A guest String is UTF-8 bytes and the cell grid
  takes codepoints, so one em-dash arrived as three cells reading `a00` — the
  fourth time that mismatch has reached a screen here. Now asserted per
  character rather than remembered.

WG8 is complete.

### WG9 — SUnit and the Tests tab (2026-08-12)

**SUnit moved into the world.** It was `world/tests/00_sunit.mst`, loaded only by
`tests/it_world.rs`, which made it part of the *harness* rather than part of the
language. That was fine while the only tests were ours and the only runner was
cargo. It stopped being fine the moment the shell grew a Browser that compiles
into a running image: a `TestCase` someone writes there has to be a real class in
the primary. It is `world/85_sunit.mst` now and loads from `world.list` for
everyone — `macvm run`, the repl, the web GUI, the Windows shell and the harness
alike. `tests.list` no longer carries a copy; two identical definitions would
shadow each other and hide any drift.

**`world/86_sunit_runner.mst`** adds the two things SUnit deliberately lacked:
discovery (`allTestClasses`, by the superclass *chain* so an intermediate
`MyProjectTestCase` still works) and a runner that **answers instead of
printing** — `TestRunner report` calls `Smalltalk quit:`, which is right for a
cargo run and fatal for a GUI.

**The Tests view runs in the primary.** Test classes live where the user's
objects live, so the view ships one expression across the worker seam and formats
what comes back — it does not know what a `TestCase` is and must not. The reply
is asynchronous, so the window keeps drawing throughout; the pane says
`Running the suite in the primary…` and re-renders when the reply lands.

`gate-wg9` gates the whole loop: an image with no test classes says so in words,
then File In puts two in it — **one passing and one failing**, because a suite
that can only report success is indistinguishable from one that always reports
it — and the view is checked against what they actually did.

Three notes from building it, each a defect the gate or an existing test caught:

- A nested block does **not** see a temp declared in an enclosing *block* here,
  only one declared in the enclosing *method*. `| col |` inside the outer block
  read as nil and failed with `does not understand <`.
- `,` is binary and binds tighter than a keyword message, so
  `a , b ifTrue: […]` makes the whole concatenation the receiver.
- The glyph table's own uniqueness test caught `#tests` colliding with
  `#transcript` before it ever reached a screen.

### WG10 + WG10a — the gallery, and the renderer it forced (2026-08-12)

WG10's demos (Life on the indexed plane, Julia in fixed point on the direct
one, `world/119_winui_demos.mst`) did their job in the way the review predicted
— **they were the reason to build SM0, and they broke the renderer into being
built properly.** Running them surfaced, in order:

1. ~~**Tier-up counts method entries, not loop back-edges** — no OSR.~~
   **WRONG ON BOTH COUNTS — corrected below.** Kept struck through rather than
   deleted, because the wrong version was committed and someone will find it.
   The measurement was real (11 ms vs 119 ms); the conclusion drawn from it was
   an artefact of the harness. See *The OSR correction* below.
2. **A free-running animation loop saturates the UI thread** — 200 fps of
   frames the panel drops, a busy cursor, and eventually a starved control
   port. Frames are now *requested*: a dedicated ~16 ms timer, a due-check at
   `canvasFpsTarget` (30), and no posted wake. Between frames the thread idles
   in `GetMessage`, where an interactive thread belongs.
3. **The D2D present path was a CPU pipe wearing a GPU device** — a fresh D2D
   bitmap per frame for the plane, per-glyph `DrawGlyphRun` for the text, and
   SM4's palette expanded by the CPU. Named by the author and replaced.

**WG10a is the replacement** (`winui_render/src/gpu.rs`), ported from
upstream's Metal renderer (`MacGamePane/graphics`), Snapdragon-first:

- One fullscreen-triangle vertex shader; three pixel shaders.
- **Indexed plane**: `R8_UINT` texture + 256×1 palette texture, and the lookup
  happens IN the shader — upstream's `fmain` verbatim. A palette cycle is a
  1KB upload; the Adreno re-colours the screen. The CPU `resolve()` is gone.
- **Direct plane**: `B8G8R8A8` dynamic texture, point-sampled (interpolation
  would invent pixels the guest never wrote).
- **Text**: the cell grid as a `cols×rows` `RGBA32_UINT` texture + a
  DirectWrite-rasterised glyph atlas (`IDWriteGlyphRunAnalysis`, once per new
  glyph); one draw call composites every glyph, `BG_TRANSPARENT` is a blend,
  and the caret is two shader pixels.
- **Unified memory, asked not assumed**: every per-frame upload is
  `Map(WRITE_DISCARD)` — a driver rename into the one LPDDR5x pool on a UMA
  part — and `MacvmRenderIsUma` reports `CheckFeatureSupport`'s answer to the
  guest. This machine answers **1**.
- `Present(1)`: presents ride the panel's refresh.

Two defects the rewrite caught, both now pinned by headless GPU readback tests
(`gpu::tests`, rendering the real pipeline into an offscreen target):

- The fullscreen triangle is counter-clockwise and **D3D11's default
  rasterizer culled it** — every counter green over a silently white pane.
  Metal has no default cull, which is why upstream never met this.
  `CULL_NONE` is load-bearing.
- The WG8 gate asserted a hard-coded stride (160) rather than the
  relationship (`indexStride = stride/4`); the resolution change broke the
  constant, not the claim.

Measured after: plasma ~34 fps at the 30 fps target, Julia ~13 fps
(CPU-bound in fixed point at 160×120 — its plane is smaller per mode because
per-pixel *iteration* does not belong on the UI thread), and the control port
answers instantly while both run.

### The OSR correction (2026-08-12, same day)

The author's response to the note above was one line: *"our compiler is designed
to do OSR — investigate this issue."* It is, and it does. The note was wrong.

**This VM performs on-stack replacement, and it counts back-edges, not method
entries.** `OP_JUMP_BACK` (`src/interpreter/mod.rs:602`) bumps a per-method loop
counter and offers the running frame for replacement every `LOOP_COUNTER_LIMIT`
= 10,000 back-edges (`src/oops/layout.rs:334`); `rt_osr_request`
(`src/runtime/osr.rs`) compiles with an OSR entry and replaces the frame in
place. It has been live since `0ed9d65` (2026-07-05) — before the note claiming
its absence was written. Measured on the same loop, same arithmetic:

| shape | time | `osrEntries` | `osrDeclined` |
|---|---|---|---|
| installed class-side method, entered **once** | **4 ms** | **+1** | 0 |
| identical loop inside a **block** | **146 ms** | 0 | **+50** |

**The real boundary is INSTALLED vs ANONYMOUS.** `rt_osr_request`
(`src/runtime/osr.rs:102`) declines any frame whose method dynamic lookup cannot
re-find — the guard exists so an OSR nmethod is never installed under a
(klass, selector) key that dispatch would resolve to a different method. A
workspace doit is an anonymous, never-installed `#doIt` run on nil; every block
runs under the placeholder selector `#aBlock`
(`src/bytecode/builder.rs:620`). Neither can pass, ever. And the frontend
*inlines* `to:do:`/`whileTrue:`/`timesRepeat:` into whatever unit textually
contains them — so a loop typed at a workspace, or timed inside
`Time millisecondsToRun: [ … ]`, puts its back edge in a unit that is
structurally ineligible and is declined every 10,000 back-edges forever.

**That is what the 119 ms measured**: a loop inside a timing block. The 11 ms
figure was 19,200 calls to an installed method tiering on the invocation counter
at call #20. Two different tiering paths, neither of them absent.

**A second, real defect the investigation found**: the compiler's envelope
refuses `argc > 5` and any send site with `argc > 7`
(`src/compiler/driver.rs:234-281`). `juliaRow:`'s **eight** keywords put it
permanently outside it — so the row method whose own comment claimed it "tiers
up and its loop is compiled" could never compile by *either* path, and
`drawJuliaOn:`'s 8-argument send site disqualified that method too. The row's
speed came entirely from `escapeAtX:y:cx:cy:` (argc 4) compiling. Fixed: the
per-frame constants moved to class variables, `juliaRow:` is argc 4, both
methods are inside the envelope.

**And one VM wart, fixed**: `rt_osr_request` never consulted `compile_disabled`,
which only a `NoPermanent` verdict sets — so a permanently ineligible method
re-ran the full decode + eligibility scan on every 10,000 back-edges, forever
(≈ twice a frame for `juliaRow:`). The call-path trigger has always checked it
(`src/interpreter/send.rs:205`); OSR now does too (`src/runtime/osr.rs`).

**And a standing rule, restated because it was nearly violated an hour later:
JIT thresholds below 20 are not a supported configuration.** `parse_jit` clamps
the env/CLI surface with *"rule 1: never below 20 — sub-floor compiles graft
from cold ICs and measure cold-compile deopts, not programs"*. When WG10's Life
tests turned `world_suite_at_sub_floor_threshold_survives_root_block_deopt` red
with a wrong ANSWER (`expected 1 got nil`) at `Threshold(2)`, the investigation
that started was into the wrong thing: a failure reachable only below the floor
is out of contract, and the response to it must never be to change the compiler
for a threshold the compiler refuses to run.

The defect that test guards is a `.expect()` inside `rt_uncommon_trap` — an
`extern "C"` **non-unwinding abort** that takes the whole test binary down. So
*surviving to produce a report line* is the guard; the `", 0 failed"` it also
asserted guarded nothing about the abort while quietly gating the entire world
corpus on an unsupported configuration. It now asserts survival and a plausible
run count, and REPORTS any sub-floor divergence to stderr instead of failing —
the information stays visible, the build does not go red for something that
cannot ship.

**The two durable rules**, which is what the original note should have said:

> A hot loop tiers only if its back edge lives in an **installed method** with
> **argc ≤ 5**. Anything timed from a workspace or inside a block measures the
> interpreter — so a benchmark must call an installed method and time the *call*.

### WG11 status, and the defect that must be fixed FIRST (2026-08-12)

**Life and FreeCell play, unedited.** `45a_life.mst` simulates (GEN 376 -> 480
in ten seconds, ~10.4 gen/s against the 15/s it asks for) with its 5x7 HUD;
`45c_freecell.mst` deals 617 with real ranks, suits, free cells and
foundations. Both are `git show upstream/main:` byte-for-byte. W0-W5 done.

**FIXED — the primitive numbers had COLLIDED.** WINARM independently allocated
ids 266-272 for its own Windows primitives while upstream had allocated the SAME
ids to GamePane shared memory. That broke the portability contract at its
foundation: the same Smalltalk cannot run on two machines while a primitive
number means two different things. WINARM's moved to 300-306 (upstream is the
source of truth and cannot move; 300+ leaves it the whole 200-299 block it has
been growing into):

| was | now | WINARM primitive | upstream wanted the old number for |
|---|---|---|---|
| 266 | 300 |  |  |
| 267 | 301 |  |  |
| 268 | 302 |  |  |
| 269 | 303 |  |  |
| 270 | 304 |  |  |
| 271 | 305 |  |  |
| 272 | 306 |  |  |

**Arity guards were not enough, and the reason is worth keeping.** They were
added first and they are correct —  and  now
fail a primitive whose declared arity does not match the method's, instead of
indexing past the end of the argument slice. But 272 was  upstream and
 here, BOTH taking no arguments: identical arity, so no
guard can see the difference and the wrong primitive simply runs, answering a
window-proc address as a column count. Only distinct numbers fix that.

**And an unimplemented primitive is now a FALLTHROUGH, not a crash.** A method
may declare any number; whether this VM implements it is not the guest's
business, and Smalltalk already has an exact contract for a primitive that does
not happen — the method's own body runs. That is precisely what upstream's
GamePane wrappers rely on ( answers ,  answers
), which is how its games LOAD and degrade on a build that has not
implemented them. It used to , so opening Minesweeper killed the primary
and took the user'''s image with it — from a build whose only sin was not having
written that primitive yet.

Minesweeper now draws its tile grid and the shell survives. It is still WRONG —
sheared, because / are no-ops and the world buffer is
bigger than the viewport, and its HUD is missing because there is no text plane.
That is W8/W9, and it is now a missing feature rather than a crash.

**WG11 — GamePane parity (settled 2026-08-12).** Not just `shader:` — the
whole of upstream's guest-facing games surface, implemented on the D3D11 pipe,
under two principles the author set:

1. **Both engines, always.** A shader demo showcases the Adreno; it says
   nothing about the compiler or the Oryon cores. Every demo that can exist in
   both forms keeps both: `shader:` source runs on the GPU, and when no source
   exists for this backend the SOFTWARE version runs — so the CPU path is not
   a curiosity but the portability guarantee (a game degrades to the compiler,
   never to a black screen). The HUD names the engine and its frame time.
2. **The same Smalltalk runs on every system** — mac, windows, arm, x64,
   linux when ported. Which means the guest API must be UPSTREAM'S, not ours:
   `GamePane` (`shader:`/`shaderParam:value:`, prims 259/260), IndexedPane's
   draw calls (`pset`/`cls`/`fillRect:`/`line:`/`circle:`/`blit:`), its
   palette model (per-scanline 1–15, global 16–255, index 0 transparent — the
   copper contract; our flat SM4 palette converges to it, a shader edit now
   that lookup is GPU-side), text overlay, and input.

   **The shaders are the hardware emulation, and they are hardware/OS
   specific** (the author's words). GamePane is a fantasy-console HARDWARE
   SPEC; the shaders are each platform's implementation of that console's
   video chip — Metal on the Mac, HLSL in `gpu.rs` here — authored per-OS by
   the port, never shipped by a game. Games touch the emulated hardware's
   state only, the way a ROM touches an emulated video chip's registers: an
   emulator's video core belongs to the emulator, not to the ROM. That is WHY
   the same game runs everywhere, and it makes WG11's scope precise — finish
   emulating upstream's chip behind upstream's primitives.

   **And the Metal→D3D11 port has been done before** — the author has shipped
   it twice, and the artifacts are vendored in
   [`docs/reference/gamepane-d3d/`](reference/gamepane-d3d/README.md):
   WINDARTTALK's `shaders.hlsl` (the complete, line-cited HLSL translation of
   `gp_engine.mm` — copper palette, sprites, the compute blitter with its
   SRV/UAV hazards documented, the runtime `fmain` template with the
   cbuffer-packing trap solved, nearest-letterbox present) plus its host-side
   design doc — whose §5.6 independently reaches this port's own UMA
   conclusion — and wingui's production shader set (`text_grid.hlsl` with a
   CRT mode, compute fill/line, sprites). WG11 implements that reference
   behind upstream's primitives; it is not a translation project.

The acceptance gate states the principle: **an upstream game's `.mst` loads
and plays unchanged.** FreeCell or Minesweeper copied byte-for-byte from
`MACVM/world` is the test — editing the game's source fails the gate, because
editing the game is what the concept forbids. Canvas input routing (WG10's
remainder) is therefore a prerequisite, not a parallel task.

**W11 landed (2026-08-12): the layer-0 shader, and galaxigans plays its
cosmos.** The shim is the sister port's, function for function
(`winui_render/src/msl.rs` ← WINDARTTALK `gp_engine_d3d.cpp`): calls-only
dialect rewrites (`fract`→`frac`, `mix`→`lerp`, 2-arg `atan`→`atan2`, `mod`→a
floor-based helper because `fmod` truncates and disagrees on negatives), the
`fragment … [[stage_in]]` entry rewriter, and the 144-byte cbuffer with
`p[8]` at 16-byte stride. Compiled at runtime by the same `D3DCompile` the
built-in shaders use; drawn as pass 0 under the plane. **A live shader forces
the copper contract** — that one decision made the indexed path free
(copper's index-0 `discard` already composes) and reduced the direct path to
"index 0 resolves transparent + the plane blends". Compile failure keeps the
previous shader and prints the compiler's text once — degradation is to the
software look, never to a dead pane (`gpu.rs` tests pin both, including
compiling galaxigans' actual 12-scene source out of the world file).
`linePaletteAt:` (261) rides the same effective-copper table, which is what
animates the boss beam. Two galaxigans-shaped traps, recorded: the copper
direct branch had never composited the legacy 5×7 overlay (the Copper demo
uses SM1 cells), so the HUD vanished until `composite_over` topped that
branch too; and `cargo build --release` does NOT rebuild the non-default
GUI members — a stale `winui_render.dll` missing one export nulls the whole
`RenderApi` OnceLock and every game silently stops uploading. Build with
`-p win_gui -p winui_host -p winui_render`, like the gates do.

**W12 landed (2026-08-12): sound, and the match is now exhaustive.** The
split is the portability contract exactly: SYNTHESIS is upstream's own Rust,
copied byte-for-byte (`cmp` agrees) from the crate the Mac host links —
`MacGamePane/audio/src/synth.rs` and `abc.rs` → `win_gui/src/sound/` — so the
recipes, the LCG and therefore the SAMPLES are identical; only the OUTPUT is
Windows. That output is the sister Dart port's design (`GP_AUDIO_DESIGN.md`):
**XAudio2** for SFX, whose one structural difference from the Mac is that a
source voice plays its queue sequentially, so polyphony is a POOL (24 voices,
round-robin steal when busy) with the mastering voice summing; and **winmm
`midiStream`** for ABC tunes against the GS Wavetable synth — the half the
sister designed and left as a no-op, implemented here with its documented
tick math (`ms * bpm / 125`, 480 ppq, tempo `60000000/bpm`). The lifetime
hazard it flags is honoured: XAudio2 does NOT copy `pAudioData`, so presets
are rendered once and LEAKED, and rendered effects are retired to a list
drained only when every voice reports idle. No audio device degrades to
silence, never to a dead game. Sound needs no pump hand-off — XAudio2 and
winmm are free-threaded, unlike D3D's thread_local seam — so the sink's arms
call straight through and a sound plays the instant the game asks.

**The `_ => {}` catch-all is gone**, and that is the parity statement: every
`GameCommand` upstream defines is handled, the match is exhaustive, and a
command added upstream now FAILS THE BUILD rather than being silently
swallowed. GamePane parity is complete.

Verification note worth keeping: the endpoint peak meter
(`IAudioMeterInformation`) read the noise floor while sound was audibly
playing, and XAudio2's own `SamplesPlayed` stayed 0 for the same buffers —
BOTH instruments lied. Sound was confirmed by ear. Do not gate audio on
either counter.

**W13 reset the pane between demos, and fixed `overscan:`.** Three defects,
all found by looking at the gallery rather than at the code, and reported
from a screenshot rather than by a test.

Demos inherited each other's LAYERS — Plasma ran under Minesweeper's
`MINES 32` and FreeCell's `DEAL/MOVES` row, three demos after either had
exited. The Mac gets the cleanup free (its pane is an object and `teardown()`
drops the whole `NativeGame`); here the layers are process-lifetime statics,
leaked deliberately because the guest holds pointers into them, so "drop the
pane" has to be spelled out.

They also inherited each other's SIZE and PALETTE. Only two demos ask for a
shape — galaxigans wants 640x360, Plasma opens 320x240 direct — and every
other one draws for the default pane without ever saying so, so after
galaxigans, Life laid an 80x60 grid meant for 320x240 across a 640x360 screen
and sat in the corner of it. The wipe now restores both, and the HOST seeds
upstream's sixteen default colours as well as the guest, because a wiped pane
is on screen for however many frames pass before the new demo's
`GamePane new` runs.

And **`overscan:` never worked**: the arm stored its margin and reallocated
nothing. Minesweeper asks for a 16-pixel border, draws into a 352x272 buffer
and blits it — so 352-wide rows were copied into a 320-wide plane and every
row slipped 32 pixels further left than the one above. The board sheared, and
the numbers inside the tiles, which are pixels and not text, sheared with it,
for the whole life of the port. `GameFrame` now carries the world and the
viewport separately, and `crop_world` takes the viewport window out of the
world at the current scroll on the way to the renderer. Cropping on the HOST
rather than in the shader is the smaller change and the better one: the
copper keys off the SCREEN row, and cropping first makes the screen row the
destination row with no further arithmetic. The renderer's own scroll is
passed zero so the pan cannot be applied twice.

Worth keeping as a rule: **the gallery is a test.** All three of these were
invisible to 9877 passing assertions and obvious in a screenshot.

### WG14 — the declarative UI (design settled 2026-08-13)

The direction, verbatim: *"we need a first class replacement here for the
cocoa ui, and I think the declarative UI will be it."* Design of record:
[`win_declarative_ui.md`](win_declarative_ui.md). The short of it: adopt the
spec+bind CONTRACT of `albanread/wingui` (the extraction of winscheme's
declarative Win32 UI — JSON-shaped node trees, id-based reconciliation,
event names bound to host handlers, full-spec republish with diffing), and
implement the realizer NATIVELY in the world layer over what this port
already trusts: the `WinControl` factory, `winui_render` (whose three
shaders ARE wingui's three pane types), and the door. winscheme itself is
the precedent — its reconciler lives in Scheme, not C++.

Staged as WG14a (WinSpec: build/normalize/validate/diff, pure, headless),
WG14b (WinRealize MVP + WinToolWindow + the Tools menu — the tool-window
lifecycle the Mac never abstracted: open/hide-on-close/teardown/tick/theme,
owned once), WG14c (panes + slider + select; THE SPRITE EDITOR ships on it),
WG14d+ (table/tree/tabs/split as tools need them; Sound Editor migrates).
Both reference repos are cloned at `C:\projects\wingui` and
`C:\projects\winscheme-dev-2026`; where their docs and headers disagree,
the headers are truth.

**WG14a and WG14b landed (2026-08-13).** The pure half first — `WinSpec`
builds node trees, `normalize` stamps `__auto__:<path>` ids, `validate` runs
the publish-time contract at BUILD time, and `diff:against:` answers patch
ops or nil for "cannot express this, republish". Then the realizer:
`WinSpecLayout` is pure arithmetic (metrics passed IN, so it is testable to
the pixel), and `WinToolWindow` is the lifecycle the Mac never abstracted —
registry, front-or-build, hide-on-close, teardown order, beat tick, and the
event-depth discipline that coalesces N state changes in a handler into one
reconcile. The Tools menu is built from the tool registry the way the View
menu is built from the view registry.

Proven on screen by `world/125_winui_demotool.mst`, which is three things and
no more: a title, a block that answers a spec, a block that handles an event.
It displays its own `patches`/`rebuilds` counters, so the claim is checkable
by looking — three clicks moved `clicks:` and left `rebuilds:` at 1.

Two bugs worth keeping, both found by looking rather than by testing:
`moveTo:` is `{x. y. WIDTH. HEIGHT}` and was handed right/bottom, which drew
every control at roughly twice its size and clipped captions away to nothing;
and a CHECKBOX must be exempt from the label rule, because Win32 draws its
caption beside the box and a STATIC above renders the word twice.

**WG14c landed the same day: panes, sliders, and the Sprite Editor.** The
three pane types realize onto `winui_render` exactly as the Canvas does, and
a pane's clicks reach its tool with the point in the pane's own pixels. The
slider needed `ICC_BAR_CLASSES` (its class is in no other set) and is POLLED
on the drain's heartbeat, because `WM_HSCROLL` is not on the door's allowlist
and a drag is a storm of them.

`world/126_winui_spriteed.mst` is the first real tool: SpriteDoc was already
in the world list on every platform, so this is a face and about a fifth the
size of the Cocoa one it replaces. Both surfaces are `rgbaPane` nodes drawn
by the tool's paint block.

Four findings, all from running it:

* **A pane's contents change without the spec changing.** A pencil stroke
  moves pixels and no props at all, so the differ correctly answered "no ops"
  and the paint block never ran. The paint block now runs on EVERY rerender —
  "nothing changed" means nothing changed about the CONTROLS.
* **A pane must carry an event or it is silent, silently.** It drew perfectly
  and reported nothing. A pane's event now defaults to its id, which is
  wingui's own rule for menu items.
* **Menu ids from a Dictionary are not stable.** The Tools menu was built by
  iterating one, so a tool could get a different id from run to run.
  Registration order is now kept explicitly beside it.
* **`getPx:` per device pixel is 186,624 sends per repaint** — slow enough
  that the door declined the next click while the VM was busy and two thirds
  of a stroke went missing. Asking once per CELL is 256.

**And then the blast went entirely** (same day, at the author's prompting:
the declarative UI HAS all the information it needs to not re-blast — that
is almost the feature of a patching system). The differ carries control
changes and refuses to guess; pane pixels are outside the spec, so they get
their own channel: a handler that changes what a pane shows INVALIDATES it —
the whole pane, or a `{x. y. w. h}` region in the pane's own pixels — and
the paint block is called once per dirty pane with that region. Regions
union into a bounding box between paints; whole-pane absorbs regions; taking
the damage clears it; a pane nobody invalidated is never painted at all.
A pencil stroke now invalidates ONE CELL and repaints 729 pixels where the
first cut rewrote 186,624 — and the proof is behavioural: the same 16-click
diagonal that lost two thirds of its strokes to a busy VM now lands all 16.
The pane's plane is retained between paints (it reallocates only on a size
change), which is what makes partial repaint sound.

**The Sound Editor migrated onto the declarative UI** the same day
(`world/127_winui_soundtool.mst`) — the migration this section promised,
`Sound Editor first`. It gets the Mac's sliders back WITHOUT the twelve
hand-written fixed-point mappings WG13 built a keyboard grid to avoid: the
fields are a TABLE (event, label, range, kind) and one pure conversion pair
serves every slider, tested at the endpoints and midpoint of every range.
The envelope visualization is a damage-driven `rgbaPane` — only the seven
fields that change the CURVE repaint it; noise, distortion and the echo trio
are audible, not visible, and dragging them repaints nothing. Audition is
the Mac's, behaviour for behaviour: Play/preset/random/mutate/wave-click
sound the recipe through primitive 263 in the primary; a slider drag does
not. Proved live with the endpoint peak meter: presets clicked in the tool
window peaked 0.51-0.81 from a dead-silent baseline.

And the migration found the realizer's best bug yet, with a signature worth
recording: `applyNodeProps:` picked ONE control for a patch's whole
changed-list. A slider's patch changes both its label (the readout) and its
value (the thumb); the label control won, the readout updated, the
TBM_SETPOS went to a STATIC as a harmless no-op — and THE THUMB STAYED PUT.
The heartbeat poll then read the stale thumb, disagreed with the echoed
spec, and dispatched the OLD position back into the document: the screen
quietly REVERTING the model, four presets in and every slider one recipe
behind, with the status line correctly naming a preset whose values were no
longer there. Headless the handler was provably perfect, which is what
isolated the patch path in one probe. Each prop now routes to ITS control —
title to the card's STATIC, label to the readout, everything else to the
control itself.

**WG14d: weighted rows, because every composition was a tower.** The
author's review, verbatim: we are led to create only vertical layouts, which
is not suitable for the sound tool — layout is meant to be automatic, but we
need more than just stack. The missing primitive was one, not many: `row`
hands children their natural widths, so nothing could say `split this width
into columns and let each column be a stack`. A child carrying `#weight` now
asks for a share — naturals are measured first, the remainder divides by
proportion with the last weighted child taking the rounding remainder so the
split is exact, and a weighted child advances the cursor by its SLOT whether
or not it filled it, because columns that jitter with their content are not
columns. The no-stretch rule survives by SCOPING: a weightless row lays out
to yesterday's pixels, asserted as such. The Sound Editor is the proof —
re-specced landscape at 1150x720 (envelope, verbs, oscillators and sheet on
the left; the twelve recipe sliders in two aligned columns of six on the
right, the Mac's own 2x6) from a 480x980 tower, by changing only the spec.

**The Sprite Editor's remaining verbs (2026-08-13).** Load, Sheets, Preview,
nudge and resize — the tool now does everything the Mac's does, and it went
landscape on the same weights the Sound Editor uses. Two findings worth
keeping:

* **A doit crossing the worker seam cannot declare temporaries.** It is
  compiled as a top-level chunk, and this world's parser refuses `| a b |`
  there. The preview raised, the reply block cheerfully reported `up`, and
  the only symptom was a black pane — an upload probe showed exactly one
  frame, the wipe's own. Wrapping the body in a BLOCK (which may declare
  temps anywhere) fixed it, and the reason is now written where the string
  is built.
* **The scrapers must be pointed at ONE method, not a class.** A sheet's
  source carries digits in its header comment and its `installOn:` loops, so
  a palette scrape over the whole class finds far more than forty-eight
  numbers and correctly refuses. Load asks the image for the `palette`
  method; the test proves the emitter puts sixteen triples somewhere a
  triple-scrape reads them, by ASKING the scraper to find that line.

**`select` and `listBox` realized (2026-08-13).** The last two node types the
spec declared and the realizer could not build. They differ by four message
numbers and their shape, so the numbers are chosen by one method and the
logic is written once — anything else is the same code twice with `CB` and
`LB` swapped, which is how two controls drift apart.

Three decisions worth keeping. An option may be a plain String, an
Association, or a two-element Array, so `optionValue:`/`optionText:` decide
which part is which — once, purely, with every spelling asserted. Selection
crosses as the option's VALUE, never its text or its index, because the value
is what the spec said and what the handler will be given back; the demo
proves it by showing `Blue` in the combo and `colour: #b` in its state card.
And a COMBO BOX'S CREATION HEIGHT IS ITS DROPPED HEIGHT — Win32 sizes the
closed control from the font and uses the remainder for the list — so the
realizer adds room for eight rows while the layout keeps reasoning about the
closed height, which is the one the arrangement can see.

`options` was already on the differ's structured-patchable list; it now means
something. A tool that recomputes its choices costs one message per option
and no new control, which the demo shows by growing its shape list on a click
while `rebuilds` stays at 1.

And the same single-backslash modulo bug the Mac's own sprite editor carries
(`76_spriteed.mst:784` emits `\` where `\` is meant) was reproduced here
through Python escaping and caught the same way — both panes blank, because
one paint raising skips the rest.

---

## Standing rules

1. A sprint is done only when **all** stress modes that exist so far are green.
2. Every bug found post-sprint gets a regression test in the layer where it
   *should* have been caught.
3. Performance numbers are recorded (`docs/PERF.md`), never gated until S15.
4. No new `unsafe` outside `src/oops`, `src/memory`, `src/codecache`,
   `src/jit` (enforced by `#![forbid(unsafe_code)]` elsewhere).
5. SPEC.md is amended (with a Δ note) whenever implementation teaches us the
   spec was wrong — the spec stays true.
