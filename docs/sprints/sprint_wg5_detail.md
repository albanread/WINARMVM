# Sprint WG5 — Workspace and Browser, the two flagship views

*docs/ROADMAP.md's WG5 row, unpacked. Written before the code, which is the
order WG3 kept and WG4 had to be corrected back into.*

The row:

> the two flagship views: Workspace with syntax colouring + ghost line +
> Ctrl-D/Ctrl-P; the four-pane Browser over the shared path-scheme model,
> Accept persisting to the image byte-identically to the web path (the CG8
> gate, re-run)

Size **L**, and it is two independent halves that share only the shell they
sit in. They are therefore two slices — **WG5a Workspace**, **WG5b Browser** —
and 5a lands first because 5b's Accept needs a place to report failures and
5a builds it.

## Prerequisites, and what changed to make them true

* **WG4 D1** — the primary VM and the `#uiReq`/`#uiReply` seam. This is the
  one that mattered: *Do It runs the user's code, and the user's code lives on
  the primary.* Before D1 this sprint could not have been built honestly at
  all, which is exactly why the author stopped the first attempt at it.
* **WG4 D4** — enablement tracked by the focused control. Already written to
  ask *"can the thing the user is typing in supply source?"*, and already
  answering `true` for an editable multiline `EDIT`. The Workspace lights it
  up with no change to that file, which is the test that D4 was designed
  rather than fitted.
* **WG4 D5** — the transcript dock, where a Do It's result and a failed
  Accept both report.
* `image_store` (55 tests, builds on Windows) for 5b's persistence.

## WG5a — the Workspace

### D1. Selection-or-everything, ported verbatim as ARITHMETIC

The Mac's rule (`CocoaUI class>>evalTargetFor:loc:len:`) is already pure and
already headless-tested: *a non-empty selection wins; an empty (collapsed)
selection falls back to the whole buffer*, answering `#(source insertAt)`.

It ports **unchanged** — same selector, same shape, same tests — because it is
arithmetic over a string and two integers and has nothing platform in it. What
differs is only where the two integers come from: `EM_GETSEL` rather than
`NSTextView>>selectedRange`.

Stating it as a separate design item because the temptation is to reimplement
it against Win32's API shape and end up with a rule that *nearly* matches the
Mac's. Two GUIs disagreeing about what Do It evaluates would be a bug nobody
could see in either one alone.

### D2. Do It and Print It, over the seam

* **Do It** — ship the target source to the primary as `#uiReq`/`#doit`, and
  append `source => result` to the transcript when the reply lands. This is
  `Worker uiDoit:onReplyTimed:`, which already exists and is already what the
  Mac calls.
* **Print It** — the same request, but the result is inserted **inline** at
  the insertion point, so the Workspace becomes a notebook rather than a
  command line.

**The one hazard, and it is the Mac's own recorded scar:** *capture the
insertion point at INVOCATION, not when the reply lands.* The reply is
asynchronous — the user can move the caret, type, or select something else
while the primary is working — and inserting at "the selection" on arrival
puts the answer wherever they happen to be. The Mac calls this its
`pendingPrintInsertAt` discipline and ported it verbatim from the web GUI;
this port inherits it as a rule rather than rediscovering it as a bug.

### D3. Ctrl-D / Ctrl-P

§2.5: Ctrl rather than Cmd, because Cmd is not a Windows key. `WM_KEYDOWN` is
already allowlisted; the shell needs only to notice the modifier
(`GetKeyState(VK_CONTROL)`) and route. No accelerator table, because there is
no menu yet to own one — WG6 adds the menu and this moves into it.

### D4. The ghost line

§2.1's Workspace row, fixing weakness #4: one ghosted line at the caret on
first open — `3 + 4 * 2   — Ctrl-P prints the result` — vanishing on the first
keystroke. It is a teaching affordance, not a placeholder: the Workspace's
empty state currently teaches nothing, and a user who has never met a
Smalltalk workspace has no way to guess what it wants.

### D5. Syntax colouring — DECLARED OUT OF THIS SLICE

The row asks for it and it is not free: a plain `EDIT` cannot colour text, so
it means either `RichEdit` (a different control, different message set,
different quirks) or a custom-drawn code pane (a full text editor: caret,
selection, scrolling, IME). Both are real work with real risk, and neither
belongs in the slice that first makes Do It work.

Recorded as **WG5c**, after the Browser, so that the decision between them is
made with the Browser's own text needs known rather than guessed. The
Workspace ships monospaced and uncoloured first, which is what every one of
these environments did in its first year.

## WG5b — the Browser

Four panes over the shared model, Accept persisting to the image. Detailed
now that 5a has landed, and the research changed the shape of it.

### What the Mac actually does, and why it matters here

