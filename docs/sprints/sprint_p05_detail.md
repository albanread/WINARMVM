# Sprint P5 — FFI on Windows ARM64: winkb, the MS-ABI classifier, world gaps

Objective: guest Smalltalk calls real Win32 functions through the existing
FFI machinery — the A64 trampolines this repo already emits — resolved via
`winkb` (the Windows API knowledge base) instead of `cocoa_data`, with an
argument classifier re-derived for the **MS ARM64** ABI. Lands the
wall-clock (`Time class>>now`) and settles each FFI-dependent world file.
Implements MIGRATION.md §3.5 + the §3.4 variadic delta. The Cocoa tier
(objc dispatch, prims 230–245) stays macOS-gated forever — this sprint
ports the **direct-call tier**, whose trampolines are ISA-correct here by
construction.

## Prerequisites

- P1 (loader's `LoadLibraryA`/`GetProcAddress` resolution — the dlopen/
  dlsym analogue, already present in the loader), P2 (FFI faults land in
  the foreign-AV recovery; a bad call is a report, not a crash).
- Existing: the S20 Tier-1 FFI (`docs/FFI.md`) — pragma, `Alien`
  representation, trampoline emitters, `dispatch_ffi_primitive`;
  `ffi_gen`; the `cocoa_data`-driven resolver design it documents.
- Reference: `WINVM/src/runtime/winkb.rs` (+ its
  `[target.'cfg(windows)'.dependencies] rusqlite = { bundled }`), WINVM
  MIGRATION's hidden-pointer finding (the cautionary tale the classifier
  stance comes from).

## Deliverables

- `src/runtime/winkb.rs` taken from WINVM (Windows-only file — wholesale
  is correct), with its classifier section replaced per D2.
- `Cargo.toml`: the rusqlite windows-dependency (deferred from P0).
- Resolver wiring: the FFI pragma's lookup path goes
  winkb-else-hand-declared on Windows, `cocoa_data`-else-hand-declared on
  macOS (the fallback-to-pragma behavior exists and stays).
- Wall-clock: `Time class>>now` / `Date class>>today` live via
  `GetSystemTimeAsFileTime` (D4).
- The four world FFI files settled (D5) — each enabled, twinned, or
  permanently re-scoped with a written rationale.
- `just gate-p05`.

## Design

### D1. What already works (do not rebuild)

The trampoline emitters produce A64 for THIS machine's ISA; argument
marshalling for the **non-variadic, register-class-simple** signatures
(`ret_class=g/f`, `arg_classes` of g/f) is ABI-identical between Apple and
MS AAPCS64: x0–x7 / v0–v7, ≤16-byte struct returns in x0:x1, >16-byte by
caller-allocated pointer in x8, HFAs in v-regs. The resolver INPUT changes
(winkb vs cocoa_data); the emitted code for simple signatures does not.

### D2. The classifier, re-derived (the sprint's substance)

winkb's classifier was written against **Win-x64** rules and refuses
struct-by-value > 8 B ("hidden pointer"). MS **ARM64** rules differ, and
the classifier must be re-derived, keeping the module's founding stance —
**refuse what cannot be modelled exactly; a misclassified float never
faults, it silently passes garbage**:

| Case | MS ARM64 rule | v1 action |
|---|---|---|
| scalar ≤ 8 B int/pointer | GPR | model (`g`) |
| float/double | SIMD reg | model (`f`) |
| struct ≤ 8 B | one GPR | model |
| struct 9–16 B | two GPRs | model |
| HFA (2–4 same-kind floats) | v0–v3 | model ONLY if the DB carries reliable field layout for it; else refuse (v1 may refuse) |
| struct > 16 B (non-HFA) | by pointer (x8 for returns, normal arg slot for params) | model returns; params refuse in v1 unless needed |
| **variadic function** | ALL variadic args in GPRs — floats too (bit-copied to x-regs); Apple passes them on the STACK | **refuse in v1** unless a needed import is variadic; if one is, implement the GPR-copy rule with a pinning test — never reuse the Apple stack layout |
| COM method | vtable index from DB, `this` in x0 | model (the DB has the two facts needed) |

Every "model" row gets a pinning test that calls a REAL function of that
shape (D3) — the WINVM discipline: the ABI is pinned by execution, not by
reading specs (their hidden-pointer bug produced "a pointer plus garbage
with nothing failing at the point of error"; same bug class exists here in
HFA/variadic form).

### D3. Pinning targets (real imports, stable signatures)

kernel32 exports chosen for shape coverage, called end to end through the
full pragma→resolve→trampoline→execute path: `GetTickCount64` (g ret),
`QueryPerformanceCounter` (pointer arg, BOOL ret),
`GetSystemTimeAsFileTime` (pointer out-param), `MulDiv` (three g args) —
plus one local `extern "C"` Rust fn per classifier row that has no clean
Win32 exemplar (16-byte struct, HFA), since the pin is about the ABI, not
about Win32.

### D4. Wall-clock

