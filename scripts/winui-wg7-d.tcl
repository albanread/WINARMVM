# Part D: the world is fresh and the WINDOW never noticed.
#
# Those are the two halves of WG7-3's contract and they pull in opposite
# directions: indistinguishable from a fresh boot TO THE WORLD, completely
# invisible TO THE WINDOW. A restart that rebuilt the views would pass the
# first and fail the second, and would be a window blink the user did not ask
# for.
gui connect 7715
gui drain now
puts "WG7 classes-after-restart [gui eval {WinShell browserClassCount}]"
puts "WG7 window-alive [gui eval {WinShell isOpen}]"
puts "WG7 views-built-after [gui eval {WinShell viewBuildCount}]"
puts "WG7 active-view [gui eval {WinShell activeView}]"
puts "WG7 snap [gui snap C:/projects/WINARM/target/winui-wg7.png]"
gui quit
