# winui-wg4.tcl — drive the running macvm-winui and prove, from a script
# rather than from someone's eyes, that WG4's shell grammar holds: that views
# build LAZILY, that switching to a view already built builds nothing, that
# the transcript dock collapses and reopens as a height change, that the
# metrics cluster takes one sample at a time, and that every one of those
# happens through the DRAIN rather than in a handler.
#
#   MACVM_WINUI_CTL=7671 ./target/debug/macvm-winui.exe &
#   ./target/debug/macvm rusttcl scripts/winui-wg4.tcl
#
# Every line of the form `WG4 <key> <value...>` is read by `just gate-wg4`.
#
# The three mechanism notes from winui-wg3.tcl all still bind, and the first
# is the one this file leans on hardest: A MESSAGE THAT REACHES SMALLTALK MUST
# ORIGINATE OUTSIDE EVERY VM ENTRY. A view-bar click is therefore `gui send`
# (the host's control drain, where the pump acts) and never `gui doit` — a
# doit that posted WM_COMMAND to its own window would be declined by the busy
# guard, correctly, and the test would be measuring the guard instead of the
# shell.

gui connect 7671
puts "WG4 ping [gui ping]"
puts "WG4 open [gui eval {WinShell isOpen}]"
puts "WG4 enabled [gui eval {WinShell wg4Enabled}]"

# ── the registry and what is built at open ──────────────────────────────
# Three views registered; ONE of them built. That difference is the whole
# claim of "lazy" — a shell that built every view at open would answer 3 here
# and would cost every future view's handles at startup.
puts "WG4 views [gui eval {WinShell views size}]"
puts "WG4 built-at-open [gui eval {WinShell viewBuildCount}]"
puts "WG4 active-at-open [gui eval {WinShell activeView}]"

# ── a real click on a view-bar button ───────────────────────────────────
# WM_COMMAND (0x111) with the control's own id in wParam's low word, from
# outside every VM entry. The door FLAGS it; the drain switches the view one
# pass later. Both halves are asserted: the queue depth in the door, the
# effect after the pass.
gui send "0 0x111 [gui eval {(WinShell controlNamed: #view_transcript) id}] 0"
puts "WG4 queued-in-door [gui eval {WinShell pendingCommandCount}]"
gui drain now
puts "WG4 active-after-click [gui eval {WinShell activeView}]"
puts "WG4 built-after-click [gui eval {WinShell viewBuildCount}]"
puts "WG4 switches-after-click [gui eval {WinShell viewSwitchCount}]"

# ── switching to a view already built must build NOTHING ────────────────
# The counter is the test: a shell that rebuilt on every visit would leak a
# window handle per switch, and the leak would be invisible until it wasn't.
gui send "0 0x111 [gui eval {(WinShell controlNamed: #view_transcript) id}] 0"
gui drain now
puts "WG4 built-after-reswitch [gui eval {WinShell viewBuildCount}]"
puts "WG4 switches-after-reswitch [gui eval {WinShell viewSwitchCount}]"

# ── a third view builds on ITS first visit ──────────────────────────────
gui send "0 0x111 [gui eval {(WinShell controlNamed: #view_metrics) id}] 0"
gui drain now
puts "WG4 active-third [gui eval {WinShell activeView}]"
puts "WG4 built-third [gui eval {WinShell viewBuildCount}]"

# ── the transcript dock: a HEIGHT change, not a structural one ──────────
# `heightOfBottom:` is read on both sides of the toggle, because "collapsed"
# that removed the band would answer 0 here too and would then have to
# reconstruct the band on reopen — a different code path for a state the user
# can reach twice a minute.
puts "WG4 dock-collapsed-0 [gui eval {WinShell transcriptCollapsed}]"
puts "WG4 dock-height-0 [gui eval {WinShell layout heightOfBottom: #transcript}]"
gui doit {WinShell toggleTranscript.}
gui drain now
puts "WG4 dock-collapsed-1 [gui eval {WinShell transcriptCollapsed}]"
puts "WG4 dock-height-1 [gui eval {WinShell layout heightOfBottom: #transcript}]"
gui doit {WinShell toggleTranscript.}
gui drain now
puts "WG4 dock-collapsed-2 [gui eval {WinShell transcriptCollapsed}]"
puts "WG4 dock-height-2 [gui eval {WinShell layout heightOfBottom: #transcript}]"

# ── the metrics cluster ─────────────────────────────────────────────────
# One call, five values: a cluster showing a mixture of two samples is the
# small lie a metrics readout must not tell, so there is no per-field setter
# to test.
gui doit {WinShell updateMetricsMem: '540K/68M' jit: '1413' code: '82K' alloc: '2.1M' gc: '2/4'.}
puts "WG4 metrics-updates [gui eval {WinShell metricsUpdates}]"
puts "WG4 metrics-readback [gui eval {(WinShell controlNamed: #metrics) text}]"

# ── the transcript itself: newest first, and it BREAKS ──────────────────
# The line count is guest state; the readback is what Win32 actually holds,
# and the two must agree about how many lines there are. A Win32 multiline
# EDIT breaks on CRLF and on nothing else — the first WG4 snap ran every line
# together, and this is the assertion that would have caught it.
gui doit {WinShell appendTranscript: 'gate line one'.}
gui doit {WinShell appendTranscript: 'gate line two'.}
gui drain now
puts "WG4 transcript-first [gui eval {WinShell transcriptFirstLine}]"
puts "WG4 transcript-breaks [gui eval {((WinShell controlNamed: #transcript) text occurrencesOf: Character nl) >= 2}]"

# ── layout ran, and the shell's children are where the layout put them ──
puts "WG4 layouts [gui eval {WinShell layoutCount}]"
puts "WG4 metrics-right-of-buttons [gui eval {((WinShell controlNamed: #metrics) screenRect at: 1) > ((WinShell controlNamed: #doit) screenRect at: 1)}]"

puts "WG4 snap [gui snap C:/projects/WINARM/target/winui-wg4.png]"
puts "WG4 door [gui door]"
