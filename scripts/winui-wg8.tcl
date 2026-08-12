# WG8 / SM0 — the pixel plane, in the view it belongs in.
#
# The first version of this demo drew plasma into the EDITOR's pane, which was
# the wrong place and was called as such. So the first thing this gate asserts
# is not that pixels appear but that a CANVAS exists to put them in — a view
# with its own pane, its own renderer and its own paint route. The plasma is
# what proves the plane; the view is what makes the plane mean something.
gui connect 7716
gui drain now

# THE VIEW. Registered, switchable, and built — the same three facts every
# other view in this shell has to satisfy, asserted the same way.
puts "WG8 open [gui eval {WinShell isOpen}]"
puts "WG8 switched [gui eval {WinShell switchToView: #canvas}]"
gui drain now
puts "WG8 active [gui eval {WinShell activeView}]"
puts "WG8 built [gui eval {WinShell viewIsBuilt: #canvas}]"
puts "WG8 pane-hwnd [gui eval {WinShell canvasPaneHwnd}]"

# THE PLANE. Stride comes from the RENDERER — it is the number the guest is
# forbidden to assume, and 160 BGRA pixels is 640 bytes only if D3D chose not
# to pad the row. Whatever it answers, asserting it is non-zero is asserting
# the map succeeded.
puts "WG8 stride [gui eval {WinPixels strideFor: WinShell canvasPaneHwnd}]"
puts "WG8 plane [gui eval {(WinPixels planeFor: WinShell canvasPaneHwnd width: 160 height: 120) > 0}]"

# IT MOVES. One frame proves the buffer reaches the screen; two frames with
# different phases prove nothing is frozen — and a still image that happened to
# be right once is exactly the failure a single-frame gate cannot see.
puts "WG8 frames-1 [gui eval {WinShell canvasFrames}]"
puts "WG8 render-a [gui eval {WinShell renderCanvas}]"
puts "WG8 render-b [gui eval {WinShell renderCanvas}]"
puts "WG8 frames-2 [gui eval {WinShell canvasFrames}]"
puts "WG8 phase [gui eval {WinShell plasmaPhase}]"

# BOTH PLANES COMPOSE. The HUD's cells carry `transparentBackground`, so the
# renderer skips their background fill and the plane shows through. If that
# constant ever stopped matching the Rust side's BG_TRANSPARENT the HUD would
# paint a solid bar over the pixels and this is the line that would say so.
puts "WG8 transparent [gui eval {WinPixels transparentBackground}]"
puts "WG8 present-frames [gui eval {WinRender framesFor: WinShell canvasPaneHwnd}]"
puts "WG8 last-error [gui eval {WinRender lastError}]"

puts "WG8 snap [gui snap C:/projects/WINARM/target/winui-wg8.png]"
gui quit