Reads are **DUAL**, deliberately (`66_cocoabrowser.mst`'s own note):

* **hierarchy and selector rows** come from the PRIMARY's live reflection —
  `UiBrowserService browseSnapshot`, which already exists, and which projects
  the live hierarchy into a names-only nested tree of Strings, Arrays and
  smis. Names only, because *the pickle REFUSES class objects* and that
  refusal is the enforcement that no class oop crosses a VM boundary.
* **the variables pane and all source text** come from the IMAGE
  (`image_store`), so an added variable shows immediately while live instance
  shape honestly waits for the next boot — there is no `become:`.

Writes drive `image_store::flows` — the web GUI's own implementation, shared
rather than reimplemented — and live-compile on the primary through the
ordinary `Worker uiDoit:` channel.

### The consequence for this port, and the slicing it forces

The live half needs **nothing new**: WG4 D1 built the seam, `browseSnapshot`
is already pickle-safe by construction, and `Worker uiDoit:` already
live-compiles. A four-pane browser over live reflection is therefore reachable
today, entirely in Smalltalk, with no new Rust at all.

The image half needs a **host service** — the Mac's `host_service.rs` is an
ObjC-shaped adapter over `image_store::flows`, and Windows has no equivalent.
That is real work, it is where the CG8 gate lives, and it is the reason this
row is sized **L**.

So:

* **WG5b-1 — the browser, over live reflection.** Four panes (packages,
  classes, protocols, selectors) plus a source pane, fed by
  `browseSnapshot` across the seam. Read-only. Everything a user needs to
  BROWSE, which is most of what a browser is for, and it exercises the seam
  with a payload far larger than a doit.
* **WG5b-2 — Accept, over `image_store::flows`.** The host service, the write
  path, and the CG8 gate: *Accept persists byte-identically to the web path*.
  That gate is the specification and it is not negotiable — two GUIs that
  wrote the image differently would be a corruption bug with a UI in front of
  it.

#### WG5b-2's real obstacle: there is no channel yet

Researched before writing any of it, and it is not what the row implies. The
question is not *how do we call `flows`* — it is **how does Smalltalk reach
Rust code that lives downstream of the VM at all**, on this platform.

Every existing route is closed:

* **A core primitive is impossible.** `image_store` DEPENDS ON `macvm`
  (its own Cargo.toml says so and explains why), so the core cannot depend on
  `image_store` without a cycle. A `PRIMITIVES` row that called `flows` cannot
  exist.
* **A downstream primitive is impossible.** WG2's Δ 1 already established
  that a downstream crate cannot add a `PRIMITIVES` row — it is why the
  WndProc door lives in the core rather than in `win_gui`.
* **The Mac's own answer does not port.** `cocoa_gui`'s `host_service.rs` is
  reachable because the ObjC runtime is a general late-bound channel and the
  guest already speaks it (`Cocoa classNamed:`). Windows has no equivalent
  bridge, and §2.4's COM path would mean authoring a COM object and its
  vtable to move four strings.
* **The FFI cannot name a foreign DLL.** `FfiPragma::Function` carries
  `{name, ret, args}` and no library. Resolution is winkb (which knows only
  Windows API functions) then `DEFAULT_PROBE` — a fixed list of five system
  DLLs. A custom host DLL is invisible to both.
* **There is no call-by-address.** WG2's primitive 272 publishes the door's
  address for WINDOWS to call; nothing lets the guest call an arbitrary
  address itself.

**The chosen answer: give the FFI pragma an optional `library:`.** One small,
general core change — parser, AST, and the resolution call that already takes
`Option<&str>` and already does `GetModuleHandleA` then `LoadLibraryA`. It
unlocks not just this host service but any third-party DLL a user wants to
call, which is a thing a Smalltalk on Windows should be able to do anyway.
The host service then becomes an ordinary DLL in the workspace, depending on
`image_store` exactly as `cocoa_gui` does, exporting `extern "C"` functions
the guest calls by name — the adapter downstream where it belongs, with no
new mechanism in the VM beyond naming the library.

Recorded at this length because the obstacle is invisible from the row, and
because the next person to reach for `flows` from the guest will otherwise
re-derive all five dead ends.

#### As built

**The core change is one optional pragma part.** `<primitive: FFI function:
#X library: #'foo.dll' ret: #g args: #(g)>`. It threads through the four
layers that already carried `function:` — AST, parser, the compiled
descriptor (one more slot, nil for every pragma that does not name a
library, which is every pragma written before this existed), and the Windows
resolver. An explicit library goes STRAIGHT to `resolve_export(Some(lib))`
and deliberately does NOT fall back to the probe: a same-named export from
user32 answering a call meant for the host service is the one outcome worse
than not resolving at all. `library:` on a Tier 2 `selector:` pragma is
REFUSED rather than ignored — an ObjC send has no library to name, and
accepting it silently would let someone write a pragma that reads as if it
targets a DLL and does not.

**The host service is an ordinary DLL.** `winui_host` — a `cdylib` over
`image_store::flows`, downstream of the VM exactly as `cocoa_gui`'s
`host_service.rs` is. Five exports (save a method, new class, add a
variable, read a method's source, ping) plus a message slot, because the FFI
marshals only integers/doubles/void: every call answers a STATUS and parks
its text where the guest reads it afterwards by COUNT. Strings cross as
UTF-16 both ways, which is what `WinArena` already speaks. `image_path` is
`cocoa_gui`'s rule verbatim — that is not incidental, it is what makes the
CG8 comparison a comparison.

**The gate is three layers, because each proves what the others cannot.**
`just gate-wg5b`:

1. **The differential** (`cargo test -p winui_host`) — the same save through
   the Windows entry point and through `flows::save_method`, against two
   identically-seeded images, compared on source, selector, side, home file
   and version count. This is the CG8 gate proper. "Byte-identical" cannot
   mean the two SQLite FILES compare equal — page layout and rowids differ
   between any two independently built databases — so what is checked is
   every stored consequence of the write, plus that the stored source really
   is the text that went in (two identically WRONG writes would otherwise
   pass). It also pins the edit case, where saving over an existing method
   must not clobber its recorded home file, and pins that a refusal is
   `flows`' own wording word for word.
2. **The channel** (`WinUiHostWg5bTests`, in the ordinary world suite) — that
   `library:` really resolves a DLL in neither winkb nor the probe list. This
   file is NOT host-free, deliberately: the channel is the novel machinery
   and a test that mocked it would prove nothing about it.
3. **End to end** (`world/bench/wg5b_accept.mst`) — a Smalltalk String through
   `nativeUtf16:`, LoadLibraryA, the A64 trampoline, `image_store`, and back
   out. It drives `acceptSourceText:`, the SAME method the Accept cell
   reaches with only the control read lifted out, rather than a private copy
   written for the gate.

**Three things the work turned up that the design did not predict:**

* **`world/image.sqlite3` does not exist in a checkout.** It is a generated
  artifact (`import_world`: 170 classes, 2508 methods). Both the test suite
  and the gate had to be written to survive its absence — a suite that
  asserted a successful read would really have been asserting that someone
  remembered to run the importer, and a gate that created the image as a side
  effect would be worse.
* **The live compile must be best-effort.** `Worker uiDoit:` has no primary
  in a CLI run, and the first gate run took the whole process down with
  `unknown or dead worker` AFTER the image had been written correctly. The
  image is the record; a completed write must not be unwound by the
  best-effort half that follows it. Guarded, and the transcript says which
  half happened.
* **A class name in a method body is resolved at COMPILE time.** The source
  pane's image read had to move from 98 to 99, because 98 loads first and 99
  is where `WinHost` is defined — while `updateSourcePane` calling
  `self sourceTextFor:` is fine, since a message send is not. The same
  bidirectional-reference problem `winui.list`'s header records for 90/91,
  arriving from a new direction.

**Still open in WG5b:** the browser's remaining write verbs (New Class, Add
Variable) have host exports and Smalltalk wrappers but no UI yet — they want
a dialog, and there is no dialog machinery in the shell. Filed for WG6, which
adds the menu that would own them.

Slicing it this way keeps a read-only browser from waiting on a write path,
and keeps the write path from being rushed to make a demo look complete.

### Pitfalls specific to 5b

* **`browseSnapshot` is a WHOLE-HIERARCHY payload.** It crosses the seam as
  one pickled tree. That is fine — it is names only — but it must be requested
  on a refresh rather than per pane, or four panes become four traversals of
  the entire image.
* **Names only, and that is load-bearing.** Anything the browser wants that
  is not in those six slots (source, comments, categories) comes from the
  image path, not by widening the snapshot until a class oop sneaks in.

## Implementation order

1. **5a D1** — the pure rule + its tests, with no window involved.
2. **5a D2/D3** — the Workspace view, editable, Ctrl-D/Ctrl-P over the seam.
3. **5a D4** — the ghost line.
4. **5b** — the Browser, on its own sprint detail.
5. **5c** — syntax colouring, once the control choice is informed.

## Pitfalls

* **The insertion point is captured at invocation.** Stated twice on purpose.
* **`EM_GETSEL` answers UTF-16 code-unit offsets**, and the guest's `String` is
  not UTF-16. Every offset that crosses the FFI is in the control's units and
  must be converted at that boundary, exactly once — a rule worth writing down
  now, because the failure mode (an emoji in a workspace shifting every
  subsequent offset by one) is invisible until it is baffling.
* **Do It's reply may outlive its view.** A user can switch views, or collapse
  the dock, while the primary works. Continuations must tolerate the world
  having moved, which the `(peer, corr)` keying already guarantees — but the
  *view* must be re-found rather than captured.
