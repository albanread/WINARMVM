# gui-snap.tcl — drive the running macvm-gui from RUSTTCL and capture it.
#
#   MACVM_GUI_CTL=7645 ./target/release/macvm-gui.exe &
#   ./target/release/macvm run --rusttcl scripts/gui-snap.tcl
#
# The Cocoa app has been scriptable since CG5; this is the same protocol
# against the cross-platform GUI, so one client drives either.
gui connect 7645
puts [gui ping]
gui sleep 2500
puts [gui snap C:/projects/WINARM/target/gui-shot-start.png]
gui doit "Transcript show: 'HELLO-FROM-TCL'; cr."
gui sleep 1200
puts [gui snap C:/projects/WINARM/target/gui-shot-transcript.png]
