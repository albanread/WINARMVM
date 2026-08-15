# WG10 — the gallery, captured.
#
# Four modes on one Canvas, each a claim about the machinery underneath rather
# than decoration. What is asserted here is CHEAP and specific: that each mode
# draws, that Life is genuinely computing (generations advance and the
# population changes without ever saturating or dying), and that Julia's frames
# come out of integer arithmetic. The pictures are captured for
# `docs/gallery-win/`; the RULES are gated in the world suite, where they can be
# checked without a GPU.
gui connect 7723
gui drain now
gui doit {WinShell switchToView: #canvas}
gui drain now

# 1. PLASMA — direct BGRA, every pixel every frame.
puts "WG10 plasma-mode [gui eval {WinShell canvasMode: #plasma}]"
puts "WG10 plasma [gui eval {WinShell renderCanvas}]"
puts "WG10 plasma-label [gui eval {WinShell canvasLabel}]"
puts "WG10 snap-plasma [gui snap C:/projects/WINARM/docs/gallery-win/plasma.png]"

# 2. PALETTE — 256 stores a frame move nineteen thousand pixels.
puts "WG10 palette-mode [gui eval {WinShell canvasMode: #palette}]"
puts "WG10 palette [gui eval {WinShell renderCanvas}]"
puts "WG10 snap-palette [gui snap C:/projects/WINARM/docs/gallery-win/palette.png]"

# 3. LIFE — the indexed plane. Generations are SLOTS; the palette themes a
#    running simulation without the rules knowing a colour exists.
puts "WG10 life-mode [gui eval {WinShell canvasMode: #life}]"
puts "WG10 life-reset [gui eval {WinShell resetLife. WinShell lifeGeneration}]"
puts "WG10 life-seed-pop [gui eval {WinShell lifePopulation}]"
puts "WG10 life-1 [gui eval {WinShell renderCanvas}]"
puts "WG10 life-pop-1 [gui eval {WinShell lifePopulation}]"
puts "WG10 life-2 [gui eval {WinShell renderCanvas}]"
puts "WG10 life-3 [gui eval {WinShell renderCanvas}]"
puts "WG10 life-4 [gui eval {WinShell renderCanvas}]"
puts "WG10 life-gen [gui eval {WinShell lifeGeneration}]"
puts "WG10 life-pop [gui eval {WinShell lifePopulation}]"
puts "WG10 life-label [gui eval {WinShell canvasLabel}]"
puts "WG10 snap-life [gui snap C:/projects/WINARM/docs/gallery-win/life.png]"

# 4. JULIA — per-pixel arithmetic at full rate from Smalltalk, in fixed point.
#    A Float per pixel would allocate six hundred thousand times a frame.
puts "WG10 julia-mode [gui eval {WinShell canvasMode: #julia}]"
puts "WG10 julia [gui eval {WinShell renderCanvas}]"
puts "WG10 julia-2 [gui eval {WinShell renderCanvas}]"
puts "WG10 julia-label [gui eval {WinShell canvasLabel}]"
puts "WG10 snap-julia [gui snap C:/projects/WINARM/docs/gallery-win/julia.png]"

puts "WG10 frames [gui eval {WinShell canvasFrames}]"
puts "WG10 error [gui eval {WinRender lastError}]"
gui quit
