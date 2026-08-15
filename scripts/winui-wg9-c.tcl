# WG9 part C: the reply has landed. Read what the view knows.
#
# Asserted through the PURE side — `testRows`, `testTotalRun`, `testTotalFailed`
# and `testsDisplayRows` are computed without touching a window, so what the
# gate checks is exactly what the pane draws rather than a parallel summary that
# could agree with the tests and disagree with the screen.
gui connect 7720
gui drain now
puts "WG9 runs [gui eval {WinShell testRuns}]"
puts "WG9 pending [gui eval {WinShell testsPending}]"
puts "WG9 rows [gui eval {WinShell testRows size}]"
puts "WG9 assertions [gui eval {WinShell testTotalRun}]"
puts "WG9 failed [gui eval {WinShell testTotalFailed}]"
puts "WG9 failing-classes [gui eval {WinShell testFailingRows size}]"
puts "WG9 verdict [gui eval {(WinShell testsDisplayRows first) at: 1}]"
puts "WG9 first-failure [gui eval {(WinShell testFailingRows first) at: 4}]"
puts "WG9 error [gui eval {WinShell testsLastError}]"

# AND ONE CLASS ON ITS OWN, which is the loop anyone actually works in: fix a
# thing, re-run just that thing. Same reply format, so one parser serves both —
# a second format for the single-class case would be a second thing to drift.
puts "WG9 rerun-one [gui eval {WinShell runTestsNamed: 'WgNineOkTests'}]"
puts "WG9 snap [gui snap C:/projects/WINARM/target/winui-wg9.png]"
