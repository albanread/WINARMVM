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

# ── the bar painted its FIRST frame ─────────────────────────────────────
# A BS_OWNERDRAW cell is drawn by nobody until someone invalidates it, and
# for a while nobody did: the strip came up as a row of empty boxes and only
# filled in when an unrelated event happened to dirty it. Reported as `where
# is the rest of the app?`. Non-zero draw calls before anything has been
# clicked is the whole assertion.
puts "WG5B drawcalls-at-open [gui eval {WinShell drawCalls}]"

# ── A VERB MUST SURVIVE BEING CLICKED ───────────────────────────────────
# The bug this pins was as bad as a bug gets: clicking `Do It` gave the
# BUTTON focus, a button is not a source surface, so enablement disabled it
# — and a disabled button never sends WM_COMMAND. The verb switched itself
# off on the way down and the click vanished. `no response from print it or
# doit`, and correctly.
#
# WG4 D4's own tests missed it because they set focus synthetically and
# asserted both directions, but never made the ONE transition a human makes
# every time: source surface -> verb cell. Asserted here in exactly that
# order, because the order IS the bug.
gui doit {WinApi setFocus: (WinShell controlNamed: (WinShell contentNameFor: #workspace)) handle.}
gui doit {WinShell checkFocusChanged.}
puts "WG5B enabled-on-workspace [gui eval {WinShell commandsEnabled}]"
gui doit {WinApi setFocus: (WinShell controlNamed: #doit) handle.}
gui doit {WinShell checkFocusChanged.}
puts "WG5B enabled-after-clicking-a-verb [gui eval {WinShell commandsEnabled}]"
puts "WG5B doit-still-clickable [gui eval {(WinShell controlNamed: #doit) isEnabled}]"

# ── and Do It really runs, through a real WM_COMMAND ────────────────────
gui doit {(WinShell controlNamed: (WinShell contentNameFor: #workspace)) setText: '3 + 4 * 2'.}
gui doit {WinApi setFocus: (WinShell controlNamed: (WinShell contentNameFor: #workspace)) handle.}
gui drain now
gui send "0 0x111 [gui eval {(WinShell controlNamed: #doit) id}] 0"
gui drain now
puts "WG5B doit-fired [gui eval {WinShell doItCount}]"

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
