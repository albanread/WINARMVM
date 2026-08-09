# Sprint P4 — GUI: the web environment on Win32 + WebView2

Objective: the Strongtalk live-HTML programming environment — class
browser, workspace, transcript, metrics, Monitor tab, debugger panes — runs
in a native Win32 window on Windows ARM64, WebView2-hosted, with the web
assets **unmodified** (WINVM's M6 proved they port with zero edits; nothing
arch-specific has appeared since). The work is a seam re-extraction: this
repo's `gui/` is NEWER than WINVM's (Monitor tab, labeled debugger panes,
snapshot changes — Aug 2026 commits) and has NO `shell/` seam; WINVM's M6
defines the seam and supplies `win.rs` nearly wholesale. Implements
MIGRATION.md §2 (gui row) with WINVM MIGRATION §6 M6 as the worked example.

## Prerequisites

- P2 (hard): guest-fatal recovery — a GUI whose Workspace dies on the
  first DNU is not shippable; WINVM shipped exactly that bug and its fix
  entry documents the pain. P3 (soft): the metrics pane shows JIT stats;
  it renders zeros before P3 and that is acceptable during development.
- References: `WINVM/gui/src/shell/{mod.rs,mac.rs,win.rs}`,
  `WINVM/gui/Cargo.toml` (the `webview2-com` 0.39 + `windows` 0.62
  **matched pair** — webview2-com-sys pins exactly that windows version),
  WINVM MIGRATION M6's three replaced mechanisms + two bug entries.

## Deliverables

- `gui/src/shell/` in THIS repo: `mod.rs` (the seam trait/surface),
  `mac.rs` (current Cocoa/WKWebView code relocated, unchanged in
  behavior), `win.rs` (from WINVM, reconciled).
- `gui/src/main.rs` with **zero platform calls** — the M6 shape: the shell
  owns the event loop and calls up (`on_script_message`, `on_vm_drain`,
  `on_menu_action`, `on_metrics_tick`); main calls down for effects.
- `gui/Cargo.toml`: `[target.'cfg(windows)'.dependencies]` webview2-com +
  windows pair; gamepane feature stays off-by-default with the
  MacGamePane path-dep note.
- Workspace membership: `gui` rejoins `members` (P0 removed it).
- `just gate-p04` (headless parts) + the manual on-screen checklist.

## Design

### D1. The seam re-extraction (the actual work)

WINVM's M6 extraction was done on July code; this repo's gui moved. So:
diff WINVM's `mac.rs` against ITS pre-seam `main.rs`/`objc.rs` to recover
the **seam boundary decisions** (which calls became `on_*` upcalls, which
downcalls the shell exposes), then re-apply that boundary to CURRENT
`main.rs`/`objc.rs` — including the post-July features (Monitor tab,
debugger panes, snapshot-capture) which must land on the seam's main-side,
not inside `mac.rs`, unless they are genuinely AppKit-bound (snapshot's
screen capture is; it becomes a shell downcall with a Windows
implementation deferred-with-stub). `win.rs` then ports from WINVM with
mostly mechanical reconciliation.

### D2. The three replaced mechanisms (carry WINVM's answers verbatim)

1. **Local assets**: WKWebView's `loadFileURL:allowingReadAccessToURL:`
   has no WebView2 counterpart; `file://` pages cannot load subresources.
   Answer: `SetVirtualHostNameToFolderMapping` publishing the GUI root at
   `http://winvm.local`; `preprocess` emits origin-relative URLs on
   Windows, `file://` on macOS — same asset tree, per-OS grant model.
2. **JS bridge**: `assets/smtk.js` posts via
   `window.webkit.messageHandlers` and is SHARED — never fork it. The
   shell injects a shim (`AddScriptToExecuteOnDocumentCreated`) forwarding
   to `window.chrome.webview.postMessage`.
3. **Worker→UI wakeup**: `performSelectorOnMainThread:` → `PostMessageW` —
   each platform's one documented thread-safe entry to its UI thread.

### D3. The two bugs WINVM already paid for (do not pay twice)

1. **The waker must read the HWND at `notify` time, not capture it at
   construction** — main spawns the VM worker BEFORE the window exists;
   a construction-time capture is permanently 0 and every wakeup silently
   no-ops (the VM computes answers that never reach the page). Plus: post
   one drain once the window exists. Port the fixed shape.
2. **Never block on an async WebView2 operation from inside a COM
   callback** — `wait_for_async_operation` pumps a nested message loop and
   deadlocks inside `WebMessageReceived`. `eval_js` is fire-and-forget
   (macOS passes a nil completion handler for the same reason).

Also check `image_store::backfill_method_sends` for the one-transaction
fix (WINVM found per-edge fsync transactions took minutes on NTFS and sat
on the boot path; the fix "helps macOS too" and may or may not have been
cherry-picked upstream — verify, apply if absent).

### D4. ARM64 specifics (thin, deliberately)

- WebView2 Evergreen runtime on Win11-ARM is **native ARM64**; the crate
  pair compiles for `aarch64-pc-windows-msvc` (COM vtables are
  arch-neutral Rust + the windows crate's aarch64 impls). Startup asserts
  the process arch (P0's assert reused in the gui bin) so an accidentally
  x64 gui binary under emulation announces itself.
- No other ARM64 content exists in this sprint — that is the point of the
  seam: `win.rs` is OS code, not ISA code.

## Implementation order

1. D1 seam extraction with `mac.rs` only; macOS behavior byte-identical
   (this step is verifiable on the Mac side of the same commit — run the
   gui there if convenient, else rely on compile + review since mac
   behavior is not this machine's gate).
2. `win.rs` port + Cargo wiring; blank window + asset serve via
   winvm.local (check all subresources load — WINVM's M6 verification
   list).
3. Shim + transcript round-trip (`Transcript show:` page→shim→Rust→VM
   worker→PostMessageW→drain→DOM).
4. D3 waker shape + startup drain; Monitor/metrics tick.
5. Browser + workspace + debugger panes against the live VM.
6. `just gate-p04` headless seam tests + the manual checklist run.

## Pitfalls

- The seam belongs to THIS repo's newer gui, not to WINVM's July snapshot
  — reconcile toward current behavior; WINVM supplies mechanism, not
  feature-state (MIGRATION §2's source-of-truth rule applied to gui).
- `webview2-com`/`windows` versions move together or not at all.
- The virtual-host name (`winvm.local`) appears in `preprocess` output —
  keep WINVM's exact name so its tests port unchanged; it is a private
  origin string, not a network name.
- Keep `gui/mock_vm` (if reincluded) building headless — the shell seam's
  unit tests depend on driving the shell surface without a real VM.
- MacGamePane path deps: still not declarable on Windows (workspace
  resolution reads every path-dep manifest); the restore instructions
  stay in `gui/Cargo.toml` comments.

## Interfaces for later sprints

- The FFI/winkb work (P5) is invisible to the GUI, but P5's wall-clock
  lands `Date/Time now` for the environment's own display uses.
- A native Win32-controls shell (the `cocoa_gui` analogue) remains a
  possible future track — the seam built here is its prerequisite too.

## Out of scope

- Game pane / D3D11 / XAudio2 (deferred exactly as WINVM defers them).
- Snapshot screen-capture Windows implementation (stubbed downcall;
  needs a DXGI/GDI decision that deserves its own slice).
- Native menus/toolbar parity beyond what `win.rs` already carries.
- Installer/packaging/signing (no Windows analogue of the notarization
  runbook is attempted yet).
