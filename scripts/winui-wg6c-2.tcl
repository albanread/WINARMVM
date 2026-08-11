# winui-wg6c-2.tcl — the second half of WG6c's windowed gate, run after the
# recipe has let the window pump FREELY for a few seconds.
#
# Why a second file at all: the Editor's class picker fills from a REPLY. The
# view's build ships `refreshBrowser`, which is a `#uiReq` to the primary, and
# the tree arrives on a later drain pass. `gui sleep` blocks the app rather
# than only this driver, so a wait inside part 1 starves the very drain pass it
# is waiting for — winui-wg5b-2.tcl records the same finding, measured there as
# twelve rounds of sleep-then-drain reporting nothing after six seconds. The
# bash `sleep` between the two scripts is the fix, and this is what runs on the
# other side of it.

gui connect 7673
gui drain now

# ── the picker filled, from the primary's own hierarchy ─────────────────
# The rows are the SAME ones the Browser and the Outliner draw — one snapshot,
# three views — so this also asserts that the fan-out in `browserTreeArrived:`
# actually reaches the Editor rather than stopping at the other two.
#
# A RELATIONSHIP, never a frozen count (WG2 Δ 14): the world grows, and a gate
# pinning today's number would fail on the next file added to world.list. What
# matters is that the picker holds exactly what the snapshot holds — a list
# that filled with a DIFFERENT number of rows would be the `listSet:items:`
# class of bug WG6b hit, where the view showed an empty list while the
# transcript reported five hits.
puts "WG6C2 browser-classes [gui eval {WinShell browserClassCount}]"
puts "WG6C2 picker-rows [gui eval {(WinShell controlNamed: #editorClasses) send: (WinApi constant: 'LB_GETCOUNT') wParam: 0 lParam: 0}]"

# And the view is still the Editor, with its pane still painting — a picker
# that filled by switching away and back would prove less than it looks.
puts "WG6C2 still-editor [gui eval {WinShell activeView}]"
puts "WG6C2 paint-error [gui eval {WinShell paintError}]"

# ── A PARTIAL PAINT is no longer a shape that can break ────────────────
# It used to be THE defect: `refreshEditorPane` invalidates with NULL, so
# rcPaint always arrived at the client origin and the document — drawn at
# `rcPaint.left + column * charW` — always landed correctly, while Windows
# invalidating a SUB-rectangle redrew the whole document displaced.
#
# WG6d made that unrepresentable rather than fixed. The renderer redraws the
# ENTIRE grid on every present and ignores rcPaint completely, so there is no
# partial composition left to get wrong. The invalidation is still provoked —
# it is a real WM_PAINT and must still reach a present without error — but the
# origin assertion is gone because the origin no longer exists.
gui doit {WinShell invalidateEditorRectX: 40 y: 30 w: 120 h: 60.}
gui drain now
gui drain now
puts "WG6C2 partial-paint-error [gui eval {WinShell paintError}]"
puts "WG6C2 frames-after-partial [gui eval {WinRender framesFor: WinShell editorPaneHwnd}]"

# ── a picture, for the human half of the gate ───────────────────────────
# TAKEN WITHOUT REPAIRING THE PANE FIRST, deliberately. A full repaint after
# the partial one would hide exactly what this is here to show: with the bug,
# the partial paint leaves a second copy of the document displaced by the
# invalid rectangle's origin, and a tidy-up refresh would wipe the evidence
# before the shutter. What the picture must show is a pane that a partial
# repaint changed in no way at all.
puts "WG6C2 snap [gui snap C:/projects/WINARM/target/winui-wg6c.png]"

# Close cleanly, so the gate can assert the exit code rather than leaving a
# window behind for the next run's build to trip over. (Learned the hard way:
# a leftover macvm-winui.exe holds its own .exe open and the NEXT
# `cargo build -p winui_host` fails with "Access is denied".)
gui quit
