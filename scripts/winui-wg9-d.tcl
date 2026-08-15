# WG9 part D: the single-class re-run landed, and it replaced the whole report.
#
# That replacement is the assertion. `runTestsNamed:` answers the same line
# format as the full sweep, so the view's rows come out to exactly one — and a
# single-class run that merely APPENDED would leave the old failing row in place
# and show a green class beside a stale red one.
gui connect 7720
gui drain now
puts "WG9 one-rows [gui eval {WinShell testRows size}]"
puts "WG9 one-name [gui eval {(WinShell testRows first) at: 1}]"
puts "WG9 one-failed [gui eval {WinShell testTotalFailed}]"
puts "WG9 one-verdict [gui eval {(WinShell testsDisplayRows first) at: 1}]"
puts "WG9 snap-one [gui snap C:/projects/WINARM/target/winui-wg9-one.png]"
gui quit