`GetSystemTimeAsFileTime` → FILETIME (100 ns ticks since 1601) → the
`Date`/`Time` seam that `world/30_date_time.mst` left skipped
(`PORTING_JOURNAL.md`'s wall-clock entry: the design intent was "an FFI
call, not a bespoke primitive" — on Windows that is exactly
`{{<kernel32 GetSystemTimeAsFileTime: buf>}}`-shaped through this
sprint's path, plus epoch conversion in Smalltalk). The macOS side keeps
its own `time()` route; the world file gains the Windows branch — the ONE
world edit this port makes, flagged for upstream cherry-pick.

### D5. The four gated world FFI files — settle, don't stall

| File | Settlement |
|---|---|
| `ffi_alien` | ENABLE — Alien is `IndexableBytes`-backed and OS-neutral; its exercises re-target kernel32 imports (D3's set) where they named libc |
| `posix_io` | TWIN as `win_io` over msvcrt's `_open/_read/_write/_close` (the CRT is present and C-ABI-plain) OR re-scope to P6+ with rationale if the CRT path proves awkward — decide by trying, one session, timeboxed |
| `socket` | RE-SCOPE (deferred): winsock needs `WSAStartup` lifecycle — real work, own slice, not this sprint |
| `accel` | RE-SCOPE (permanent): Accelerate.framework is macOS; the NEON kernels (`simd_kernels.rs`) already cover the arch story and have their own tests |

Whatever the settlement, the file's gate entry in the world runner says
WHICH and points here — no silently-skipped tests (the WINVM "5891 run
minus four files" bookkeeping pattern).

> **Δ (2026-08-09, measured during P0/P2).** The 12 tests still carrying
> `ignore = "P5"` were surveyed, and **the POSIX group is not uniform** —
> planning P5 as "one resolver reclaims them all" would be wrong:
>
> | symbol | Windows story | reclaimed by |
> |---|---|---|
> | `getaddrinfo`, `freeaddrinfo`, `inet_ntop` (`world/75_dns.mst`) | all three exist in **ws2_32** with the same C signatures | the resolver alone (D2) |
> | `getpid`, `llabs`, `fabs`, and the CRT set | present in the UCRT (some spelled `_getpid` — P0 already renamed one such binding rather than gating it away) | the resolver alone |
> | **`kqueue`** (`IoWorker`) | **no Windows twin exists at all** | **not the resolver** |
>
> `kqueue`/`kevent` have no equivalent primitive on Windows. `IoWorker`
> needs a genuinely different readiness backend — **IOCP** (the idiomatic
> Windows answer, completion-based rather than readiness-based, so the
> calling code inverts) or **`WSAPoll`** (readiness-based and a much smaller
> change, but sockets only, and it inherits `select`'s scaling behaviour).
>
> That is design work with a semantic choice in it, not a table entry, and
> it should be **scoped as its own slice** rather than discovered midway
> through wiring the resolver. Seven of the twelve gated tests hang off it.
> Sequencing suggestion: land the resolver + D2's classifier first, which
> reclaims DNS and the CRT bindings and proves the FFI path end to end, and
> only then take the `IoWorker` backend as a separate piece with its own
> gate — the same posture that kept the P2 trap layer from being tangled
> into P1's loader.

## Implementation order

1. winkb.rs + rusqlite dep; resolver-selection seam compiles both OSes;
   DB-absent path verified (`WinkbError::DbMissing` → pragma fallback —
   the module's own "absence is not an error" contract).
2. D2 classifier + D3 pins, simple rows first (g/f scalars) — the first
   guest-visible Win32 call lands here (`GetTickCount64`).
3. Struct rows + their pins; refusal paths tested (a refused signature is
   a clean guest-visible error naming the reason, not a fallback).
4. D4 wall-clock end to end (`Time now` prints, `Date today` correct).
5. D5 settlements; world-suite bookkeeping updated.
6. `just gate-p05: gate-p03` + this suite (P4 independent — do not chain
   it).

## Pitfalls

- **The DB is a ~90 MB machine-local artifact, not a build input** — the
  build and suite must stay green without it (the fallback path IS the
  no-DB CI story). Never commit it; document the build/refresh command
  from RASM's winkb crate in the status entry.
- **Variadic is the booby trap** (§D2): the Apple trampoline's
  stack-spill layout is WRONG on MS ARM64 in a way that mostly works for
  integer args (first eight land in regs anyway on neither path... no —
  they land differently; it fails loudly only sometimes). The v1 refusal
  exists precisely so nobody discovers this in a guest backtrace.
- **`GetLastError` discipline**: a failing Win32 call's error code is
  per-thread state the next FFI call clobbers — if error reporting is
  wanted, capture it in the trampoline's immediate return path, not
  lazily (v1: expose `GetLastError` as its own import and document the
  hazard; no implicit capture).
- **`extern "system"` vs `extern "C"`**: identical on ARM64 (no stdcall
  legacy), so the DB's calling-convention column can be ignored on this
  target — assert it anyway when reading, cheaply, in case x86 rows leak
  through a query.
- FILETIME epoch/tick conversion belongs in Smalltalk (world file), not
  in a primitive — keep the VM's no-bespoke-clock-primitive stance
  (PORTING_JOURNAL's reasoning).

## Interfaces for later sprints

- A future `win_gui`/native-controls track would ride this resolver +
  COM-vtable modelling (the DB carries vtable indices; CG-analogue work
  becomes possible).
- Sockets (D5) as its own future slice over this substrate.

## Out of scope

- The Cocoa/objc tier, `perform:`-style dynamic dispatch to COM (only
  static vtable calls are modelled), callbacks from C into Smalltalk
  (none exist on the mac side's direct tier either).
- Winsock, Accelerate parity (settled above), any world porting beyond
  the wall-clock file's Windows branch.
