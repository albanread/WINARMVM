# winui-wg6d.tcl — WG6d-2's windowed gate: the Editor pane on DirectWrite.
#
#   MACVM_WINUI_CTL=7677 ./target/debug/macvm-winui.exe &
#   ./target/debug/macvm rusttcl scripts/winui-wg6d.tcl
#
# Every line of the form `WG6D <key> <value...>` is read by `just gate-wg6d`.
#
# WHAT THIS PROVES THAT WG6c's GATE COULD NOT. That gate could see state —
# `paintCalls` climbing, `paintError` empty, a caret offset — and every one of
# those was green while the pane was visibly smeared, because none of them
# could see INK. The pixel assertions now live in `winui_render`'s own Rust
# tests, where they run with no window at all; what is left for a WINDOW to
# prove is the part a bitmap cannot: that a real device was created on this
# machine, that frames are actually reaching the screen, and that the guest's
# cell arithmetic agrees with the renderer's about the grid.

gui connect 7677
puts "WG6D open [gui eval {WinShell isOpen}]"

# ── the Editor view, through a real WM_COMMAND on its bar cell ──────────
gui send "0 0x111 [gui eval {(WinShell controlNamed: (WinShell viewButtonNameFor: #editor)) id}] 0"
gui drain now
puts "WG6D view-active [gui eval {WinShell activeView}]"

gui doit {WinShell openEditorOn: ('Object subclass: Demo [', (String with: Character nl), '    "the comment"', (String with: Character nl), '    run [ ^self foo: 3 + 4 ]', (String with: Character nl), ']').}
gui drain now

# ── the renderer ────────────────────────────────────────────────────────
# `available` separates "the DLL is not beside the exe" from a genuine
# renderer failure — they fail identically at the FFI and are entirely
# different problems.
puts "WG6D dll-available [gui eval {WinRender available}]"
puts "WG6D attached [gui eval {WinShell renderAttached}]"
puts "WG6D cell-w [gui eval {WinRender cellWidthFor: WinShell editorPaneHwnd}]"
puts "WG6D cell-h [gui eval {WinRender cellHeightFor: WinShell editorPaneHwnd}]"
puts "WG6D last-error [gui eval {WinRender lastError}]"

# THE GRID IS THE VIEWPORT, and the guest's cols/rows must be the renderer's.
# A disagreement here is text clipped where nothing looks clipped, or writes
# landing on the wrong row — the seam's own version of the defect this whole
# redesign exists to make unrepresentable.
puts "WG6D guest-grid [gui eval {WinShell editorGridSize}]"
puts "WG6D render-grid [gui eval {WinRender gridSizeFor: WinShell editorPaneHwnd}]"

# ── frames actually reach the screen ────────────────────────────────────
gui doit {WinShell refreshEditorPane.}
gui drain now
gui drain now
puts "WG6D frames [gui eval {WinRender framesFor: WinShell editorPaneHwnd}]"
puts "WG6D paint-error [gui eval {WinShell paintError}]"

# A selection, so the picture carries the background-as-cells claim: the
# renderer has no notion of a selection at all, it just draws the colours it
# was given.
gui doit {WinShell editorCaretAt: 1.}
gui doit {WinShell editorSelectAll.}
gui doit {WinShell refreshEditorPane.}
gui drain now
puts "WG6D selected [gui eval {WinShell editorSelectedText size}]"
puts "WG6D frames-after [gui eval {WinRender framesFor: WinShell editorPaneHwnd}]"
puts "WG6D last-error-after [gui eval {WinRender lastError}]"

puts "WG6D snap [gui snap C:/projects/WINARM/target/winui-wg6d.png]"
gui quit
