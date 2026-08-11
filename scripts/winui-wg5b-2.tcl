# winui-wg5b-2.tcl — the second half of WG5b's windowed gate, run after the
# recipe has let the window pump FREELY for a few seconds.
#
# Why a second file at all: `gui sleep` blocks the app rather than only this
# driver, so a wait inside part 1 starves the very drain pass it is waiting
# for. Twelve rounds of sleep-then-drain reported 0 refreshes after six
# seconds, which is indistinguishable from a browser that never filled. The
# bash `sleep` between the two scripts is the fix, and this file is what runs
# on the other side of it.

gui connect 7671
gui drain now

puts "WG5B browser-classes [gui eval {WinShell browserClassCount}]"
puts "WG5B browser-refreshes [gui eval {WinShell browserRefreshes}]"

# ── the source pane reads the IMAGE ─────────────────────────────────────
# WG5b-1's pane printed `Source comes from the image (image_store), which
# WG5b-2 wires up`. This asserts the promise was KEPT rather than restated:
# the sentence is gone, and an unselected browser asks for a class instead of
# showing an empty box, which reads as a failed load.
# WG5c D4: the recolour the EN_CHANGE in part 1 scheduled has had the
# recipe's whole wait to fire. The debounce is 200ms; the wait is 15s.
puts "WG5C passes-after-idle [gui eval {WinShell colourPasses}]"

# WG5c D3: the Browser's source pane is the OTHER swapped surface, and the
# one Accept reads from — so it must satisfy the same predicate.
puts "WG5C browser-source-is-source [gui eval {(WinShell controlNamed: #browserSource) isSourceSurface}]"
puts "WG5B source-empty [gui eval {WinShell sourceTextFor: nil selector: nil}]"
puts "WG5B source-still-a-promise [gui eval {(WinShell sourceTextFor: 'Point' selector: nil) includesSubstring: 'wires up'}]"

# ── the channel, from inside the REAL host process ──────────────────────
# The world suite already proves `library:` resolves. This proves it resolves
# HERE — in the process that actually owns the window, loading the DLL from
# the executable's own directory rather than from a test harness's.
puts "WG5B host-available [gui eval {WinHost available}]"
puts "WG5B host-ping [gui eval {WinHost ping}]"

# ── a picture, for the human half of the gate ───────────────────────────
puts "WG5B snap [gui snap C:/projects/WINARM/target/winui-wg5b.png]"

# Close cleanly, so the gate can assert the exit code rather than leaving a
# window behind for the next run's build to trip over. (Learned the hard way:
# a leftover macvm-winui.exe holds its own .exe open and the NEXT
# `cargo build -p win_gui` fails with "Access is denied".)
gui quit
