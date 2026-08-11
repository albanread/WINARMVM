# winui-wg5b.tcl — drive the running macvm-winui and prove, from a script
# rather than from someone's eyes, that WG5b's Browser and Accept are really
# there: that the Accept cell exists and takes part in the same focus-driven
# enablement the other two verbs do, that the browser's panes filled from the
# primary's live hierarchy, and that the source pane now reads the IMAGE
# rather than printing a promise about it.
#
#   MACVM_WINUI_CTL=7671 ./target/debug/macvm-winui.exe &
#   ./target/debug/macvm rusttcl scripts/winui-wg5b.tcl
#
# Every line of the form `WG5B <key> <value...>` is read by `just gate-wg5b`.
#
# THE MECHANISM NOTE THAT STILL BINDS, and it is the one every winui script
# leans on: A MESSAGE THAT REACHES SMALLTALK MUST ORIGINATE OUTSIDE EVERY VM
# ENTRY. A view-bar click is therefore `gui send` (the host's control drain,
# where the pump acts) and never `gui doit` — a doit that posted WM_COMMAND
# to its own window would be declined by the busy guard, correctly, and the
# test would be measuring the guard instead of the shell.
#
# WHAT THIS FILE DOES NOT DO: write the image. The write path is gated
# separately, against a scratch image, by world/bench/wg5b_accept.mst. A
# windowed run uses whatever MACVM_IMAGE_PATH names, and a gate that wrote
# the developer's own image as a side effect of taking a screenshot would be
# a genuinely nasty thing to leave behind.

gui connect 7671
puts "WG5B ping [gui ping]"
puts "WG5B open [gui eval {WinShell isOpen}]"

# ── the Accept cell exists, and is a BAR cell ───────────────────────────
# Not decoration and not a view button: a bar cell, owner-drawn by the same
# path that draws Do It and Print It. A cell that existed but was not drawn
# by that path would look like a plain Win32 button in the middle of a
# Fluent strip — present, and obviously wrong.
puts "WG5B accept-exists [gui eval {(WinShell controlNamed: #accept) notNil}]"
puts "WG5B accept-is-bar-cell [gui eval {WinShell isBarControl: #accept}]"

# ── enablement follows FOCUS, exactly as WG4 D4 built it ────────────────
# The claim is that Accept JOINED the existing rule rather than acquiring one
# of its own. Proven in both directions, because a cell stuck ON looks
# identical to a working one right up until the moment it matters.
gui doit {WinApi setFocus: (WinShell controlNamed: #transcript) handle.}
gui doit {WinShell checkFocusChanged.}
puts "WG5B accept-on-readonly [gui eval {(WinShell controlNamed: #accept) isEnabled}]"

# ── the Browser, over the primary's live hierarchy ──────────────────────
# A real WM_COMMAND (0x111) from outside every VM entry, then the panes. The
# class count is a RELATIONSHIP, never a frozen integer (WG2 Δ 14): the world
# grows, and a gate that pinned today's count would fail on the next file.
gui send "0 0x111 [gui eval {(WinShell controlNamed: #view_browser) id}] 0"
gui drain
puts "WG5B active-view [gui eval {WinShell activeView}]"
# The panes fill from a REPLY, not from the switch: `refreshBrowser` ships a
# #uiReq to the primary and the tree arrives on a later drain pass. THE WAIT
# IS NOT HERE, and that is the finding: `gui sleep` blocks the APP, not just
# this driver, so twelve rounds of sleep-then-drain delivered nothing at all
# (0 refreshes after six seconds -- indistinguishable from a browser that
# never filled). The recipe disconnects and waits in bash instead, letting
# the window pump freely, then runs winui-wg5b-2.tcl. Split for that reason
# alone.
# Part 1 ends here. `just gate-wg5b` now waits in bash and runs
# scripts/winui-wg5b-2.tcl, which reads the panes and closes the window.
