# Sprint WG1 — the window and the loop

Objective: a **visible, native Windows 11 window** — Mica backdrop,
dark-mode-aware titlebar, correct DPI — created by Smalltalk, owned by a UI
VM running on the process's real main thread, pumping its own message loop,
closing cleanly. Ends with the RUSTTCL control channel pointed at it, so
every later WG gate is a script rather than a request for someone's eyes.
Implements `win_gui_design.md` §2.1 (materials), §2.2 commitments 1 and 3,
§3 row WG1, and §3.1 (the capture channel, already built — inherited here,
not rebuilt).

## Prerequisites

- **WG0 landed** and its Δ is required reading — eleven corrections, and
  WG1 meets at least six of them. The load-bearing ones:
  - **`GetLastError` is a diagnostic, never a control input.** The VM's own
    `VirtualProtect`/`VirtualAlloc`/tier-1 compilation run between guest
    sends and reset it. Ask Win32 the real question instead
    (`GetClassInfoW`, `IsWindow`, a return value). Any WG code that
    *branches* on `GetLastError` is fragile in this exact way.
  - **A direct `Alien` moves.** Every byte Win32 reads lives in the
    `VirtualAlloc`'d arena; `nativeUtf16:` is the copy that answers an
    address. `MSG`, `RECT`, `PAINTSTRUCT` — all arena.
  - **Window classes, atoms, the message queue and last-error are
    PER-PROCESS or per-thread**, and `cargo test` runs several VMs in one
    process. Names stay unique per VM.
  - **A layered world's test file cannot self-guard** — `tests.list` loads
    the layer file first; that ordering rule is inherited, not re-litigated.
  - **Classes in a layer forward-declare** before they name each other.
- P5's resolver; WG0's primitives 268–271 (`primWinkb*`) and its
  `WinProbe`/`WinRef`/arena machinery — WG1 **promotes** these rather than
  copying them (see D1).
- The hosted-worker machinery: `register_hosted_worker` (CG1) — the
  mechanism that lets a VM run *on the current thread* rather than being
  spawned. Tested on Windows since P0.

## Deliverables

