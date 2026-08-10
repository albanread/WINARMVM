# winui-wg2.tcl — drive the running macvm-winui from RUSTTCL and prove, from a
# script rather than from someone's eyes, that Windows messages reach Smalltalk
# and that none of the three ways a handler can fail breaks the window.
#
#   MACVM_WINUI_CTL=7650 ./target/debug/macvm-winui.exe &
#   ./target/debug/macvm rusttcl scripts/winui-wg2.tcl
#
# WG1's Δ 8 is what makes this a script at all: `eval` answers INLINE here (the
# VM is on the pump's own thread), so every line below reads a value rather than
# sleeping and hoping. Every line of the form `WG2 <key> <value...>` is read by
# `just gate-wg2`.
#
# TWO MECHANISM NOTES, both measured, because they shape the whole file.
#
# 1. tests_wg2.md item 2 says to drive the resize with "SetWindowPos via FFI
#    from a doit". That cannot reach Smalltalk: SetWindowPos SENDS WM_SIZE
#    synchronously, so from a doit the window proc runs inside `vm.exec` — a
#    second top-level VM entry on a thread that already has one, sharing the
#    single per-thread sigsetjmp slot — and the door refuses it, correctly. A
#    WM_SIZE that reaches Smalltalk has to originate OUTSIDE every VM entry,
#    which is where every real one does: the pump, or the modal move/size loop
#    Windows runs while a user drags the frame. `gui resize` is that place.
#
# 2. The vendored Tcl is deliberately small: `while` and `foreach` but no `for`,
#    `expr` with braces but no `%`, and NO `eq`/`ne` and no `string` command.
#    So nothing below compares strings in Tcl. Every accumulation happens in
#    SMALLTALK — `messagesSeen` counts ARRIVALS and `sizeCount` counts
#    SUCCESSES — and the two numbers are printed for `just gate-wg2` to assert.
#    That is a better test anyway: it is the guest's own record of what
#    happened to it.

gui connect 7650
puts "WG2 ping [gui ping]"
puts "WG2 iswindow [gui eval {WinShell isOpen}]"
puts "WG2 dooraddr [gui eval {WinApi wndProcAddress}]"

# ── item 2: a message reaches Smalltalk ────────────────────────────────────
# The assertion is a CROSS-CHECK, not a tautology: `lastSizeSeen` is decoded
# from the MESSAGE's lParam (low word width, high word height) and the client
# rect comes from GetClientRect. Two independent sources of the same number.
gui door reset
gui doit {WinShell resetDoorCounters.}
puts "WG2 resize1 [gui resize {700 500}]"
puts "WG2 lastsize [gui eval {WinShell lastSizeSeen}]"
puts "WG2 client [gui eval {WinShell clientWidth}] [gui eval {WinShell clientHeight}]"
puts "WG2 sizematches [gui eval {WinShell lastSizeMatchesClientRect}]"
puts "WG2 sizecount [gui eval {WinShell sizeCount}]"
puts "WG2 door-after-resize [gui door]"

