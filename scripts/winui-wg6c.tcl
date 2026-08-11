# winui-wg6c.tcl — WG6c-2's windowed gate: the editor pane really ACCEPTS
# INPUT, proven with real messages through the real door rather than by
# calling the handlers from a test.
#
#   MACVM_WINUI_CTL=7673 ./target/debug/macvm-winui.exe &
#   ./target/debug/macvm rusttcl scripts/winui-wg6c.tcl
#
# Every line of the form `WG6C <key> <value...>` is read by `just gate-wg6c`.
#
# WHY THIS FILE HAS TO EXIST, and it is not "for completeness". WG6c-2's whole
# shell-side change is four lines in 91's door: route by HWND to the pane
# before the shell's own handlers see anything. NOTHING HEADLESS CAN TOUCH
# THAT. `world/tests/67_winui_editor_tests.mst` calls `editorApply:` and the
# navigation directly — it proves the arithmetic and the rope, which is most of
# the risk, but it never once goes through `window:message:wParam:lParam:`. A
# pane whose routing was wrong would pass all six of those tests and be
# completely dead on screen.
#
# THE MECHANISM NOTE EVERY WINUI SCRIPT LEANS ON: a message that reaches
# Smalltalk must originate OUTSIDE every VM entry. `gui send` is the host's
# control drain — "the one place in this process that is neither a VM entry
# nor a wndproc" — so a WM_CHAR from there arrives exactly as one from a
# keyboard does. A `gui doit` that sent itself a message would be declined by
# the busy guard, correctly, and the gate would be measuring the guard.

gui connect 7673
puts "WG6C ping [gui ping]"
puts "WG6C open [gui eval {WinShell isOpen}]"

# ── the pane, on screen ─────────────────────────────────────────────────
# Built and placed by hand because NOTHING CALLS `buildEditorPane` YET — the
# Editor view, its class picker and its place in the view bar are WG6c-3. This
# slice's claim is about input reaching a pane, not about the pane having a
# home, so the gate builds one and says so rather than quietly implying the
# view exists.
gui doit {WinShell buildEditorPane.}
gui doit {(WinShell controlNamed: WinShell editorPaneName) show: true.}

