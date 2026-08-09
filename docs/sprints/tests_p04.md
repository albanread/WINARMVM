# Sprint P4 — Test Plan

## Acceptance gate

Headless (automated) parts:

1. `cargo build -p gui` (Windows) + gui unit suite green — WINVM finished
   M6 at 100 unit + 1 integration; parity of coverage, not of count, is
   the bar (this repo's gui is newer/bigger).
2. Shell-seam unit tests: main.rs contains zero platform calls (a grep
   gate over `objc_msgSend|windows::|webview2` outside `shell/`);
   `preprocess` emits origin-relative URLs under the Windows flag and
   `file://` under macOS (pure function — table-driven test).
3. Waker: notify-before-window-exists is buffered/no-op'd then flushed by
   the startup drain (unit-testable with a fake HWND slot — the D3 bug
   pinned forever).

Manual on-screen checklist (recorded in the status entry with
observations, WINVM-M6 style):

4. Page serves via `http://winvm.local`; stylesheets + `smtk.js` + toolbar
   icons all load (no broken subresources in DevTools' network panel);
   UTF-8 intact.
5. `Transcript show: 'ROUND-TRIP-nn'` doit round-trips page → VM →
   PostMessageW → DOM in ~1 s.
6. Class browser lists classes; selecting one loads its methods; Go to
   Definition works (post-July feature — must have survived the seam).
7. Workspace: doit/print-it; **a DNU reports to transcript and the app
   serves the next doit** (STILL-ALIVE then AND-AGAIN — the P2
   dependency's visible payoff).
8. Metrics/Monitor tick with live numbers (zeros acceptable pre-P3; live
   nmethod counts required once P3 landed).
9. Process is native ARM64 (Task Manager arch column / the startup
   assert) — not x64-emulated WebView2 hosting.

## Unit tests

| Test | Module | Assertion | Rationale |
|---|---|---|---|
| `preprocess_origin_relative_on_windows` | `gui::preprocess` | Windows flavor rewrites asset URLs origin-relative; macOS flavor `file://` | D2.1 both directions |
| `shim_injected_forwards_postmessage` | `gui::shell::win` (webview harness or JS-string unit) | injected shim maps `webkit.messageHandlers.*.postMessage` calls onto `chrome.webview.postMessage` | D2.2 without forking smtk.js |
| `waker_reads_hwnd_at_notify` | `gui::shell::win` | slot=0 → buffered; set slot → notify delivers; startup drain flushes backlog | D3.1 |
| `eval_js_is_fire_and_forget` | `gui::shell::win` | eval path contains no wait on async COM (API-shape test / review-pinned) | D3.2 deadlock class |
| `main_has_no_platform_calls` | meta/grep in gate script | see gate item 2 | the seam IS the deliverable |
| `backfill_single_transaction` | `image_store` | backfill runs one transaction (statement-count probe) | the NTFS boot-path stall, wherever the fix currently lives |

## Integration/golden tests

- The existing gui integration test (vm_host round-trip) running on
  Windows — WINVM un-gated its two vm_host tests after recovery landed;
  same here (P2 prerequisite).
- `mock_vm`-driven shell test if the mock re-enters the workspace: drive
  `on_script_message` → downcall effects without a real webview.

## In-language tests

None — the GUI is a shell over the same VM; in-language coverage is the
world suite's.

## Stress/negative tests

- Kill/restart the VM worker under the shell (the guest-fatal +
  worker-death path): shell survives, reports, continues (mirrors WINVM's
  live verification).
- Boot-order race: start with the window deliberately delayed (test hook)
  — no lost first drain (D3.1's backlog flush proves out under the race
  that created the bug).
- A doit that faults foreign (P2's controlled AV) from the Workspace:
  transcript gets the report; app lives.

## Non-goals

- Pixel/theming parity with the Mac shell; menu completeness beyond
  ported items.
- Game pane, snapshot capture (stubbed), packaging.
- Performance of the page pipeline (not measured this sprint beyond the
  backfill fix's boot-time sanity).
