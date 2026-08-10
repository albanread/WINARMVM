# The Windows-native environment — design of record

*A Windows-11-flavoured native UI for WINARMVM, **written in Smalltalk** the
way `cocoa_gui`'s is — window procs and all — loaded as a conditional world
layer users can browse, change, and break while it runs. Drafted 2026-08-10
against the macVM UI gallery (`MACVM/docs/gallery/`, captured from the
signed Mac build the same day) and the `cocoa_gui` design lineage
(`cocoa_gui_design.md`, sprints CG0–CG10). The gallery README's "Notes for
a port" section is treated as requirements; this document is the response.*

---

## 1. Review of the reference design — what the Mac UI actually is

Reading the sixteen screenshots as a body of work, the design is five
decisions applied without exception:

1. **One window, one grammar.** Nine views behind a toolbar switcher —
   Workspace, Browser, Outliner, Find, Editor, Canvas, Docs, Debugger,
   Monitor — never MDI, never tabs-of-documents. The classic Smalltalk
   failure mode (a desktop drowning in windows) is designed out. Only
   things that genuinely own a surface escape: game windows, tool editors.
2. **The VM's vitals are chrome.** MEM / JIT / CODE / ALLOC / GC live at
   the toolbar's right edge, always visible, read from the VM not cached.
   The environment *feels* alive because its pulse is literally in the
   window frame. The Monitor view is the same idea at table depth — one
   row per running VM, plus the UI bridge's own drain stats.
3. **The verbs are global.** Do It / Print It sit in the toolbar as
   first-class window verbs (greyed when the view has no evaluable text),
   not buried in menus. The Transcript docks at the bottom of *every*
   view, shared, collapsible, with its state remembered.