# ── item 4: a RAISING handler does not break the window ────────────────────
# Twenty raises interleaved with twenty good messages (tests_wg2.md's stress
# row). `messagesSeen` must reach 40 — every message ARRIVED, including the
# ones whose handler blew up, because the counter is bumped before any handler
# can fail — while `sizeCount` must reach exactly 20, because only the good
# ones COMPLETED. That pair is the whole property: the window keeps working
# (DefWindowProcW answered the failures) and the next message still dispatches
# into Smalltalk.
gui door reset
gui doit {WinShell resetDoorCounters.}
set i 0
while {$i < 20} {
    gui doit {WinShell handlerMode: #raise.}
    gui resize "[expr {600 + $i}] 460"
    gui doit {WinShell handlerMode: nil.}
    gui resize "[expr {640 + $i}] 480"
    incr i
}
puts "WG2 raise-messagesseen [gui eval {WinShell messagesSeen}]"
puts "WG2 raise-sizecount [gui eval {WinShell sizeCount}]"
puts "WG2 raise-lastsize [gui eval {WinShell lastSizeSeen}]"
puts "WG2 raise-matches [gui eval {WinShell lastSizeMatchesClientRect}]"
puts "WG2 raise-alive [gui eval {3 + 4}]"
puts "WG2 door-after-raise [gui door]"

# ── item 5: a FAULTING handler does not break the process ──────────────────
# A wild dereference inside the handler: P2's VEH catches the ACCESS_VIOLATION
# and its non-unwinding longjmp lands inside dispatch_callback — SKIPPING every
# ordinary return between. If the door's depth guard were released by a
# decrement rather than by an RAII drop it would leak HERE, and every later
# message would be declined as nested, forever, with nothing in any log. So the
# check is not "did it survive" but "does the NEXT message still get through".
gui door reset
gui doit {WinShell resetDoorCounters.}
set i 0
while {$i < 5} {
    gui doit {WinShell handlerMode: #fault.}
    gui resize "[expr {660 + $i}] 500"
    incr i
}
puts "WG2 fault-messagesseen [gui eval {WinShell messagesSeen}]"
puts "WG2 fault-sizecount [gui eval {WinShell sizeCount}]"
puts "WG2 fault-lastsize [gui eval {WinShell lastSizeSeen}]"
gui doit {WinShell handlerMode: nil.}
gui resize {720 520}
puts "WG2 fault-then-sizecount [gui eval {WinShell sizeCount}]"
puts "WG2 fault-then-lastsize [gui eval {WinShell lastSizeSeen}]"
puts "WG2 fault-then-matches [gui eval {WinShell lastSizeMatchesClientRect}]"
puts "WG2 fault-alive [gui eval {3 + 4}]"
puts "WG2 fault-ping [gui ping]"
puts "WG2 door-after-fault [gui door]"

# ── item 6: nesting degrades safely ────────────────────────────────────────
# The handler SendMessageW's its own window. That is a nested wndproc call on
# this thread with the door already open, so the depth guard declines it and
# DefWindowProcW answers — and the counter is back to 0 afterwards.
# `sizeCount` = 1 is the load-bearing half: the OUTER WM_SIZE completed and the
# nested one never reached Smalltalk at all, so the handler ran once, not twice.
gui door reset
gui doit {WinShell resetDoorCounters.}
gui doit {WinShell handlerMode: #nest.}
gui resize {760 540}
puts "WG2 nest-result [gui eval {WinShell nestedSendResult}]"
puts "WG2 nest-messagesseen [gui eval {WinShell messagesSeen}]"
puts "WG2 nest-sizecount [gui eval {WinShell sizeCount}]"
puts "WG2 door-after-nest [gui door]"
gui doit {WinShell handlerMode: nil.}

# ── item 7: D5's two numbers, warm ─────────────────────────────────────────
# Warm-up first, and separately: the first WM_SIZE through the door pays for
# the handler's inline caches and its tier-1 compilation, and averaging that in
# would describe the first message rather than the millionth. 40 warm-up
# resizes, then reset, then the 200 that count (which are also the sprint's own
# stress row: `lastSizeSeen` tracks the final rect and the guard is 0 at the
# end).
set i 0
while {$i < 40} {
    gui resize "[expr {600 + $i}] 440"
    incr i
}
gui door reset
gui doit {WinShell resetDoorCounters.}
set i 0
while {$i < 200} {
    gui resize "[expr {600 + $i}] [expr {440 + $i}]"
    incr i
}
puts "WG2 stress-lastsize [gui eval {WinShell lastSizeSeen}]"
puts "WG2 stress-matches [gui eval {WinShell lastSizeMatchesClientRect}]"
puts "WG2 stress-sizecount [gui eval {WinShell sizeCount}]"
puts "WG2 latency-door [gui door]"

# The BASELINE: the same message DefWindowProcW'd with the allowlist off, in
# the same warm process, on the same clock, at the same call site. `door off`
# empties the allowlist, so `sizeCount` must stay 0 through all 200 — which is
# also implementation-order step 1's transparency claim, checked here rather
# than only asserted.
gui door off
gui doit {WinShell resetDoorCounters.}
set i 0
while {$i < 40} {
    gui resize "[expr {600 + $i}] 440"
    incr i
}
gui door reset
set i 0
while {$i < 200} {
    gui resize "[expr {600 + $i}] [expr {440 + $i}]"
    incr i
}
puts "WG2 offdoor-sizecount [gui eval {WinShell sizeCount}]"
puts "WG2 offdoor-messagesseen [gui eval {WinShell messagesSeen}]"
puts "WG2 latency-base [gui door]"
gui door on

# ── the camera, during traffic ─────────────────────────────────────────────
# A snap right after a burst: the door must not starve the control channel's
# drain. `just gate-wg2` checks the PNG's own IHDR against the client rect
# printed here, because a file that merely exists proves nothing.
gui resize {900 600}
# The gallery shot's own evidence. WG2 paints nothing (that is WG3/WG4's), so
# a picture of the client area would show a flat fill and prove only that a
# window exists. Putting the size the MESSAGE carried into the titlebar makes
# the shot show the thing the sprint built: Windows sent WM_SIZE, Smalltalk
# decoded its lParam, and Smalltalk wrote what it read back onto the frame.
gui doit {WinShell setTitle: 'WG2 — WM_SIZE handled in Smalltalk: ' , WinShell lastSizeSeen printString.}
gui sleep 300
puts "WG2 title [gui eval {WinShell title}]"
puts "WG2 snapclient [gui eval {WinShell clientWidth}] [gui eval {WinShell clientHeight}]"
# The PNG is sized by GetWindowRect, not GetClientRect — gui/src/shell/snap.rs
# says so in its own doc, and gate-wg1 compares it against the CLIENT rect,
# which differs by the frame (916x639 against 900x600 on this machine). The
# gate reads THIS line instead. A capture's size is machine-checkable evidence
# only if it is checked against the thing that was actually captured.
puts "WG2 winrect [gui eval {(WinShell windowRect at: 3) - (WinShell windowRect at: 1)}] [gui eval {(WinShell windowRect at: 4) - (WinShell windowRect at: 2)}]"
puts "WG2 bg [gui eval {WinShell backgroundColorRef}]"
puts "WG2 snap [gui snap C:/projects/WINARM/target/winui-wg2.png]"
puts "WG2 finaldoor [gui door]"

# ── item 3: WM_DESTROY posts the quit FROM SMALLTALK ───────────────────────
# WM_CLOSE reaches `onClose`, which answers #defwindowproc so that the
# DestroyWindow which follows happens on the FAR SIDE of the depth guard — and
# the WM_DESTROY it provokes is therefore a fresh top-level entry that reaches
# `onDestroy`, which posts the quit. The gate greps the host's log for
# Smalltalk's line and for the ABSENCE of the host's backstop line.
#
# The reply may not arrive (the app is on its way down), so this is deliberately
# the last thing the script does.
puts "WG2 close [gui doit {WinShell close.}]"
puts "WG2 done"