- `world/91_winui_shell.mst` in `winui.list`: `WinShell` (the window's
  Smalltalk owner), `WinArena` (WG0's arena, promoted out of `WinProbe`),
  `WinApi` (the FFI façade: constants, struct offsets, the imports WG1
  needs). `WinProbe` keeps only what WG0's tests exercise.
- `world/tests/61_winui_shell_tests.mst` — headless assertions (see D5).
- **`win_gui` crate, binary `macvm-winui`** — the *thin* half: boot a VM,
  register it as a hosted worker on the main thread, load `winui.list`, ask
  Smalltalk to open the window, then run the loop. Plus `control.rs`
  (lifted verbatim) and `snap` (lifted verbatim) — **§3.1's inheritance is
  the point; do not write a second capture**.
- `just gate-wg1`.

## Design

### D1. Who owns what — the line WG1 draws for every later sprint

| concern | owner |
|---|---|
| window class, window creation, styles, title, DPI scaling, Mica/dark-mode calls | **Smalltalk** (`WinShell`) |
| the message loop's *pump* (`GetMessageW`/`TranslateMessage`/`DispatchMessageW`) | **Rust** (`macvm-winui`), because it must run before any Smalltalk exists and must survive a guest fault |
| what each message *means* | **Smalltalk** — but not until WG2. WG1's window uses `DefWindowProcW` unchanged. |
| the arena, UTF-16 conversion, constants, struct offsets | **Smalltalk** (`WinArena`, `WinApi`) |

WG1 deliberately does **not** open the WndProc door. The window is real,
visible, DWM-styled and closable, and every message is `DefWindowProcW`'s
to answer. That keeps WG1's risk in "does a Smalltalk-created window
behave" and leaves dispatch — the genuinely hard part — wholly to WG2.

### D2. The thread arrangement (§2.2 commitment 1, made concrete)

```
main thread ── macvm-winui::main
               ├─ boot VM  (the UI VM)
               ├─ register_hosted_worker(&mut vm)      ← runs HERE, not spawned
               ├─ load_list("winui.list")
               ├─ eval "WinShell openMain."            ← creates the window
               └─ loop { GetMessageW; Translate; Dispatch; drain control }
```

The UI VM is a *hosted* worker: it lives on the thread that called
`register_hosted_worker`, which is `main`. That is not an optimisation —
Windows requires window creation and the pump to share a thread, and
`CreateWindowExW` binds the HWND to whichever thread made it. WG0 got away
with a non-main thread only because its window was hidden, loop-less and
immediately destroyed; that exemption ends here.

**No primary VM in WG1.** Commitment 2 (primary messages the UI VM) needs
something worth messaging *about*, which is WG4+. One VM, one thread, and
`MACVM_WINUI_CTL` drives it — the smallest arrangement that proves the
thread story.

### D3. Windows-11 materials — the three DWM calls

All via the FFI, all from Smalltalk, all `dwmapi.dll`:

| what | call | note |
|---|---|---|
| Mica backdrop | `DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE=38, &DWMSBT_MAINWINDOW=2, 4)` | Build-gated: on a Windows build that predates it the call returns a failure HRESULT and the window is simply opaque. **Check the HRESULT, do not check the build number** — same discipline as asking `GetClassInfoW` rather than reading an error code. |
| dark titlebar | `DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE=20, &BOOL, 4)` | Value read from the system (see below), not hardcoded. |
| rounded corners | automatic on Win11 | Nothing to call; recorded so nobody hunts for it. |

**The dark-mode value comes from the system**, not from a preference this
project invents: `AppsUseLightTheme` under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize` (0 =
dark). Reachable through the FFI with `RegGetValueW`. If the read fails,
light is the honest default.

**DPI**: `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` — the web
GUI's shell already does exactly this and the reasoning transfers
verbatim. Every later custom-drawn element takes its scale from
`GetDpiForWindow`; WG1 proves the plumbing by sizing the window in DIPs
and asserting the client rect scales.

### D4. The loop, and why Rust holds it

```rust
while GetMessageW(&mut msg, None, 0, 0).into() {
    TranslateMessage(&msg);
    DispatchMessageW(&msg);
    drain_control_requests();       // the §3.1 channel
}
```

Rust owns the pump for three reasons, each worth stating because WG2 will
be tempted to move it: it must run before `winui.list` has loaded; a guest
fault inside a Smalltalk-owned loop would abandon the pump and freeze the
window (P2 recovers the VM, not the loop); and `GetMessageW` blocks, which
a Smalltalk `[true] whileTrue:` would do with no way for the control
channel to interleave. Smalltalk decides what messages *mean* (WG2); Rust
guarantees they keep arriving.

### D5. Gate — headless, because §3.1 exists

The window is visible, but nothing about proving it needs a human:

1. `macvm-winui` starts, the window appears, `IsWindow` true, the title is
   what Smalltalk set.
2. `gui connect <port>` → `ping` → `pong`; `snap` writes a PNG whose
   dimensions equal the client rect (the shot is *the* evidence, and its
   size is machine-checkable).
3. `doit` evaluates in the UI VM — `WinShell setTitle: 'WG1'` changes the
   real titlebar, verified by `GetWindowTextW` read back through the FFI.
4. `WM_CLOSE` (posted via the FFI from a doit) closes the window and the
   process exits 0 — the loop terminates rather than being killed.
5. The Rust and world suites are unchanged from WG0's close (1086/0/10 and
   7650/0), both database states.

## Pitfalls

- **`GetMessageW` returns `-1` on error**, not just 0/nonzero, and the
  naive `while` above treats −1 as true. Handle it explicitly or the
  process spins on a bad `MSG` pointer.
- **The window must be created on the loop's thread.** If a refactor ever
  moves `openMain` off main, `DispatchMessageW` silently stops delivering
  to it. Assert the creating thread id equals the pumping thread id once,
  at startup, and say why.
- **Closing is two events, not one.** `WM_CLOSE` destroys the window;
  `WM_DESTROY` should `PostQuitMessage(0)` so `GetMessageW` returns 0 and
  the loop ends. With `DefWindowProcW` in charge (D1) the default already
  does the first; the second needs the door WG2 builds — **so WG1 posts
  `WM_QUIT` from the control channel's own exit path** and records that
  the proper `WM_DESTROY` → `PostQuitMessage` wiring lands with WG2.
- **`DwmSetWindowAttribute` before the window is shown**, or Mica applies
  to an already-painted frame and flickers.
- Per WG0's Δ 11: name the window class uniquely per VM. Two `macvm-winui`
  processes on one desktop are a normal debugging situation.

## Interfaces for later sprints

- `WinShell class>>openMain`, `>>hwnd`, `>>setTitle:`, `>>close` — WG2
  wraps the same object with dispatch.
- `WinArena`, `WinApi` — every later sprint's struct and string plumbing.
- The control channel with a live window: WG3+ gates are
  `view`/`doit`/`sleep`/`snap` scripts against it.

## Out of scope

Any WndProc other than `DefWindowProcW`; any control; any painting beyond
what DWM does; menus; the primary VM; COM (WG3+).

---

> **Δ (2026-08-10, WG1 — BUILT; what measurement corrected).** The window is
> real: 900×600 client at 96 DPI, title `MACVM — Windows` then `WG1-OK`,
> created on tid *N* and pumped on tid *N*, both `DwmSetWindowAttribute` calls
> `S_OK`, closed by a scripted `WM_CLOSE`, process exit **0**. `snap` produced
> a 900×600 PNG whose first pixel is `243,243,243` — the light-theme fill this
> machine's `AppsUseLightTheme` asked for. Twelve corrections, in the order
> WG2 will meet them.
>
> 1. **D2's `register_hosted_worker(&mut vm)` is incoherent, and WG1 needs
>    nothing in its place.** That call mints a worker entry **in a PRIMARY
>    VM's registry** (CG1) and answers `None` when the receiver is not a
>    primary — while the same section correctly says WG1 has **no primary
>    VM**. There is nothing for a hosted worker to be hosted *by*. What D2
>    actually wants — "this VM lives on the thread that called this, not on a
>    spawned one" — is precisely what `VmHandle::boot` on `main` already is,
>    so `macvm-winui` boots in place and stops there. `register_hosted_worker`
>    comes back at WG4+, when commitment 2 has something worth messaging
>    about. **WG2 should not reach for it either.**
> 2. **A remembered HWND is not a window, and that cost the first clean
>    shutdown.** `WinShell hwndValue` originally answered whatever the class
>    variable held. `WM_CLOSE` → `DefWindowProcW` → `DestroyWindow` really did
>    destroy the window, but the host asked, got a plausible handle back,
>    concluded its window was alive and pumped an empty queue forever — a hang
>    with no error anywhere. `hwndValue` now answers 0 unless `IsWindow` says
>    otherwise, which is the same rule `WinRef>>isAlive` already stated and
>    which WG2's dispatch will need on every handle it caches.
> 3. **The pump must not ask the VM anything per message.** The first pump
>    read `WinShell hwndValue` through `vm.eval` on every dispatched message —
>    a parse, a compile and a send inside what will be a `WM_MOUSEMOVE` storm.
>    Rust now reads the HWND once and asks Win32's own `IsWindow` thereafter.
>    The general rule for WG2: the door crosses into Smalltalk because a
>    message MEANS something; anything the pump merely needs to *know* about
>    its own window, Win32 answers more cheaply than the VM can.
> 4. **`gui/src/shell/win.rs::run` still has the −1 bug this sprint's own
>    pitfall names.** `while GetMessageW(&mut msg, None, 0, 0).as_bool()` —
>    and `BOOL(-1).as_bool()` is `-1 != 0`, i.e. TRUE. `macvm-gui` has shipped
>    that loop since P4. WG1's pump classifies −1/0/other explicitly and has a
>    unit test that asserts the naive predicate's wrongness so nobody
>    "simplifies" it back; forcing the −1 for real (`GetMessageW` filtered to
>    a dead HWND) confirms `rc = -1`. **The WebView2 host still needs the same
>    fix** and it is not WG1's to make.
> 5. **`AdjustWindowRectEx` (D3's DPI plan) is the wrong tool twice** — it is
>    DPI-ignorant (its DPI-aware twin wants the DPI you are trying to
>    discover), and it describes the frame the non-client metrics imply rather
>    than the one DWM actually draws, which on Windows 11 differ. `applyDips`
>    MEASURES instead: create, read the window rect and the client rect, grow
>    the window by exactly the difference. One API fewer, no constant, and it
>    makes gate item 7 an equality (`client == DIP × dpi/96`) rather than an
>    approximation.
> 6. **tests_wg1's `size = last field's offset + its width` does not hold for
>    `MSG`, and should not.** `MSG.pt` is a `POINT` (8 bytes) at offset 36 and
>    `MSG` is 48 — 44 rounded up to the struct's alignment. TAIL PADDING is
>    exactly what `sizeOf:` exists to report and what deriving a size from
>    offsets silently drops. The honest invariant is `lastOffset + width <=
>    size` with the gap below the alignment. `WNDCLASSW` satisfied the
>    stricter form only because its last member is a pointer already at a
>    multiple of 8 — a coincidence of that struct, not a rule, and WG0's Δ 3
>    should be read that way from now on.
> 7. **§3.1's "lift `control.rs` wholesale (it is already shell-agnostic)" is
>    false; `snap`'s claim is true.** `control.rs` reads `MACVM_GUI_CTL` and
>    calls `crate::shell::waker()` — two hard couplings to the WebView2 host.
>    Both became parameters (env var, log prefix, wake closure), after which
>    BOTH files are **`#[path]`-included by the new crate rather than copied**:
>    one listener, one PNG writer, two hosts, and a test in `win_gui` that
>    fails if either is ever re-implemented locally. `snap` needed no change at
>    all — `PrintWindow` + `PW_RENDERFULLCONTENT` captured a Smalltalk window
>    in a process with no WebView2 in it, first try, correct size and correct
>    pixels. §3.1's reasoning for that choice was right.
> 8. **`PostThreadMessageW` needs a queue that does not exist yet.** The
>    control channel is armed BEFORE Smalltalk makes a window — deliberately,
>    because that is what keeps the app drivable when `openMain` fails — and a
>    thread that has never called a message function has no message queue, so
>    the first wake would be dropped. One `PeekMessageW` at startup creates it.
>    A window message (what `macvm-gui` posts) would have had the same problem
>    for a different reason; a thread message has no window to be null.
> 9. **`gui quit` did not exist.** The "closing is two events" pitfall says
>    WG1 "posts `WM_QUIT` from the control channel's own exit path", but
>    `macvm rusttcl`'s `gui` verb had no subcommand that could say it. One
>    line in `src/rusttcl/verbs.rs`; the other two hosts answer `ERR unknown
>    control verb`, which is the honest reply rather than a silent no-op. The
>    normal close path is still `WM_CLOSE` from a doit, with the host's pump
>    noticing its window has gone — that branch is WG1's stand-in for the
>    `WM_DESTROY` → `PostQuitMessage` wiring **WG2 should delete when the door
>    lands**.
> 10. **The control channel's `eval` answers INLINE here, and that is what
>     makes the gate a script.** `macvm-gui` can only reply "OK submitted"
>     because its VM is on a worker thread; `macvm-winui`'s VM is on the
>     pump's own thread, so `eval` returns the real `printString`. Every
>     number in gate items 1–7 — the client rect, the DPI, the HRESULTs, the
>     title read back through `GetWindowTextW` — comes back over the wire in
>     the same session that captures the PNG. WG3+ gates inherit this and
>     should prefer `eval` over `sleep`-and-hope.
> 11. **Mica "took" is not Mica "visible", and the HRESULT is not the reason.**
>     Both attributes returned `S_OK` on this build; rounded corners and the
>     system-themed titlebar are there. The backdrop is not, because the
>     client area is filled opaquely by the class's background brush and DWM
>     draws Mica *behind* the window. Making it show through is a painting
>     decision (WG3/WG4), not another DWM call — recorded so nobody re-checks
>     the HRESULT looking for a bug that is not there. The brush is not waste:
>     it is read from `AppsUseLightTheme`, so the capture's pixels
>     (`243,243,243` here) are a machine-checkable statement about the theme
>     read, and the gate checks them.
> 12. **Gate item 8 cannot hold literally, because item 27 of the same
>     document mandates a new test file.** Measured: world **7650 → 7723**
>     with the database present and **7526 → 7535** without it, **0 failed**
>     in both; Rust unchanged. The invariant that means something is "nothing
>     lost, nothing failing", not a frozen integer — and WG0's own numbers
>     should be read the same way.
> 13. **`winui.list` became two files and that broke `gate-wg0`.** Its ladder
>     step ran `cat world/90_winui_probe.mst` on its own, which after the
>     promotion still COMPILES (90 forward-declares `WinArena`/`WinApi` with
>     empty bodies) and then dies at run time with `does not understand
>     winkbAvailable` — a failure that only appears when the file is loaded
>     ALONE, i.e. never in the test suite and always in that one gate. The
>     recipe now reads `winui.list` and concatenates in the list's own order,
>     so **WG2's third file cannot break it a second time**; anything else
>     that names a layer file directly should do the same.
>
> Two non-corrections worth recording. **Every number WG1 needed was in the
> database**, including the ones that look like macros rather than constants:
> `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` (−4),
> `HKEY_CURRENT_USER` (−2147483647, which marshals sign-extended to the real
> 64-bit `0xFFFFFFFF80000001`), `IDC_ARROW`, `DWMSBT_MAINWINDOW`,
> `USER_DEFAULT_SCREEN_DPI` — so D2's "never transcribe a header number" held
> for all of WG1, and `dwmapi.dll` resolved through exactly the path `user32`
> does. And **P2's recovery does what D4 assumed**: a forced guest `error:`
> and a forced ACCESS_VIOLATION, both injected at the end of `openMain` with
> the window fully shown, were recovered by the VM while the pump — which
> never learned either happened — kept the window live and answered the
> control channel, exiting 0 on request. That is D4's second reason, measured
> rather than argued.
