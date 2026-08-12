# WG9 part A: the Tests view before there is anything to test, then File In.
#
# The empty screen is worth gating precisely because it looks like a broken
# view. The primary loads `world.list`, which has TestCase in it now but no
# SUBCLASSES — so the honest answer is "no test classes in the image", and
# failing to distinguish that from "the runner is broken" is what is being
# prevented.
gui connect 7720
gui drain now
puts "WG9 open [gui eval {WinShell isOpen}]"
puts "WG9 switched [gui eval {WinShell switchToView: #tests}]"
gui drain now
puts "WG9 built [gui eval {WinShell viewIsBuilt: #tests}]"
puts "WG9 pane [gui eval {WinShell testsPaneHwnd}]"

# THE VIEW RUNS ITSELF ON FIRST OPEN. A Tests tab that showed nothing until you
# found the verb would be a tab that mostly shows nothing.
# ISSUED, not COMPLETED. The reply is asynchronous, so asserting that a run
# has FINISHED microseconds after opening the view is asserting a race — it
# passed until the world grew (WG11 loaded the games) and the sweep took
# longer. What the view promises is that opening it STARTS a run.
puts "WG9 run-issued [gui eval {WinShell testRuns > 0 or: [ WinShell testsPending ]}]"
puts "WG9 rows-empty-image [gui eval {WinShell testRows size}]"
puts "WG9 snap-empty [gui snap C:/projects/WINARM/target/winui-wg9-empty.png]"

# NOW GIVE IT SOMETHING TO FIND. The recipe has already written two test classes
# to the file-in path — one passing, one failing, and the failing one is the
# point: a suite that can only report success is indistinguishable from a suite
# that always reports success.
#
# THE SOURCE IS WRITTEN BY THE RECIPE, NOT FROM HERE, and that is a Tcl fact
# rather than a design one: a Smalltalk method body is full of `[` and `]`, and
# inside a Tcl quoted string those are COMMAND SUBSTITUTION. Passing a class
# definition through this driver made Tcl try to run `self runTest: ...` as a
# command. The path is the one both sides derive independently — Python's
# `gettempdir` and the host's `std::env::temp_dir` both read TMP/TEMP.
puts "WG9 requested [gui eval {WinShell requestFileIn}]"
