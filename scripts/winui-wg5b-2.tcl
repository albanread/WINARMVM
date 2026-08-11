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

# ── WG6b: Find, and the jump that makes it a tool ───────────────────────
# A result you can only read is a report. The assertions are therefore about
# the JUMP as much as the search: after choosing a hit the Browser must be on
# that exact method, in the state clicking your way there would have produced.
gui send "0 0x111 [gui eval {(WinShell controlNamed: #view_find) id}] 0"
gui drain now
gui doit {(WinShell controlNamed: #findField) setText: 'printString'.}
puts "WG6B implementors [gui eval {WinShell find: #implementors}]"
puts "WG6B listbox-rows [gui eval {(WinShell controlNamed: #findResults) send: (WinApi constant: 'LB_GETCOUNT') wParam: 0 lParam: 0}]"
# Senders come from the persisted send index, not from a text search — the
# whole reason the feature is worth having.
puts "WG6B senders [gui eval {WinShell find: #senders}]"
# And the jump.
gui doit {(WinShell controlNamed: #findField) setText: 'printString'.}
gui eval {WinShell find: #implementors}
puts "WG6B jumped [gui eval {WinShell jumpToHit: (WinShell findCount)}]"
puts "WG6B landed-view [gui eval {WinShell activeView}]"
puts "WG6B landed-selector [gui eval {WinShell browserSelectedSelector}]"

# ── WG6a: the Outliner ──────────────────────────────────────────────────
# It renders the SAME tree the Browser fetched, so by the time part 2 runs
# the snapshot has landed and the tree can be built without a second wait.
# Row count is a RELATIONSHIP, never a frozen integer (WG2 Δ 14).
gui send "0 0x111 [gui eval {(WinShell controlNamed: #view_outliner) id}] 0"
gui drain now
puts "WG6A active-view [gui eval {WinShell activeView}]"
puts "WG6A rows [gui eval {WinShell rebuildOutliner}]"
puts "WG6A built [gui eval {WinShell outlinerIsBuilt}]"
# The composed struct size, asserted from the running window as well as from
# the headless tests — this is the one struct whose recorded size is a trap.
puts "WG6A tv-recorded [gui eval {WinApi sizeOf: 'TVINSERTSTRUCTW'}]"
puts "WG6A tv-composed [gui eval {WinShell tvInsertSize}]"

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
