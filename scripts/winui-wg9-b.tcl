# WG9 part B: the primary has restarted with the filed-in classes. Run them.
#
# Separate script and a bash sleep between, for the reason winui-wg5b.tcl
# recorded first: `gui sleep` blocks the APP, and what we are waiting for is the
# app's own pump doing the restart and then servicing a worker reply. Sleeping
# inside it would starve the very work being waited on.
gui connect 7720
gui drain now
puts "WG9 run-again [gui eval {WinShell runTests}]"
