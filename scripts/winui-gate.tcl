# winui-gate.tcl — drive the running macvm-winui from RUSTTCL and prove, from
# a script rather than from someone's eyes, that WG1's window is real.
#
#   MACVM_WINUI_CTL=7649 ./target/debug/macvm-winui.exe &
#   ./target/debug/macvm run --rusttcl scripts/winui-gate.tcl
#
# The protocol is byte-identical to the Cocoa channel's and to macvm-gui's, so
# the SAME `gui` verbs drive all three with no client-side branch
# (docs/win_gui_design.md §3.1 — WG1 inherits this channel, it does not build
# one). The one WG1-specific fact is the env var, MACVM_WINUI_CTL, which keeps
# both apps drivable in one session.
#
# Every line this prints in the form `WG1 <key> <value...>` is read by
# `just gate-wg1`; the PNG's own dimensions are checked there against the
# client rect printed here, because a file that merely exists proves nothing.

gui connect 7649
puts "WG1 ping [gui ping]"

# The window's own facts, read back THROUGH the FFI in this same session —
# GetClientRect and GetDpiForWindow answered by Win32 a moment ago, not by
# anything this script or the shell remembered.
puts "WG1 hwnd [gui eval {WinShell hwndValue}]"
puts "WG1 iswindow [gui eval {WinShell isOpen}]"
puts "WG1 dpi [gui eval {WinShell dpi}]"
puts "WG1 client [gui eval {WinShell clientWidth}] [gui eval {WinShell clientHeight}]"
puts "WG1 expected [gui eval {WinShell expectedClientWidth}] [gui eval {WinShell expectedClientHeight}]"
# The WINDOW rect, added by WG2: `snap` is `PrintWindow` of the whole window and
# sizes its bitmap with GetWindowRect (gui/src/shell/snap.rs, whose own doc
# records that the older client-sized version satisfied the gate's equality
# while silently cropping the bottom of the client area). The gate compares the
# PNG against THIS line now, because a capture's size is evidence only when it
# is checked against the thing that was captured.
puts "WG1 window [gui eval {(WinShell windowRect at: 3) - (WinShell windowRect at: 1)}] [gui eval {(WinShell windowRect at: 4) - (WinShell windowRect at: 2)}]"
puts "WG1 threadinvariant [gui eval {WinShell threadInvariantHolds}]"
puts "WG1 mica [gui eval {WinShell micaHResult}] took [gui eval {WinShell micaTook}]"
puts "WG1 dark [gui eval {WinShell darkHResult}] took [gui eval {WinShell darkTitlebarTook}]"
puts "WG1 darkpref [gui eval {WinShell darkMode}] appsUseLightTheme [gui eval {WinShell appsUseLightTheme}]"
# The client area's fill, as a COLORREF (0x00BBGGRR). The gate reads the same
# three bytes out of the PNG, which makes the capture evidence about the
# window's CONTENT and not only its size — and makes the registry read's
# effect visible in a file rather than only in a variable.
puts "WG1 bg [gui eval {WinShell backgroundColorRef}]"
puts "WG1 title-before [gui eval {WinShell title}]"

# The camera. `snap` is PrintWindow + PW_RENDERFULLCONTENT, the same code
# macvm-gui uses, pointed at an HWND it has never seen before.
gui sleep 400
puts "WG1 snap [gui snap C:/projects/WINARM/target/winui-wg1.png]"

# Gate item 5: a doit changes the REAL titlebar, and the proof is
# GetWindowTextW reading it back out of Win32 — not a variable WinShell kept.
puts "WG1 settitle [gui doit {WinShell setTitle: 'WG1-OK'.}]"
gui sleep 200
puts "WG1 title-after [gui eval {WinShell title}]"
gui sleep 300
puts "WG1 snap2 [gui snap C:/projects/WINARM/target/winui-wg1-titled.png]"

# Gate item 6: a doit posts WM_CLOSE — the same message the titlebar's X sends.
# DefWindowProcW destroys the window, the pump notices its window has gone and
# posts WM_QUIT, the loop ENDS and the process exits 0. Not killed, not timed
# out. The reply may not arrive at all (the app is on its way down by then),
# so this is deliberately the last thing the script does.
puts "WG1 close [gui doit {WinShell close.}]"
puts "WG1 done"
