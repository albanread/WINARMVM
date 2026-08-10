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
