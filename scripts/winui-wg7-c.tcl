# Part C: the ghost is there, then RESTART THE PRIMARY IN PLACE and ask again.
gui connect 7715
gui drain now
puts "WG7 classes-with-ghost [gui eval {WinShell browserClassCount}]"
puts "WG7 restart [gui restart]"
gui drain now
gui doit {WinShell refreshBrowser.}