4. **The system teaches itself.** Every empty state is documentation: the
   Debugger's idle text tells you how to set a breakpoint *and shows the
   script line*; the Canvas placeholder states the expression contract; the
   Docs welcome page is the tour, and every code example carries a ▶ that
   runs it. The voice is consistent everywhere ("workers appear here when
   code spawns them").
5. **Everything round-trips to source.** Browser Accept compiles into the
   running world *and* persists to the image. The Sprite and Sound editors
   don't save opaque assets — they emit Smalltalk classes ("Copy Code").
   No dead-end artifacts exist anywhere in the system.

**Strengths to preserve at all costs:** the restraint (monochrome chrome;
colour only where it means something — syntax, doc callouts, the games);
buttons over hidden gestures (+ New Class / Remove Method are visible,
discoverable, mouse-first); empty-states-as-pedagogy; the shared
Transcript; the always-live metrics.

**Honest weaknesses, so the Windows design can do better rather than
copy:**

- The nine toolbar glyphs are cryptic until learned — monochrome,
  unlabelled, and Outliner-vs-Browser is a coin flip on first sight. The
  Docs tour compensates, but labels would onboard faster.
- The Monitor table scrolls horizontally at default width (the IC column
  falls off the edge) — wide-table-in-fixed-window with no column
  priority.
- The Transcript's collapse affordance is a near-invisible tick mark.
- The empty Workspace is a wall of white; the "type `3 + 4 * 2`, Cmd-P"
  hint lives in another view (Docs) rather than as a ghosted line where
  the cursor already blinks.
- Do It / Print It enablement must track focus perfectly or reads as
  dead buttons — a contract worth stating, since the Windows twin will
  live or die by the same detail.

None of these are structural. The grammar is right; the Windows job is to
speak it in Windows materials — and fix the four small things above while
at it.

## 2. The Windows design — same grammar, Windows-11 materials

**Name:** the `win_gui` workspace crate, binary `macvm-winui`; the
Smalltalk side is the `winui.list` conditional world layer (the exact
mechanism `cocoaui.list` uses — CG1's `load_list`, already ported and
tested on Windows). Users open the Browser and find the UI's own classes
sitting in the world, editable while the UI runs on them.

### 2.1 Visual language — Windows-flavoured, not Aqua-imitated

The reference design's own restraint principle argues for honest native
materials. On Windows 11 that means:

| element | treatment |
|---|---|
| Window | rounded corners (free on Win11); **Mica backdrop** via `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE)`; dark-mode aware titlebar (`DWMWA_USE_IMMERSIVE_DARK_MODE`) following the system setting |
| Type | **Segoe UI Variable** (text 9pt/12px equivalents per Fluent metrics); `Cascadia Mono` for code panes — both ship with Win11 |
| Toolbar | a custom-drawn command bar (one owner-drawn strip, not comctl's dated TOOLBARCLASS chrome): view glyphs from **Segoe Fluent Icons** (the system icon font — the native answer to SF Symbols) **with 9pt labels beneath** — fixing weakness #1; active view marked by an accent-colour underline (the Windows tab idiom), accent read from the system |
| Verbs | Do It / Print It as labelled pill buttons on the same bar, greyed by focus context — the Mac contract, stated and tested |
| Metrics cluster | right-aligned on the bar, custom-drawn: small-caps grey captions over values, exactly the Mac's arrangement — this is identity, not decoration |
| Transcript | docked bottom, shared, on a real splitter with a **visible** grab bar + chevron (fixing weakness #3), `Cascadia Mono`, Clear button right |
| Panes | standard controls under visual styles v6: `SysTreeView32` (Outliner, Docs sidebar), `SysListView32` report mode (Browser columns, Find results, Monitor), `EDIT`/custom code pane, real splitters between panes |
| Colour | monochrome chrome; syntax colouring in code panes; accent only for the active-view underline and focus rings; doc callouts keep the Mac's green/blue/orange headings |
| Empty states | every view ships its teaching text, Windows-corrected (Ctrl-D / Ctrl-P, "Debug menu", script verbs) — the pedagogy ports verbatim as *content* |
| Workspace ghost | one ghosted line at the caret on first open: `3 + 4 * 2   — Ctrl-P prints the result` (fixing weakness #4; vanishes on first keystroke) |
| Monitor | column-priority layout: the identity + memory + nmethods columns fixed, the long tail in a horizontally-scrolling region (fixing weakness #2) |

The nine views, their order, the two verbs, the metrics, the docked
Transcript: **identical to the Mac**. A screenshot of either should be
recognisable as the same system wearing its host's clothes.

### 2.2 Architecture — Smalltalk drives Win32 through the FFI

The Cocoa shell's architecture ports because it was designed around a
seam, not around AppKit:

| concern | cocoa_gui (reference) | win_gui (this design) |
|---|---|---|
| Who owns the UI | a **UI worker VM pinned to the main thread** (a "dumb terminal"); the persistent **primary VM** on a background thread | identical — the two-VM split is the gallery README's own hard requirement, and the hosted-worker machinery (`register_hosted_worker`, CG1) already passes its tests on Windows |
| Native handles | `ObjcRef` wrapping `id`, DNU dispatch → `objc_msgSend` | **`WinRef`** wrapping `HWND`/`HANDLE`; messages resolve through **winkb** (P5's database: 18,271 functions, 46,250 COM methods *with vtable indices*, constants, struct offsets) → the A64 FFI trampolines that have passed their tests since P0 |
| Calls INTO Smalltalk | C6 reverse dispatch: delegate IMPs as top-level VM entries | **the WndProc door**: one Rust `extern "system"` wndproc registered per class, forwarding `(hwnd, msg, wParam, lParam)` into the UI VM as a top-level entry; Smalltalk `WinShell>>window:message:wParam:lParam:` dispatches. **Structurally simpler than Cocoa's C6** — no runtime class synthesis, no IMP shapes, one uniform four-word signature |
| Crash safety in callbacks | per-thread sigsetjmp + signal recovery (CG0) | **already built**: P2's VEH + non-unwinding AArch64 setjmp/longjmp *were* this design's CG0, landed and tested — a faulting handler recovers, the message loop continues |
| View models | shared with the web GUI (a second renderer, not a fork) | same rule, third renderer: browser path-scheme, find sweeps, doc content, metrics structs — consumed, not re-implemented |
| World layer | `cocoaui.list` | `winui.list`; classes browsable/editable live |
| Persistence tools | image_store-backed browser/find | identical — the crate already builds on Windows (55 tests) |
| Canvas | CALayer/Metal blit | **GDI DIB blit** (`StretchDIBits` of the RGBA buffer) — no D3D dependency for a pixmap view; measured before optimised |
| Games / sound | Metal + AVFoundation windows | **out of scope** (D3D11/XAudio2 remain the recorded stretch); the Demos menu greys with a reason naming this document |

**Dependency truth:** everything above the handle layer rides on **P5**
(the winkb resolver + ARM64 classifier). P5 is therefore the gate to WG1,
and the WndProc door needs one classifier row P5 can refuse in v1 but this
track cannot: **callback function pointers as arguments**
(`RegisterClassW` takes the wndproc; `SetTimer` optionally takes one). The
door hands Win32 a *Rust* trampoline address — no Smalltalk-side callback
marshalling — so the classifier only needs to pass a `g`-class pointer,
which it already models. Recorded here so P5's refusal list doesn't
accidentally block WG.

### 2.3 What is deliberately different from the Mac

- **Labels under toolbar glyphs** (weakness #1). The Mac may adopt it back.
- **No separate Outliner icon mystery**: the label says Outliner.
- **Keyboard**: Ctrl-D / Ctrl-P (Cmd is not a Windows key); menu
  accelerators per Windows convention; F5/F10/F11 in the Debugger
  (Continue/Step Over/Step Into — the muscle memory of every Windows
  debugger user) with the Mac's button row kept.
- **The window survives the primary VM dying** (the CG9 restart-in-place
  behaviour) — unchanged in intent, but the Windows shell owes it from
  day one because the two-VM split makes it nearly free: respawn the
  primary, re-sync, transcript note.

## 3. The ladder — Phase WG

Sized like CG (S/M/L), each gate headless-testable except where marked
on-screen. **WG0 may start the moment P5's resolver resolves one function;
nothing in WG waits for P5's world-file settlements.**

| # | title | size | needs | gate |
|---|---|---|---|---|
| WG0 | FFI probe: user32 by hand | S | P5 resolver | headless: `MessageBeep`, `GetSystemMetrics`, `RegisterClassW`+`CreateWindowExW`+`DestroyWindow` round-trip driven from a Smalltalk doit through winkb-resolved imports; struct-by-layout (`WNDCLASSW`) built via Alien + winkb struct offsets |
| WG1 | the window + the loop | M | WG0 | a real top-level window with Mica + dark titlebar, `GetMessage` loop owned by the hosted UI VM on the main thread; clean close; `macvm-winui` binary exists; snap verb (`MACVM_WINUI_CTL`) captures a PNG — the gallery's own capture channel, ported |
| WG2 | the WndProc door | **L** | WG1, P2 (done) | the risky sprint, CG3's twin: `WM_COMMAND`/`WM_SIZE`/`WM_CLOSE` dispatch into `WinShell` methods as top-level entries; a raising handler answers `DefWindowProc` and the NEXT message still dispatches; a forced AV inside a handler recovers via the P2 layer and the loop continues; latency of the door measured and recorded |
| WG3 | controls + layout | M | WG2 | Smalltalk creates child controls (`BUTTON`, `EDIT`, `SysListView32`, `SysTreeView32`, trackbar), receives their notifications through the door, lays them out on `WM_SIZE` via a Smalltalk layout class; visual styles v6 manifest in place |
| WG4 | the shell: command bar, transcript, metrics | M | WG3 | the one-window grammar stands: custom-drawn view bar (Fluent glyphs + labels + accent underline), view switching with lazy build, docked collapsible Transcript, live metrics cluster reading `VmMetrics` off the primary; Do It/Print It enablement tracked by focus |
| WG5 | Workspace + Browser | L | WG4, image_store | the two flagship views: Workspace with syntax colouring + ghost line + Ctrl-D/Ctrl-P; the four-pane Browser over the shared path-scheme model, Accept persisting to the image byte-identically to the web path (the CG8 gate, re-run) |
| WG6 | Outliner + Find + Editor | M | WG5 | tree over live reflection with counts; Find sweeps landing selections in the Browser; Editor with File In / Add to World |
| WG7 | Debugger + Monitor | M | WG5 | the halt loop fronted natively: stack/source/frame panes, Step Into/Over/Finish/Continue/Abort + F-keys, auto-front on halt; Monitor with column priority + the UI bridge's own drain stats; primary-restart-in-place (CG9 behaviour) |
| WG8 | Docs + Canvas + polish sweep | M | WG4 | Docs sidebar+content with find-in-page and ▶ Run in Workspace; Canvas as GDI DIB blit with the Mandelbrot + Benchmark Chart quick actions; every empty state carries its teaching text; gallery parity screenshots captured via the snap verb into `docs/gallery-win/` |

**Order rationale:** WG2 is the only sprint with genuine unknowns (the
door's latency and its crash story), so it lands third, not last — the
same reasoning that put CG3 early. Views arrive in usefulness order:
Workspace+Browser make the environment *usable*, Debugger+Monitor make it
*trustworthy*, Docs+Canvas make it *whole*. The gallery README's five
"Notes for a port" map: shared Transcript → WG4; lazy views → WG4; two-VM
split → architecture (§2.2), proven at WG1; live metrics → WG4; separate
game windows → out of scope, greyed honestly.

**What could sink it, named now:** (1) door latency — if a
`WM_SIZE`-storm of Smalltalk dispatches stutters, the answer is the Mac's
own: coalesce in the shell, dispatch the settled state; measure at WG2
before believing either way. (2) `SysListView32` custom-draw fights for
the Browser's source pane — fallback is an owner-drawn code pane from the
start (the web GUI's editor model already defines the contract). (3)
comctl6 + per-monitor DPI — declare `PerMonitorV2` in the manifest at WG1,
not later; every custom-drawn element takes a DPI scale from day one.

## 4. Relationship to the other tracks

- **P5 first.** WG0 consumes its resolver; the classifier rows WG needs
  (`g` scalars, pointers, one 16-byte struct) are all in P5's "model"
  set. The `kqueue`/IOCP slice stays independent of WG entirely.
- **The web GUI (`macvm-gui`) continues as-is** — it is the proven
  environment today and the differential oracle for view models (CG7's
  trick: the native data source must answer the same rows the
  `htmlFragment` model does — reuse that gate shape in WG5/WG6).
- **MacGamePane/D3D11/XAudio2** remain the recorded stretch; nothing in
  WG blocks on them.
- The Mac may cherry-pick back: toolbar labels, the Workspace ghost line,
  Monitor column priority, the visible Transcript grab bar — §2.3's
  fixes are platform-free ideas wearing Windows clothes first.
