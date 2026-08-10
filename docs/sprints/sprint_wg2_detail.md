# Sprint WG2 — the WndProc door

Objective: Windows messages reach **Smalltalk**. A Rust trampoline
registered as the window class's `lpfnWndProc` forwards a chosen set of
messages into the UI VM as top-level entries; `WinShell` decides what each
one means and answers either an `LRESULT` or "let `DefWindowProcW` handle
it". A raising handler must not break the window; a *faulting* handler must
not break the process. Implements `win_gui_design.md` §2.2 commitment 4 and
§3 row WG2 — the phase's risk budget, spent here on purpose and early.

This is CG3's twin (`cocoa_gui`'s C6 reverse dispatch), and the comparison
is encouraging: **one uniform four-word signature, no runtime class
synthesis, no IMP shapes, and no exceptions to unwind** — COM/Win32 report
failure by value. What remains hard is re-entrancy and crash safety, which
is where this sprint's budget goes.

## Prerequisites

- **WG1 landed**; its Δ is required reading. Four items bear directly:
  - **Δ1: do not reach for `register_hosted_worker`.** It mints an entry
    in a *primary's* registry and WG2 still has no primary. `VmHandle::boot`
    on `main` is the arrangement.
  - **Δ2: a remembered HWND is not a window** — `IsWindow` gates every
    cached handle. The door will cache; apply the rule.
  - **Δ3: the pump must not ask the VM anything per message.** The door
    crosses into Smalltalk because a message *means* something. Anything
    the host merely needs to *know* about its own window, Win32 answers
    more cheaply than the VM can. This is the sprint's governing
    performance rule.
  - **Δ8: `eval` answers inline** (the VM is on the pump's thread), so
    every gate item below reads a number rather than sleeping and hoping.
- **WG0 Δ8: nothing publishes a Rust function's address to Smalltalk.**
  WG2 must build that channel (D2) — it was flagged a sprint early
  precisely so it is planned, not discovered.
- P2's trap/recovery layer (VEH + non-unwinding AArch64 setjmp/longjmp),
  which WG1 already proved recovers a guest fault *with the window shown*
  while the pump keeps running.

## Deliverables

- `win_gui`: the `extern "system"` trampoline, the re-entrancy guard, the
  allowlist, and a primitive publishing the trampoline's address.
- `world/91_winui_shell.mst`: `WinShell class>>window:message:wParam:lParam:`
  and the per-message handlers; the class registration switches from
  `DefWindowProcW` to the door's address.
- `world/tests/62_winui_door_tests.mst`.
- `just gate-wg2`; a `docs/gallery-win/` shot showing a Smalltalk-handled
  resize.

## Design

### D1. What crosses, and what does not — the allowlist

**Do not route every message.** `WM_MOUSEMOVE`, `WM_NCHITTEST`,
`WM_SETCURSOR` and friends arrive in storms; sending each through a VM
entry would make the door the slowest thing in the system and multiply the
re-entrancy surface for no gain. WG2 routes a small, stated set and
`DefWindowProcW`s the rest:

| message | why it crosses |
|---|---|
| `WM_CLOSE` | the app decides whether to close |
| `WM_DESTROY` | must `PostQuitMessage(0)` — **WG1 explicitly deferred this here** |
| `WM_SIZE` | layout is Smalltalk's job from WG3 on |
| `WM_COMMAND` | menus and controls (WG3+) |
| `WM_KEYDOWN` / `WM_CHAR` | the editor and shortcuts (WG5+) |
| `WM_PAINT` | **not yet** — WG2 keeps `DefWindowProcW`'s erase; painting is WG3/WG4 |

The allowlist is a Rust-side constant *and* a Smalltalk-side fact: the door
asks Smalltalk only for messages on the list, and `WinShell` answers
`#defwindowproc` for anything it does not itself handle, so the two can
disagree safely in the direction of doing less.

### D2. The address channel (WG0 Δ8)

Smalltalk cannot name a Rust function. Add one primitive —
`primWndProcAddress`, answering the trampoline's address as an Integer —
and register it as `lpfnWndProc` through the existing arena-built
`WNDCLASSW`. Keep it **policy-free**, exactly as WG0's winkb primitives
are: it answers an address; it knows nothing about windows.

> The alternative — having Rust register the class — was rejected: it would
> move window creation out of Smalltalk and break §2.2 commitment 4 and
> WG1's D1 ownership line for the sake of one integer.

### D3. Re-entrancy — the sprint's real hazard

`DispatchMessageW` can be re-entered: `DefWindowProcW` generates nested
messages, `SendMessageW` from inside a handler runs another wndproc call on
the same thread, and a modal drag loop pumps internally. A VM top-level
entry inside a VM top-level entry is **not** safe.

**The guard**: a thread-local depth counter in Rust. On entry, if the depth
is already non-zero, the door does **not** call the VM — it answers
`DefWindowProcW` directly. Nesting is therefore always safe and always
degrades to default behaviour rather than to corruption.

Two things follow, both worth stating so WG3 does not trip over them:

- A Smalltalk handler that itself calls `SendMessageW` will see the nested
  message handled by `DefWindowProcW`, not by Smalltalk. That is the
  correct trade for v1; if a later sprint needs true re-entrant dispatch it
  needs a queue, not a deeper counter.
- The guard must be released on **every** path out, including the fault
  path — see D4.

### D4. Crash safety — three failure modes, three answers

| failure | answer |
|---|---|
| Smalltalk **raises** (`error:`, DNU) | the handler's result is an error; the door answers `DefWindowProcW` and **the next message still dispatches**. The window must not become inert. |
| Smalltalk **faults** (wild deref) | P2's VEH + longjmp recovers the VM; the door answers `DefWindowProcW`; the pump — which never learns it happened — keeps running. WG1 proved this shape with the window shown. |
| The door is re-entered mid-recovery | the depth guard is decremented by an RAII drop, so a longjmp that skips ordinary returns still leaves the counter honest. **A guard that leaks on the fault path silently disables the door forever after** — the failure would look like "Smalltalk stopped receiving messages", with nothing in any log. |

### D5. Latency — measured, because the design promised it

§3's row for WG2 says "latency of the door measured and recorded". Measure
two numbers and put them in the sprint's Δ:

1. **Door round-trip**: `QueryPerformanceCounter` either side of the VM
   entry, for a `WM_SIZE` handled in Smalltalk, warm.
2. **The same message `DefWindowProcW`'d** (allowlist off), as the baseline.

The ratio is what tells WG3 whether per-message Smalltalk is affordable for
layout, or whether a `WM_SIZE` storm needs the coalescing the design doc's
risk table already anticipates ("coalesce in the shell, dispatch the settled
state").

## Implementation order

1. D2's primitive + registering the door with the class; the trampoline
   immediately calls `DefWindowProcW` for everything. **The window must
   behave exactly as WG1's did** — same gate, same shots. Prove the door
   is transparent before it does anything.
2. The depth guard + its RAII drop, with a unit test that forces nesting.
3. `WM_CLOSE`/`WM_DESTROY` into Smalltalk; `PostQuitMessage` from the
   Smalltalk side. Clean exit still 0.
4. `WM_SIZE` — the first message whose *meaning* is Smalltalk's. Gate: a
   scripted resize changes a value `WinShell` computed.
5. Raise + fault tests (D4), then D5's measurements.

## Pitfalls

- **`WM_DESTROY` arrives during `DestroyWindow`, which the door may itself
  have called.** That is a nested dispatch by construction — so the
  `PostQuitMessage` path must work *under the depth guard*, or arrange for
  `WM_DESTROY` to be handled outside it. Decide deliberately and write down
  which.
- **The trampoline must never panic across the FFI boundary.** A Rust
  `panic!` unwinding into Win32's dispatcher is undefined behaviour; catch
  at the boundary (or ensure the guard's drop runs) and answer
  `DefWindowProcW`.
- **`lpfnWndProc` is captured at `RegisterClassW` time**; changing it later
  needs `SetWindowLongPtrW(GWLP_WNDPROC)`. Register the door *first* rather
  than retro-fitting it.
- WG1 Δ2 again: the door may hold an HWND; gate it with `IsWindow`.
- WG1 Δ4: the `−1` bug still lives in `gui/src/shell/win.rs::run`. Not
  WG2's to fix, but do not copy that loop.

## Out of scope

Controls, painting, layout, menus (WG3+). True re-entrant dispatch. The
primary VM. COM. Any message not on D1's list.

---

> **Δ (2026-08-10, WG2 — BUILT; what measurement corrected).** The door is
> real: six messages cross, `WM_SIZE`'s lParam is decoded in Smalltalk and
> equals `GetClientRect` (884×561 after a scripted 900×600 frame), 20 raising
> handlers interleaved with 20 good ones leave `messagesSeen` 40 and
> `sizeCount` 20, five real `ACCESS_VIOLATION`s inside handlers are recovered
> with the next message still dispatching, a handler's own `SendMessageW` is
> declined and answered by `DefWindowProcW` with the depth counter back at 0,
> `WM_DESTROY` posts the quit **from Smalltalk** and the process exits **0**.
> **D5: 8.79 µs door round trip vs 57 ns `DefWindowProcW`, release, warm, 200
> samples each — a ratio of ~154×.** Sixteen corrections, in the order WG3
> will meet them.
>
> 1. **"`win_gui`: … and a primitive publishing the trampoline's address" is
>    not buildable, and the door belongs in the core crate anyway.** Primitive
>    numbers and the `PRIMITIVES` table are `macvm`'s; a downstream crate
>    cannot add a row. Worse, a door that existed only when one particular
>    *binary* was the host would leave `62_winui_door_tests.mst` unable to see
>    the address under the plain CLI, and would make the world layer's
>    behaviour depend on which `main` linked it. So the trampoline, the
>    allowlist and the guards live in **`src/runtime/win_wndproc.rs`** —
>    exactly where the Cocoa twin already is (`runtime::objc_delegate` is
>    core; `cocoa_gui` is the thin host) — and `win_gui` keeps the pump,
>    publishes the VM pointer, and stays thin, which is its own stated
>    deliverable. Primitive **272**, `WinApi class>>primWndProcAddress`.
> 2. **D3 names one re-entrancy source. There are TWO, and the second fires on
>    the very first run.** `CreateWindowExW`, `SetWindowPos` and
>    `DestroyWindow` **send** messages synchronously, and `openMain` is called
>    from `vm.eval` — so the trampoline is invoked *inside* a live top-level
>    entry with the depth counter legitimately **0**. That is the same
>    corruption D3 exists to prevent, arriving by a door D3 does not name:
>    `deopt_trap::claim_jmp_slot` hands out **one slot per thread** (keyed by
>    thread id, reusing the caller's own), so a nested `sigsetjmp` overwrites
>    the outer `eval`'s recovery buffer and its single idle-baseline
>    watermark; a later fault in that `eval` would `longjmp` into a returned
>    frame. Measured on the first run: **`busy=2` during `openMain`** (the
>    `WM_SIZE` from `CreateWindowExW` and the one from `applyDips`) and
>    **`busy=30` during `cycle: 10`**, with `entries=0` in both. The answer is
>    a second guard — `win_wndproc::vm_busy` / `BusyGuard`, which the host
>    wraps around **every** `eval`/`exec`, enforced by
>    `every_vm_entry_is_bracketed` rather than remembered. **Its right
>    long-term home is `eval`/`exec` themselves**, so every host gets it
>    without cooperating; that is a core change with a blast radius bigger
>    than this sprint's and it is WG3's to consider.
> 3. **`tests_wg2.md` item 2's mechanism cannot work, and the reason is item 2
>    of this list.** "A scripted resize (`SetWindowPos` via FFI from a doit)"
>    sends `WM_SIZE` synchronously, so from a doit the wndproc runs inside
>    `vm.exec` and the door correctly refuses it — the resize happens, and
>    `DefWindowProcW` answers. **A message that reaches Smalltalk must
>    originate outside every VM entry**, which is where every real one does:
>    the pump, or the modal move/size loop Windows runs while a user drags the
>    frame. The gate drives resizes from the host's control **drain** (`gui
>    resize <w> <h>`), which is that same place. `WinShell
>    resizeWindowTo:by:` still exists — WG3's layout code wants it — but a
>    doit calling it cannot observe its own `WM_SIZE`. **WG3's layout tests
>    inherit this constraint entirely.**
> 4. **The depth guard must cover the VM entry ONLY, never the
>    `DefWindowProcW` call — and that one brace is the whole answer to this
>    sprint's `WM_DESTROY` pitfall.** With the tight scope: `WM_CLOSE` enters
>    at depth 0→1, `WinShell` answers `#defwindowproc`, the guard drops at
>    1→0, *then* `DefWindowProcW` runs `DestroyWindow`, and the `WM_DESTROY`
>    it provokes is a **fresh top-level entry** that reaches Smalltalk. Had
>    the guard wrapped the default call, `WM_DESTROY` would have been declined
>    as nested and the loop would never have ended. The matching guest-side
>    rule is written into `WinShell`: **a handler must not call
>    `DestroyWindow`**, because that would nest it again from the other side.
> 5. **"WG2 should delete the host's liveness backstop" is wrong, and keeping
>    it naively is also wrong.** Deleting it makes an `onDestroy` that raises
>    an unkillable process pumping an empty queue in front of a window that no
>    longer exists — WG1's Δ 2 hang, re-earned. Keeping it as WG1 wrote it
>    made it fire on **every** clean close (the pump's `IsWindow` check runs
>    *after* the dispatch that destroyed the window, so it always sees the
>    corpse) and print the opposite of what had happened, which would make
>    gate item 3 unprovable. The fix is `win_wndproc::guest_handled_destroy()`
>    — set when a `WM_DESTROY` VM entry answers an LRESULT — so the two paths
>    print different lines and the gate asserts it saw Smalltalk's and did NOT
>    see the backstop's. Safety and provability, rather than a choice.
> 6. **`gate-wg1` has been failing since `snap` was corrected, and WG2 had to
>    fix it because `gate-wg2` chains it.** `snap` sizes its bitmap with
>    `GetWindowRect`, and `gui/src/shell/snap.rs`'s own doc records exactly
>    why the change was made: the older client-sized version "measured correct
>    — client 900×600, PNG 900×600, **the gate's equality satisfied** — while
>    actually containing the titlebar plus only the top 560 px of the client
>    area". The capture was fixed. **The gate that depended on the defect was
>    not**, so it has been comparing **916×639 against 900×600** ever since,
>    and its pixel check reads byte 49 — the top-left of the *window*, i.e.
>    DWM's frame (**245,241,247**), not the client fill (243,243,243). WG1's Δ
>    reports a 900×600 capture *and* a 243,243,243 first pixel, and those are
>    both true only of the pre-fix `snap`; neither is true now. Both gates now
>    compare against the **window** rect (a new `WG1 window` / `WG2 winrect`
>    line from `WinShell windowRect`) and read the **centre** pixel, which is
>    client area for any window with a titlebar — 243,243,243, measured, in
>    both. **The centre offset needs real arithmetic**: the writer emits stored
>    deflate blocks capped at 65535 bytes, so a 5-byte header appears every
>    65535 raw bytes and each scanline carries a filter byte, making the naive
>    fixed `49` wrong for every scanline past the first ~17. The general
>    lesson, which is bigger than one recipe: **a gate whose assertion outlives
>    the thing it asserts about goes green on the wrong evidence, then red for
>    the right reason, and looks like a regression either way.**
>    §3.1's own wording — "it captures *any* HWND's **client area**" — is the
>    origin of the confusion and is wrong in the same direction: the capture is
>    the whole window including the DWM frame, which is why the gallery shot
>    shows a titlebar. That is a better shot, and it is not what the sentence
>    says.
> 7. **The door's probe counters must be per-thread, not process-wide.** The
>    unit tests are before/after comparisons ("an off-list message must not
>    reach the VM"), `cargo test` runs several VMs in one process on parallel
>    threads (WG0 Δ 11), and a `static AtomicU64` lets one test move another's
>    counter — a flake by construction. Thread-local is also the honest model
>    (a wndproc runs on the thread that owns its window) and cheaper on a path
>    that runs per message. Everything that reads them — the control drain —
>    is on the pump thread, so nothing is lost.
> 8. **`dispatch_callback` has one `u64` return and no out-of-band channel**,
>    so "the handler answered `#defwindowproc`" and "the handler blew up" must
>    arrive as the same value. The sentinel is `0x8000_0000_DEFB_DEFB`, which a
>    `SmallInteger` **cannot** produce (61-bit value space cannot reach bit
>    63) — unreachable by construction rather than by convention, with a test
>    that says so. Any WG3 handler answering an LRESULT is unaffected,
>    including negative ones (`WM_NCHITTEST`'s family), which are checked.
> 9. **`SmallInt::new` PANICS out of range** and a wndproc must never panic
>    (a Rust unwind into Win32's dispatcher is UB). `wParam` is a `UINT_PTR`
>    and can exceed 61 bits in principle, so all four words cross through
>    `try_new` and an unrepresentable message answers `DefWindowProcW`. The
>    whole trampoline body is additionally inside `catch_unwind`; the caught
>    count is reported and must stay 0 (it is).
> 10. **Rust must not hold a `&mut VmHandle` across `DispatchMessageW`.** The
>     trampoline re-borrows the same handle from the published pointer, so a
>     `&mut` held by the pump would alias it. `pump` now carries `*mut
>     VmHandle` and materialises a `&mut` only inside the control drain, where
>     the door provably is not running. The handle is `Box`ed so the address
>     is stable by contract rather than in practice.
> 11. **`publish_ui_vm` is the mechanism, and it is not
>     `register_hosted_worker`.** WG1 Δ 1 said not to reach for the latter and
>     WG2 did not; the CG3 thread-local `*mut VmHandle` is exactly the door's
>     shape and needs no primary. It is published **before** `openMain`
>     (`CreateWindowExW` calls the door before it returns an HWND) and
>     **nulled** after the loop ends, so a message delivered during teardown
>     finds a null door rather than a freed VM.
> 12. **The allowlist's six numbers are TRANSCRIBED in Rust, necessarily**, and
>     that is the one place in this layer where D2's "never transcribe" cannot
>     hold: a `const` cannot query SQLite and the core crate must build on a
>     machine with no `windows_api.db`. The compensating control is data, not
>     care: `testAllowlistMatchesWinkb` asks winkb for all six by name and
>     asserts the literal values, so a seventh added with a wrong number fails
>     in-language. All six matched on the first run.
> 13. **The vendored RUSTTCL is smaller than a gate author expects.** It has
>     `while` and `foreach` but **no `for`**; `expr` needs braces (`expr {a +
>     b}`, not `expr a + b`) and has **no `%`**; there is **no `eq`/`ne`** and
>     **no `string` command**. Every accumulation therefore moved into
>     Smalltalk — `messagesSeen` counts arrivals, `sizeCount` counts
>     successes — which is a better test than a Tcl-side tally anyway, because
>     it is the guest's own record of what happened to it. WG3's scripts
>     should start from that assumption.
> 14. **Gate item 8's frozen integers cannot hold, for the reason WG1's Δ 12
>     already gave.** Measured: world **7723 → 7762** with the database and
>     **7535 → 7540** without, **0 failed** in both; Rust grows by the door's
>     own 12 unit tests plus `win_gui`'s 3. "Nothing lost, nothing failing" is
>     the invariant; the integer is a reading.
> 15. **`dispatch_callback`'s recovered-fault line prints `[cocoa-delegate]`
>     on Windows.** Cosmetic, shared with the Cocoa host, and genuinely
>     confusing in a `macvm-winui` log that has no delegates in it. Left
>     alone rather than changed under a sprint that does not own that file —
>     recorded so WG3 does not spend ten minutes on it.
> 16. **`PostQuitMessage` is `void` and needs `ret: #v`.** The resolver checks
>     the declared return class against winkb and would refuse `ret: #g`;
>     reading x0 after a void callee answers whatever was in it. `#v` is
>     already in the pragma vocabulary (world/61a uses it for vDSP) and this
>     is the first Win32 use of it.
>
> **D5, and what it means for WG3.** Warm, 200 `WM_SIZE` samples each, same
> process, same QPC clock, same call site:
>
> | build | door round trip | `DefWindowProcW` baseline | ratio |
> |---|---|---|---|
> | **release** | **8 793 ns** | **57 ns** | **~154×** |
> | debug | 540 925 ns | 52 ns | ~10 400× |
>
> The ratio is large and the absolute number is small, and it is the absolute
> number that decides. 8.8 µs per message means a live frame-drag — which
> produces `WM_SIZE` at roughly the compositor's rate, order 10²/s — spends
> about **one millisecond per second** in Smalltalk. Per-message Smalltalk is
> comfortably affordable for layout, and the design doc's coalescing risk row
> is not triggered by `WM_SIZE`. It *would* be triggered by anything in the
> storm class: at 154× the default path, a message arriving at 10⁴/s would
> cost ~9% of a core before the handler did anything useful, which is the
> quantitative reason `WM_MOUSEMOVE`/`WM_NCHITTEST`/`WM_SETCURSOR` stay off
> the allowlist and must stay off it. **Record the debug figure too, because
> it is what a developer will see**: 61× slower than release, and a WG3
> reading 540 µs in a debug session should not conclude the door is unusable.
> One caveat worth handing on: the 8.8 µs is the *whole* round trip including
> the handler, and `WinShell>>window:message:wParam:lParam:` currently does up
> to six `WinApi constant:` Dictionary lookups per message before it dispatches.
> Caching those six numbers in class variables is the obvious first
> optimisation and it is WG3's, not a defect here.
>
> **Three non-corrections worth recording.** The **two-sided allowlist worked
> exactly as designed** and was worth the words: `WinShell` answers
> `#defwindowproc` for everything it does not handle, the door treats every
> non-Integer answer as the default, and the halves were deliberately
> disagreed with (three allowlisted messages — `WM_KEYDOWN`, `WM_CHAR`,
> `WM_COMMAND` — are recorded and then defaulted) with no effect on the
> window. **P2's recovery is as good as D4 assumed and better than it
> claimed**: five consecutive `ACCESS_VIOLATION`s inside a live handler, on a
> shown window, under a running pump, with the control channel answering
> throughout and the process exiting 0 on request. And **the RAII depth guard
> survives the `longjmp`** — verified not by argument but by
> `depth_returns_to_zero_after_a_recovered_guest_fault` and its native-fault
> twin, which drive a real fault through the real trampoline into a real VM
> and then assert the *next* message still reaches Smalltalk. That second
> assertion is the one that matters: a guard that leaked would pass every
> "did it survive" check and fail only that one.
