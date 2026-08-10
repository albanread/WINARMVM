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
