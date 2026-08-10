# Sprint WG0 — FFI probe: user32 by hand

Objective: prove the whole chain a Smalltalk-authored Windows UI stands on —
guest Smalltalk → winkb-resolved import → A64 FFI trampoline → a real
user32/kernel32 call — before any window, loop, or door exists. Ends with a
window class registered, a (hidden) window created and destroyed, and a
`WNDCLASSW` built byte-by-byte from winkb's struct offsets via `Alien`.
Implements `win_gui_design.md` §2.2 (the handle layer's floor) and §3 row
WG0. Deliberately tiny: every later WG sprint assumes this one's facts.

## Prerequisites

- **P5 landed**: `resolve_ffi_symbol` resolves through winkb (or
  LoadLibraryA fallback); the ARM64 classifier models `g` scalars,
  pointers — including function-pointer params as plain `g` (the addition
  sent to P5 for exactly this sprint) — and 9–16-byte structs; pinning
  tests include `GetSystemMetrics` and `MessageBeep`.
- Existing: `Alien` (IndexableBytes-backed, typed accessors), the FFI
  pragma syntax, `world/tests` SUnit-lite, the `platformName` guard
  pattern (P0), `load_list` (CG1, tested on Windows).

## Deliverables

- `world/90_winui_probe.mst` — the probe classes, **loaded only via a new
  `world/winui.list`** (the `cocoaui.list` mechanism verbatim): a
  `WinProbe` class carrying the FFI pragmas and the doits below. Nothing
  enters `world.list`; the base world stays byte-identical.
- `world/tests/60_winui_probe_tests.mst` + a `winui`-guarded entry in the
  runner (extend `99_run_all.mst`'s `platformName` pattern — these run
  ONLY on `#windows`).
- A `WinRef` **sketch only** (class + handle wrapping + `printOn:`) — the
  full protocol is WG2's; here it exists so the probe's window handle has
  an honest home.

## Design

### D1. The call ladder, in proving order

1. `GetSystemMetrics(SM_CXSCREEN)` — int in, int out, zero risk; already
   pinned by P5. First guest-visible user32 value.
2. `MessageBeep(MB_OK)` — BOOL semantics, side effect audible.
3. `GetModuleHandleW(NULL)` — the HINSTANCE every registration needs;
   NULL pointer arg, pointer out.
4. `RegisterClassW(&wc)` — the sprint's substance: a **16-entry struct by
   layout**. Build the `WNDCLASSW` in an `Alien`, each field written at
   the offset **queried from winkb's `struct_fields`** (never hardcoded —
   the test asserts the queried offsets against the two invariants we can
   check locally: total size, and `lpfnWndProc` at offset 8). For v1 the
   wndproc field takes **`DefWindowProcW`'s own address** (resolved like
   any import) — no Rust trampoline yet, no dispatch; WG2 owns that door.
5. `CreateWindowExW(...)` — created **hidden** (no `WS_VISIBLE`): WG0
   proves handles, not pixels; a visible window belongs to WG1 where a
   loop can serve it. Returns the HWND into the `WinRef` sketch.
6. `IsWindow(hwnd)` → true; `DestroyWindow(hwnd)` → true;
   `IsWindow(hwnd)` → false — the lifecycle round-trip that IS the gate.

### D2. Constants come from the database

`SM_CXSCREEN`, `MB_OK`, `WS_OVERLAPPEDWINDOW`, `CW_USEDEFAULT` — resolved
from winkb's 97k `constants` at world-load time, not transcribed into
Smalltalk source. One helper (`WinProbe class >> constant:`) with a
guest-visible error naming the constant when absent. Transcribing numbers
by hand is how a port drifts; the database is present, use it.

### D3. Wide strings

Win32 is UTF-16 (`...W` entries). The probe needs class-name and title
strings: one `String>>asUtf16Alien` helper (append the NUL terminator;
answer the Alien) written here, tested here, reused forever after. Do NOT
take the `...A` entries as a shortcut — the design doc's Windows-materials
stance includes the character set.

## Implementation order

1. `winui.list` + empty probe class; loads via `load_list`, world suite
   untouched.
2. D2 constants helper + its absence error, tested against known values.
3. D3 `asUtf16Alien`, round-trip tested (write, read back, length).
4. Ladder steps 1–3 as doits + tests.
5. Step 4–6: struct build, register, create hidden, lifecycle asserts.
6. Un-guard the world-runner entry; full suite both DB states (the
   probe's winkb lookups fail CLEANLY to pragma fallback when the DB is
   absent — which for these entries means the test SKIPS with a named
   reason, same announced-skip discipline as `posix-only:`).

## Pitfalls

- **`RegisterClassW` fails with `ERROR_CLASS_ALREADY_EXISTS` on re-run** —
  the probe unregisters (`UnregisterClassW`) in an ensure-style cleanup,
  and the test tolerates the already-exists error on the second
  registration by asserting the class is usable either way. Test suites
  re-run in one process; a probe that passes once and fails forever after
  is a trap for the next sprint.
