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

### 2.2 The architecture, stated

*The author's own formulation, 2026-08-10, and the sentence every later
decision in this document answers to:*

> **UI VM on the UI thread, messaging to a Smalltalk independent GUI
> layer. All in Smalltalk, with COM and API.**

Unpacked into its four commitments:

1. **UI VM on the UI thread.** A second VM owns the main thread — the
   thread Windows demands for window creation, the message loop, and every
   `SendMessage`. It is a terminal, not an application: it holds handles
   and pumps messages.
2. **Messaging to the GUI layer.** The primary VM (the user's world, the
   long-lived one) never touches a handle. It *messages* the UI VM —
   copy-passed, asynchronous, the `#uiReq`/`#uiReply` protocol the workers
   already carry. The two heaps stay strictly separate.
3. **An INDEPENDENT GUI layer.** The GUI is its own body of Smalltalk in
   its own world layer (`winui.list`), not a set of hooks bolted onto
   application classes. It can be loaded, browsed, edited, and reasoned
   about on its own terms — and a user can rewrite the Browser from inside
   the Browser.
4. **All in Smalltalk, with COM and API.** Both native surfaces are
   first-class from Smalltalk: flat Win32 exports (§2.3) *and* COM
   interfaces dispatched through vtables (§2.4). No Rust UI code beyond
   the door trampoline and the loader that already exists.

The consequence worth stating plainly: **Rust owns no widget, no layout,
no view logic.** It owns the code cache, the GC, the JIT, one wndproc
trampoline, and the FFI marshalling. Everything a user would call "the
environment" is Smalltalk they can open and change while it runs.

### 2.3 Architecture — Smalltalk drives Win32 through the FFI

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

> **Addendum (WG0, measured).** That `g`-class pointer needs a *source*,
> and WG0 proved there isn't one yet. A guest can learn a DLL export's
> address the ordinary way (`GetModuleHandleW` + `GetProcAddress`, which is
> exactly how WG0 fills `lpfnWndProc` with `DefWindowProcW`), because the
> FFI resolves by NAME and never hands the guest an address of its own.
> **No path publishes a *Rust* function's address to Smalltalk at all**, so
> WG2 owes the door its own channel — a primitive, or a
> `WinShell class>>doorAddress`-shaped verb. The same measurement produced
> the winkb DATA channel WG0 needed (constants + struct layouts, primitives
> 268–271): P5 wired winkb into the resolver only, and everything above the
> handle layer that builds a struct or names a flag goes through those.
> See `docs/sprints/sprint_wg0_detail.md`'s Δ.

> **Addendum (WG2, built).** That channel exists: **primitive 272**,
> `WinApi class>>primWndProcAddress`, answering
> `runtime::win_wndproc::macvm_wndproc`'s address as an Integer — policy-free,
> exactly like the four winkb rows, and the guest writes it into `lpfnWndProc`
> itself. The door is in the **core crate**, not in `win_gui`, for the reason
> the sprint's Δ 1 gives (a downstream crate cannot add a `PRIMITIVES` row, and
> a door that existed only under one binary would make the world layer's
> behaviour depend on which `main` linked it) and for the symmetry that
> `runtime::objc_delegate` is core while `cocoa_gui` is thin. The classifier
> row this section worried about — a callback pointer as an argument — needed
> nothing: `RegisterClassW`'s wndproc is a `g`-class integer end to end, as
> predicted, and `winkb`'s `delegate`-kind modelling passed it first try.
>
> One thing this section could not have known and WG2 measured: the door needs
> a **second** re-entrancy guard beyond D3's depth counter, because
> `CreateWindowExW` and `SetWindowPos` SEND messages synchronously and window
> creation runs inside `vm.eval`. See the WG2 Δ, items 2 and 3 — they constrain
> how WG3's layout can be driven and tested.

### 2.4 COM from Smalltalk — the vtable path

The flat API alone cannot reach modern Windows: the file dialogs
(`IFileOpenDialog`), taskbar progress (`ITaskbarList3`), shell items, DWM's
richer surfaces, WIC imaging, and — should the Canvas ever want it —
Direct2D/DirectWrite are all COM. The author's "with COM and API" says
both surfaces are in scope, and **winkb makes COM the cheaper of the two**:
46,250 interface methods, each with its **vtable index**, plus 79,208
typed parameters and every IID. That is exactly and only what vtable
dispatch needs.

**COM is simpler from a VM than Objective-C was**, and worth spelling out
because the Cocoa bridge's difficulty set a false expectation:

| | Objective-C (cocoa_gui) | COM (here) |
|---|---|---|
| dispatch | `objc_msgSend`, selector→IMP at runtime, ABI varies by return shape | read vtable ptr at `[this+0]`, entry at `[vtable + 8*index]`, call it with `this` as the first argument. **One shape, always.** |
| how the shape is known | live runtime introspection | winkb's `interface_methods.vtable_index` — static, already local |
| exceptions | `NSException` unwinding through JIT frames — needed a whole `@try` shim in C | **none.** COM returns `HRESULT`. A failure is a number, not an unwind. The entire objc_shim problem does not exist. |
| identity | `isa` pointers | `IUnknown` at slots 0/1/2 of every interface, universally |

So `ComRef` is a small class:

```smalltalk
ComRef>>invoke: slot args: anArray retClass: aSymbol
    "this-call: the interface pointer is argument 0; the target address is
     read from the vtable at 8*slot. Both are plain integers by the time
     the FFI trampoline sees them, so this needs no new marshalling."
```

