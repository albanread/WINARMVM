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

> **Δ (2026-08-10, P5 — BUILT; what measurement corrected, in the order
> the plan stated it).**
>
> 1. **D2's f-row narrowed: `F` models `f64` ONLY.** The table said
>    "float/double → SIMD reg → model (`f`)", but the marshal shape
>    (`runtime::ffi::marshal_f`) is `f64::to_bits` into a full d-register,
>    while an **f32 travels in the s-register** — the LOW 32 bits — so an
>    f32 classified `F` passes the double's mantissa tail as the value,
>    silently. WINVM's x64 classifier had the same latent hole (f32 → XMM
>    low half) and never noticed because its DB test checked masks, not
>    calls. f32 params AND returns now refuse naming the s-register rule;
>    real exemplar pinned: `D2D1Vec3Length`. Same finding, return side,
>    measured live: **the UCRT materialises C-`int` returns with
>    w-register writes (zero-extend), so `_open`'s −1 reads as 4294967295
>    through `ret: #g` — while Darwin's libc happens to sign-extend, which
>    is the only reason world/61's `close(-1) = -1` works on macOS.**
>    AAPCS64 leaves the upper half unspecified for 32-bit returns; the
>    width is not expressible in the g/f vocabulary, so i32-returning
>    Windows bindings narrow guest-side (`WinIo i32:`,
>    `world/tests/59_win_io_tests.mst`'s header Δ).
> 2. **D2's struct 9–16 B cell said "model"; v1 refuses.** One pragma
>    token maps to one 64-bit slot, so a two-GPR composite is not
>    expressible end to end without either a two-token convention or a new
>    trampoline shape — and the sprint brief's own constraint ("the
>    trampolines work; do not touch the emitters") decides that. The ABI
>    fact itself (16-byte returns in x0:x1) stays execution-pinned by
>    `it_codecache::struct16_return_in_x0_x1`; the classifier's refusal
>    names the rule and that pin. tests_p05's `pin_struct16_roundtrip`
>    row ("both halves intact via trampoline") is therefore satisfied at
>    the published-A64 layer, not the `FfiStubs` layer, whose Rust-side
>    contract returns one u64 by design.
> 3. **The variadic belt cannot fire from this DB build:
>    `is_variadic = 0` for ALL 18,271 rows** (`wsprintfA` — genuinely
>    variadic — records fixed arity 2). The refusal stays as a belt for
>    future builds; the LIVE guard is the strict arity cross-check in
>    `resolve_ffi_symbol` (a tail-passing call site disagrees with the
>    recorded fixed arity and refuses loudly). Pinned so a DB rebuild that
>    starts carrying the mark flips a test.
> 4. **HFA: refused, and the DB could not have modelled it anyway at ≤8 B
>    without field-walking** — `D2D_POINT_2F` is a 64-bit struct whose
>    fields (f32, f32) live in `struct_fields`; a size-only classifier
>    (WINVM's shape) would have modelled it as one GPR while the callee
>    reads s0/s1. The classifier therefore walks fields (recursively,
>    `type_id` is fully populated — 0 NULLs of 66,708) and refuses ANY
>    FP-bearing composite; FP-free ≤8 B structs (HANDLE, BOOL, PSTR,
>    POINT, FILETIME) model as `G`.
> 5. **D4's "the world file gains the Windows branch — the ONE world
>    edit" was short by one file.** P0 already landed the clock-VALUE
>    branch (world/30 → prim 267); what P5 actually had to add for
>    `Time now`/`Date today` to be CORRECT is the ZONE OFFSET —
>    world/81's `localOffsetSeconds` rides `localtime_r`+`tm_gmtoff`,
>    neither of which exists on Windows (no UCRT `localtime_r`; struct tm
>    has no `tm_gmtoff` AT ALL). The Windows arm is
>    `GetTimeZoneInformation` (return value selects the active bias;
>    offsets Bias@0/StandardBias@84/DaylightBias@168 pinned against the
>    DB by `runtime::winkb`'s real-DB test). Verified against the host
>    clock live (BST: +3600, daylight arm taken). Also: `mmap` grew a
>    THIRD guarded branch (world/30's buffer + world/81's buffer + the
>    alien capstone all allocate via `VirtualAlloc` on Windows). So the
>    honest count is three world files with `platformName` branches plus
>    the runner + the new twin file — all flagged for upstream
>    cherry-pick. tests_p05's `filetime_conversion` row tests a guest-side
>    FILETIME→epoch conversion that does not exist in this design (prim
>    267 converts in Rust; P0's decision) — its intent (known instant →
>    known date/time) is carried by ClockCompletionTests' fixed epoch-day
>    vectors, which now run on Windows.
> 6. **The D5 Δ's "reclaimed by the resolver alone" DNS row is half
>    right.** The ws2_32 trio does resolve by name — pinned as a test
>    (`winkb::resolve_export` finds `getaddrinfo`/`freeaddrinfo`/
>    `inet_ntop` with no DB) — but the world's `Dns` path ALSO needs the
>    winsock lifecycle (`getaddrinfo` fails WSANOTINITIALISED without
>    `WSAStartup`), a `gai_strerror` twin (an inline function on Windows,
>    not a ws2_32 export), and a non-mmap `NativeBuffer`. Those belong to
>    the winsock slice; the embed DNS test's gate reason now says exactly
>    that.
> 7. **`GetLastError` discipline gained a measured clause: warm the
>    binding first.** A binding's FIRST call runs resolution, and
>    resolution itself performs Win32 calls (the winkb sqlite open,
>    GetModuleHandle/LoadLibrary/GetProcAddress) that reset the thread's
>    last-error — `fail(); GetLastError()` reads 0 on a cold binding.
>    Warmed, the sequence reads the real code (pinned: `CloseHandle(0)` →
>    6). Docs in `runtime::winkb`; guest-level pin in the win_io twin.
> 8. **Phase-WG addendum (win_gui_design.md §2.2, added mid-sprint):
>    function-pointer (`delegate`-kind) parameters classify `G`, never
>    refuse** — WG passes Rust trampoline addresses (RegisterClassW's
>    wndproc, SetTimer's TIMERPROC); pinned against the real `SetTimer`
>    row. WG0's own first calls (`GetSystemMetrics`, `MessageBeep`) joined
>    the D3 pinning set and user32/ws2_32 joined the no-DB probe list so
>    both DB states behave identically.

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