# AND THE WORKSPACE'S OWN CONTENT CHILD GOES AWAY, which is not tidying up —
# it is the difference between a gate that shows the pane and one that shows a
# white rectangle and calls it a pass. The Workspace's RichEdit sits at the
# same content rect and ABOVE this pane in z-order; the pane is
# WS_CLIPSIBLINGS, so it paints into a region that is entirely clipped away.
# `paintCalls` climbs, `paintError` stays empty, and NOTHING APPEARS —
# measured here before it could be believed. Hiding the other view's content
# child is exactly what `switchToView:` does for every real view, and is what
# WG6c-3 will do for this one.
gui doit {(WinShell controlNamed: (WinShell contentNameFor: #workspace)) show: false.}
gui doit {(WinShell controlNamed: #workspaceGhost) show: false.}
gui doit {(WinShell controlNamed: WinShell editorPaneName) moveTo: #(16 96 720 380).}
gui doit {WinShell openEditorOn: ('Object subclass: Demo [', (String with: Character nl), '    "the comment"', (String with: Character nl), '    run [ ^self foo: 3 + 4 ]', (String with: Character nl), ']').}
gui drain now

puts "WG6C pane-exists [gui eval {(WinShell controlNamed: WinShell editorPaneName) notNil}]"
puts "WG6C pane-hwnd-nonzero [gui eval {WinShell editorPaneHwnd ~= 0}]"

# THE FIRST PAINT HAS TO HAPPEN BEFORE ANY PIXEL ARITHMETIC, and this is not
# tidiness — it is a race that already produced a wrong answer here.
# `editorCharWidth` and `editorLineHeight` answer DEFAULTS (8 and 16) until a
# paint has measured the real font with DT_CALCRECT, and they change exactly
# once, at that first paint. Every click below is built by one `gui eval` and
# decoded by a later `gui send`, so a paint landing between the two encodes
# with one set of metrics and decodes with the other.
#
# Measured, not theorised: adding repaints elsewhere in this script moved the
# transition and a click that had been landing on line 1 column 4 started
# landing at the end of the document. Forced and ASSERTED here, so everything
# after this line is arithmetic against metrics that can no longer change.
gui doit {WinShell refreshEditorPane.}
gui drain now
gui drain now
puts "WG6C metrics-measured [gui eval {WinShell paintCalls > 0}]"
puts "WG6C char-width [gui eval {WinShell editorCharWidth}]"
puts "WG6C line-height [gui eval {WinShell editorLineHeight}]"

# THE PANE MUST HOLD THE FOCUS or a keystroke is addressed to a window that
# is not listening — and that is not a quibble: it is the entire reason WG6c-1
# rejected an SS_OWNERDRAW STATIC, which can hold focus and still never see a
# WM_KEYDOWN. Asserted rather than assumed.
gui doit {WinApi setFocus: WinShell editorPaneHwnd.}
gui doit {WinShell checkFocusChanged.}
gui drain now
puts "WG6C pane-has-focus [gui eval {WinApi getFocus = WinShell editorPaneHwnd}]"

# ── typing, through the door ────────────────────────────────────────────
# WM_CHAR (0x0102) with 'X' (88). The caret is put at 1 first so the assertion
# below can name the exact character at the exact place rather than merely
# observing that the length changed — a document that grew by one is also what
# a stray newline looks like.
gui doit {WinShell editorCaret: 1.}
puts "WG6C doc-before [gui eval {WinShell editorText size}]"
puts "WG6C caret-before [gui eval {WinShell editorCaret}]"
puts "WG6C edits-before [gui eval {WinShell editorKeys}]"

gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0102 88 0"
gui drain now

puts "WG6C doc-after-typing [gui eval {WinShell editorText size}]"
puts "WG6C edits-after [gui eval {WinShell editorKeys}]"
puts "WG6C typed-char-landed [gui eval {(WinShell editorText at: 1) asString = 'X'}]"
puts "WG6C caret-after-typing [gui eval {WinShell editorCaret}]"

# ── navigation, through the door ────────────────────────────────────────
# VK_LEFT (0x25) as a real WM_KEYDOWN (0x0100). Arrows need no modifier, so
# this is a COMPLETE proof of the WM_KEYDOWN route — unlike Ctrl-Z below.
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0100 0x25 0"
gui drain now
puts "WG6C caret-after-left [gui eval {WinShell editorCaret}]"

# ── the modifier gate really gates ──────────────────────────────────────
# VK_Z (0x5A) with NO Ctrl held. `editorKeyDown:` asks `controlIsDown`, which
# asks Win32's GetKeyState — the REAL keyboard — so this must do nothing at
# all. It is also the honest limit of this gate, stated rather than papered
# over: a SYNTHESISED WM_KEYDOWN cannot make GetKeyState answer "Ctrl is
# down", so the Ctrl-Z PATH is unreachable from a script. What is reachable is
# both halves either side of it — that an unmodified Z is declined (here), and
# that `editorCommand: 'undo'` restores the document exactly (below). The
# keystroke-to-command wiring between them is four lines of `vk = 16r5A`.
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0100 0x5A 0"
gui drain now
puts "WG6C edits-after-bare-z [gui eval {WinShell editorKeys}]"

# ── backspace, through the door ─────────────────────────────────────────
# VK_BACK (0x08). Also modifier-free, and it proves the OTHER direction of the
# edit path: WM_CHAR would deliver 8 here too, and `editorChar:` declines the
# whole control range precisely so this is not applied twice. A document that
# shrank by two would be that bug.
gui doit {WinShell editorCaret: 2.}
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0100 0x08 0"
gui drain now
puts "WG6C doc-after-backspace [gui eval {WinShell editorText size}]"

# ── a click places the caret, through the door ──────────────────────────
# WM_LBUTTONDOWN (0x0201) with lParam packing y in the high word and x in the
# low. Line 2, column 4 at the pane's own metrics — computed in Smalltalk
# because this tcl's `expr` has no `<<`, which is the same finding
# winui-wg5b.tcl records against its own packed wParam.
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0201 0 [gui eval {(WinShell editorLineHeight * 1 + 3) * 65536 + (WinShell editorCharWidth * 4 + 1)}]"
gui drain now
puts "WG6C caret-after-click [gui eval {WinShell editorCaret}]"
puts "WG6C click-line [gui eval {(WinShell editorLineColOf: WinShell editorCaret) at: 1}]"
puts "WG6C click-col [gui eval {(WinShell editorLineColOf: WinShell editorCaret) at: 2}]"

# ── undo, which is the slice's stated gate ──────────────────────────────
# "typing changes the document and undo restores it — which costs nothing,
# because the rope is persistent." Restoration is asserted as EXACT EQUALITY
# against the string the document started as, not as a length: a journal that
# replayed badly would very often come back the right length.
puts "WG6C doc-now [gui eval {WinShell editorText size}]"
gui doit {WinShell editorCommand: 'undo'.}
gui doit {WinShell editorCommand: 'undo'.}
gui drain now
puts "WG6C doc-after-undo [gui eval {WinShell editorText size}]"
puts "WG6C undo-restored-exactly [gui eval {WinShell editorText = ('Object subclass: Demo [', (String with: Character nl), '    "the comment"', (String with: Character nl), '    run [ ^self foo: 3 + 4 ]', (String with: Character nl), ']')}]"

# ── WG6c-2b: a DRAG selects, through three real messages ────────────────
# LBUTTONDOWN, MOUSEMOVE, LBUTTONUP — the full gesture, and none of the three
# needs a modifier, so unlike shift-extend this is a COMPLETE proof rather
# than a partial one. Line 0, columns 0 to 5: `Object`.
gui doit {WinShell editorCaretAt: 1.}
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0201 0 [gui eval {3 * 65536 + 1}]"
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0200 1 [gui eval {3 * 65536 + (WinShell editorCharWidth * 6 + 1)}]"
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0202 0 [gui eval {3 * 65536 + (WinShell editorCharWidth * 6 + 1)}]"
gui drain now
puts "WG6C drag-selected [gui eval {WinShell editorSelectedText}]"
puts "WG6C drag-rects [gui eval {(WinShell editorSelectionRectsAtX: 0 y: 0) size}]"
# AND THE MOUSE IS NOT STILL CAPTURED. A drag that never released would leave
# the pointer captured for the life of the window — WG4 D5's recorded hazard,
# and invisible until every other control in the app stops responding.
puts "WG6C capture-released [gui eval {WinApi getCapture = 0}]"

# ── the clipboard, which is real, global, and shared ────────────────────
# A round trip through Win32's own clipboard rather than through a variable
# pretending to be one: put text on it, ask for it back, compare.
puts "WG6C clip-put [gui eval {WinShell clipboardText: 'ROUNDTRIP'}]"
puts "WG6C clip-got [gui eval {WinShell clipboardText}]"
puts "WG6C clip-error [gui eval {WinShell clipError}]"

# COPY, then PASTE over a different selection. The document is the assertion:
# the six characters the drag selected end up where the second selection was.
gui doit {WinShell editorCopy.}
puts "WG6C clip-after-copy [gui eval {WinShell clipboardText}]"
gui doit {WinShell editorCaretAt: 1.}
gui doit {WinShell editorSelectAll.}
gui doit {WinShell editorPaste.}
gui drain now
puts "WG6C doc-after-paste [gui eval {WinShell editorText}]"
# ONE undo, not two — a replace is a single commit, so the first Ctrl-Z must
# land on the document the user recognises rather than half way.
gui doit {WinShell editorCommand: 'undo'.}
gui drain now
puts "WG6C undo-after-paste-restores [gui eval {WinShell editorText = ('Object subclass: Demo [', (String with: Character nl), '    "the comment"', (String with: Character nl), '    run [ ^self foo: 3 + 4 ]', (String with: Character nl), ']')}]"

# Leave a selection ON SCREEN for the snapshot, because the highlight is the
# half of this slice a human has to be able to see.
gui doit {WinShell editorCaretAt: 1.}
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0201 0 [gui eval {(WinShell editorLineHeight * 2 + 3) * 65536 + 1}]"
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0200 1 [gui eval {(WinShell editorLineHeight * 2 + 3) * 65536 + (WinShell editorCharWidth * 28 + 1)}]"
gui send "[gui eval {(WinShell controlNamed: WinShell editorPaneName) id}] 0x0202 0 [gui eval {(WinShell editorLineHeight * 2 + 3) * 65536 + (WinShell editorCharWidth * 28 + 1)}]"
gui drain now
puts "WG6C snap-selection [gui eval {WinShell editorSelectedText}]"

# ── and none of it broke the paint ──────────────────────────────────────
# A raise inside a paint is captured into `paintError` rather than propagated
# (WG6c-1, following WG4 D3), which is right — and is exactly why it has to be
# ASKED FOR. An editor that threw on every repaint would look like an editor
# that had simply stopped drawing.
gui doit {WinShell refreshEditorPane.}
gui drain now
puts "WG6C paint-calls [gui eval {WinShell paintCalls}]"
puts "WG6C paint-error [gui eval {WinShell paintError}]"

# ── a picture, for the human half of the gate ───────────────────────────
puts "WG6C snap [gui snap C:/projects/WINARM/target/winui-wg6c.png]"

# Close cleanly, so the gate can assert the exit code rather than leaving a
# window behind for the next run's build to trip over.
gui quit