with `doesNotUnderstand:` resolving a selector → (IID, vtable index,
param classes) through winkb and caching it — **reusing S11's PIC
machinery exactly as the Cocoa tier-2 bridge does** (`docs/FFI.md`'s Tier
2 pattern; the design is already written, only its data source changes).
An `HRESULT` < 0 raises a Smalltalk error carrying the code and the
interface/method names; success answers the out-parameter.

Three obligations this creates, named now rather than discovered:

- **Apartment threading.** The UI VM's thread calls
  `CoInitializeEx(NULL, COINIT_APARTMENTTHREADED)` at startup — STA,
  because it owns windows — and `CoUninitialize` on the way down. The
  primary VM never calls COM at all (commitment 2 above), which is what
  keeps the apartment rules trivially satisfiable.
- **Lifetime.** `AddRef`/`Release` are manual, and this VM has no
  finalization yet (weak refs are the S22 stretch). The answer is the one
  the Cocoa bridge already ships: a **scope/pool discipline** draining at
  doit boundaries, plus explicit `release` for long-lived references. Not
  a new mechanism — the same one, pointed at `IUnknown::Release` (slot 2).
- **`HRESULT` is the error channel, everywhere.** No call site may ignore
  it; the `invoke:` wrapper checks centrally so no individual binding can
  forget.

**COM enters at WG3 at the earliest** (the common controls are flat API;
`ITaskbarList3` and the file dialogs are polish), so WG0–WG2 need none of
it. But the classifier work is P5's and the shape is fixed now, so nothing
about it is a surprise later.

### 2.5 What is deliberately different from the Mac

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
| WG1 | the window + the loop | M | WG0 | a real top-level window with Mica + dark titlebar, `GetMessage` loop owned by the hosted UI VM on the main thread; clean close; `macvm-winui` binary exists; **the control channel + `snap` INHERITED, not rebuilt** (see below) |
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
split → architecture (§2.2/§2.3), proven at WG1; live metrics → WG4; separate
game windows → out of scope, greyed honestly.

**What could sink it, named now:** (1) door latency — if a
`WM_SIZE`-storm of Smalltalk dispatches stutters, the answer is the Mac's
own: coalesce in the shell, dispatch the settled state; measure at WG2
before believing either way. (2) `SysListView32` custom-draw fights for
the Browser's source pane — fallback is an owner-drawn code pane from the
start (the web GUI's editor model already defines the contract). (3)
comctl6 + per-monitor DPI — declare `PerMonitorV2` in the manifest at WG1,
not later; every custom-drawn element takes a DPI scale from day one.

### 3.1 The capture channel already exists — WG1 inherits it

Built and proven 2026-08-10, **before** WG1 needs it, against the WebView2
GUI (`gui/src/control.rs`, `gui/src/shell/win.rs::snap_client_area`,
`scripts/gui-snap.tcl`):

- **`MACVM_GUI_CTL=<port>`** arms a loopback listener whose framed
  `<len>\n<bytes>` protocol is **byte-identical to the Cocoa channel's**,
  so `macvm rusttcl`'s existing `gui connect/ping/doit/view/snap/sleep`
  verbs drive either app with no client-side branch. Verified end to end:
  `gui connect` → `pong` → `doit` → `snap` → a PNG showing the running
  environment.
- **`snap` is `PrintWindow` + `PW_RENDERFULLCONTENT`, not WebView2's
  `CapturePreview`.** That choice is what makes it reusable: it captures
  *any* HWND's client area, so the native UI's window works unchanged,
  and it is **synchronous** — no async COM, so none of the nested-message-
  loop deadlock risk this module's header warns about. The PNG writer is
  ~40 lines of stored-deflate by hand rather than a new dependency.
- **`snap` never blocks the UI thread** even so: the UI thread captures
  inline and answers; the LISTENER thread is the one that waits. Blocking
  the right thread is the whole trick.

So WG1's obligation is not "write a snap verb" but "point the existing one
at the new window" — the pieces to lift are `control.rs` wholesale (it is
already shell-agnostic) and `snap_client_area`/`capture_png`/`write_png`
verbatim. The only WG1-specific decision is the env var's name, and
`MACVM_WINUI_CTL` keeps the two apps independently drivable in one
session.

> **Addendum (WG1, measured).** The `snap` half of that paragraph is exactly
> right and the `control.rs` half is not. `PrintWindow` +
> `PW_RENDERFULLCONTENT` captured the Smalltalk-authored window on the first
> attempt, in a process with no WebView2 in it — correct size, correct
> pixels, no change to a line of it. `control.rs`, however, was **not**
> shell-agnostic: it read `MACVM_GUI_CTL` by name and called
> `crate::shell::waker()`. Both are now parameters (env var, log prefix, wake
> closure), and with that done **neither file was copied**: `win_gui`
> `#[path]`-includes `gui/src/control.rs` and `gui/src/shell/snap.rs`, so
> there is one listener and one PNG writer serving both hosts, with a test in
> `win_gui` that fails if either is ever re-implemented locally. Two
> WG1-specific facts worth carrying forward: the wake is a **thread**
> message (`PostThreadMessageW`), because this channel is armed before there
> is a window and must survive an `openMain` that never returns — which needs
> one `PeekMessageW` at startup, since a thread with no message queue silently
> drops the first post; and `eval` answers **inline** with the real
> `printString`, because unlike `macvm-gui` this host's VM is on the pump's
> own thread. That last one is what turns "does it look right" into a script
> that reads numbers instead of one that sleeps and hopes.

**This is also how WG's on-screen gates get tested at all.** Every later
sprint's "does it look right" question becomes a script: switch view,
sleep, snap, read the PNG — the same loop that produced this design's
review of the Mac gallery.

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
  Monitor column priority, the visible Transcript grab bar — §2.5's
  fixes are platform-free ideas wearing Windows clothes first.