- **`CreateWindowExW` on a non-main thread**: fine for a hidden,
  loop-less, immediately-destroyed window — but say so in the comment,
  because WG1 moves window ownership to the hosted main-thread VM and
  someone will wonder why WG0 got away without it.
- **GetLastError discipline** (P5's rule): read it immediately after the
  failing call in the SAME primitive round-trip if error detail is
  wanted; v1 asserts success and names the call on failure.
- The struct-offset test must not assert all 16 winkb offsets against
  hardcoded numbers — that would just re-transcribe the headers. Assert
  the two checkable invariants (size, one landmark offset) and that all
  16 queries ANSWER; trust the database for the rest — it is the design.

## Out of scope

- Any message loop, any visible window, any WndProc other than
  `DefWindowProcW` (WG1/WG2). Mica/DWM calls (WG1). Controls (WG3).
  The `win_gui` crate itself — WG0 is world-side only, driven through
  the existing `macvm` CLI.

---

> **Δ (2026-08-10, WG0 — BUILT; what measurement corrected).** The ladder
> ran end to end on the first machine it was pointed at:
> `GetSystemMetrics(SM_CXSCREEN)` = 1920, `MessageBeep(MB_OK)` = true,
> `GetModuleHandleW(NULL)` = `0x7ff66fe20000`, `DefWindowProcW` =
> `0x7ffbaedaf790`, `RegisterClassW` → `CreateWindowExW` (hidden) → HWND
> `0x302b6` → `IsWindow` true → `DestroyWindow` true → `IsWindow` false.
> Nine things the plan got wrong, in the order WG1 will meet them.
>
> 1. **"Zero Rust changes if P5's surface suffices" — it did not, and the
>    gap is exactly one shape.** P5 wired `runtime::winkb` into the
>    RESOLVER only: `lookup_function` feeds `resolve_ffi_symbol`, while
>    `lookup_constant` and `lookup_struct_field` — both built, both tested
>    — were reachable from Rust and nothing else. From Smalltalk the 97,402
>    constants and 66,708 struct fields did not exist, so D2 ("resolved
>    from winkb's constants, not transcribed") and the winkb-queried
>    `WNDCLASSW` were unreachable. **There is no world-side substitute**:
>    the FFI resolves by NAME and never hands the guest an address, and no
>    call-through-an-address primitive exists, so even a guest-side
>    `LoadLibraryA` + `GetProcAddress` of some sqlite entry point could not
>    be *called*. The alternative was world/81's posture — transcribe the
>    numbers with a citation, pin them in a Rust test — which is precisely
>    what D2 and pitfall 4 forbid and which does not scale to a UI wanting
>    hundreds of constants. WG0 therefore adds **four primitives and one
>    winkb function, and no policy**: 268 `primWinkbAvailable`, 269
>    `primWinkbConstant:`, 270 `primWinkbStructField:field:`, 271
>    `primWinkbStructSize:` (+ `winkb::lookup_struct_size`). They answer
>    what the database says, or nil; every decision about the answer is
>    made in Smalltalk. Nothing about them knows what a window is.
> 2. **`WNDCLASSW` has TEN members and is 72 bytes.** D1 step 4 calls it "a
>    16-entry struct" and tests_wg0 asks for "all 16 field-offset queries";
>    16 belongs to no Win32 window-class struct. `WNDCLASSEXW` — which WG0
>    does not use — has twelve members and 80 bytes. Pinned in
>    `runtime::winkb`'s real-DB test.
> 3. **"Total size" is not locally checkable from offsets alone**, which is
>    why 271 exists. The two invariants actually asserted are better than
>    the two the plan named: `lpfnWndProc`@8 (the only offset in the struct
>    derivable from first principles — a 4-byte `style`, then LP64 pointer
>    alignment), and **`size == lpszClassName's offset + 8`**, which
>    cross-checks `types.size_bits` against `struct_fields.byte_offset` —
>    two independent database columns agreeing, not a tautology. Plus:
>    every one of the ten queries answers, and the offsets strictly
>    increase in declaration order. No offset is compared to a hardcoded
>    number anywhere.
> 4. **The "deliberately wrong struct SIZE" stress cannot be written for
>    this API.** `WNDCLASSW` has no size member to lie about — only
>    `WNDCLASSEXW` carries `cbSize`. The equivalent deliberate corruption
>    is a NULL `lpszClassName`, which `RegisterClassW` rejects; the test
>    asserts the failure is a clean guest error naming the call *and* that
>    the next legitimate registration still works.
> 5. **A direct `Alien` cannot cross the FFI, so "build the `WNDCLASSW` in
>    an `Alien`" needed a second sentence.** `Alien new:` is ordinary
>    GC-managed heap storage that a scavenge relocates; every byte Win32
>    reads must be GC-stable (world/61a's NativeBuffer rule). WG0 bump-
>    allocates the struct and every string out of one 64 KiB `VirtualAlloc`
>    arena. Related: **`Alien` publishes no address accessor to the guest
>    at all**, so `String>>asUtf16Alien` alone could never have served the
>    FFI. It stays a pure, database-free, platform-free conversion
>    answering a direct Alien (and is tested as such, including the
>    surrogate-pair path for astral code points); `WinProbe nativeUtf16:`
>    is the copy into the arena that answers an address.
> 6. **A test file for a layered world cannot guard itself at run time.**
>    An unresolved capitalised name in a method body is a COMPILE error
>    (`frontend/codegen.rs` auto-declares a global only for a top-level
>    write) and the guest has no dynamic `Smalltalk at: #WinProbe`. So
>    `60_winui_probe_tests.mst` cannot load unless `90_winui_probe.mst`
>    loaded first, and **`world/tests/tests.list` therefore names
>    `../90_winui_probe.mst`** — the one place outside `winui.list` that
>    loads the layer, and not by choice. `world.list` is untouched and the
>    base world stays byte-identical, which was the actual requirement.
>    Every later WG sprint inherits this ordering rule.
> 7. **The two classes in a layer will name each other**, and a reference
>    to a class not yet installed is a compile error: `WinRef` is
>    forward-declared with its instance variables and reopened with its
>    methods below `WinProbe` (world/49_galaxigans.mst's pattern;
>    cocoaui.list's own comment records the load-ORDER half of the same
>    problem).
> 8. **WG2 will need a channel WG0 proves does not exist.** WG0 gets
>    `DefWindowProcW`'s address the only way a guest can — `GetModuleHandleW`
>    + `GetProcAddress`, both ordinary FFI calls. That works because the
>    address belongs to a DLL export. **The WndProc door's trampoline is a
>    Rust function, and no path publishes a Rust address to Smalltalk**, so
>    WG2 needs its own primitive (or a `WinShell class>>doorAddress`-shaped
>    verb) for it. Plan for it; do not discover it.
> 9. **Two small ABI facts, both measured rather than assumed.**
>    `CW_USEDEFAULT` is a `constants`-table `int` row, i.e. a
>    SIGN-EXTENDED `u64` (`0xFFFF_FFFF_8000_0000`); primitive 269 answers
>    it as C's own −2147483648, which is the only SmallInt-representable
>    form and marshals to identical register bits (`marshal_g` re-widens
>    with `value() as u64`). And `CreateWindowExW`'s **twelve** g-class
>    arguments are the first call on either platform in this port to spill
>    past x0–x7 into `codecache::ffi_stubs`' stack slots against a Win32
>    entry point: MS ARM64 and Apple AAPCS64 agree on 8-byte stack slots
>    for 8-byte values, and it works unchanged.
>
> 10. **`GetLastError` cannot be a control input under this VM — only a
>     diagnostic.** Pitfall 3 and P5's own discipline say to read it as the
>     immediately-next FFI call through a warmed binding, and WG0's first
>     `ensureClassRegistered:` did exactly that: zero atom → GetLastError →
>     tolerate `ERROR_CLASS_ALREADY_EXISTS`. It passed standalone and
>     **failed under the JIT**, reading 203 (`ERROR_ENVVAR_NOT_FOUND`) — a
>     leftover from a call nobody in the guest made. P5's clause "nothing
>     between two CACHED guest FFI sends performs a Win32 call" is true of
>     *marshalling* and false of the *VM underneath it*: tier-1
>     compilation, the W^X guard's `VirtualProtect` flips and code-cache
>     `VirtualAlloc` all run between guest sends and all reset the thread's
>     last error, and no guest-side sequencing can close that window. The
>     fix is to ask Win32 the question directly — `GetClassInfoW` answers
>     "is this class usable", which is what the sprint doc's own wording
>     ("assert the class is usable either way") always meant. **Any WG code
>     that branches on `GetLastError` is fragile in this exact way.** Read
>     it to report with; never to decide with.
> 11. **A window class is PER-PROCESS state, and this suite runs several
>     VMs in one process.** Two `it_world` tests each boot a VM and each
>     ran the ladder in parallel threads, so one VM's `UnregisterClassW`
>     could pull the class out from under another VM's `CreateWindowExW`.
>     The probe class name is therefore unique per VM (it carries the
>     arena's base address, which nothing frees), while staying stable
>     WITHIN a VM so the re-registration pitfall is still genuinely
>     exercised twenty times over. WG1 inherits the general form: window
>     classes, atoms, the message queue and the last-error value are all
>     per-process or per-thread state that the two-VM split (§2.2) has to
>     reason about explicitly — the heaps are separate, the Win32 namespace
>     is not.
>
> One non-correction worth recording: the hidden window really is created,
> proven and destroyed on an arbitrary (non-main) thread, including a
> `cargo test` worker — because it is hidden, pumps no messages and dies
> immediately. All three of those stop being true at WG1.
