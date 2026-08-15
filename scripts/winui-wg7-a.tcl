# winui-wg7-a.tcl — WG7-3 part A: ask the primary for its hierarchy.
#
# The browser fills from a REPLY across the seam, so every part of this gate is
# a separate script with a bash `sleep` between: `gui sleep` blocks the APP, not
# just the driver, and would starve the very drain pass it waits for. Same
# finding, same shape, as winui-wg5b.tcl records against itself.
gui connect 7715
gui drain now
puts "WG7 open [gui eval {WinShell isOpen}]"
puts "WG7 views-built-at-start [gui eval {WinShell viewBuildCount}]"
gui doit {WinShell refreshBrowser.}
