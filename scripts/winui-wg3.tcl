# winui-wg3.tcl — drive the running macvm-winui from RUSTTCL and prove, from a
# script rather than from someone's eyes, that the flag-and-drain pass works:
# that it COALESCES, that a modal loop suppresses it, that it never re-enters,
# and that real Win32 controls are created by Smalltalk, notified through WG2's
# door and laid out by a Smalltalk layout object.
#
#   MACVM_WINUI_CTL=7651 ./target/debug/macvm-winui.exe &
#   ./target/debug/macvm rusttcl scripts/winui-wg3.tcl
#
# Every line of the form `WG3 <key> <value...>` is read by `just gate-wg3`.
#
# THREE MECHANISM NOTES, all measured, because they shape the whole file.
#
# 1. WG2's Δ 3, now permanent for every WG gate: **a message that reaches
#    Smalltalk must originate OUTSIDE every VM entry.** A doit that calls
#    SetWindowPos, or that sends WM_COMMAND to its own window, delivers the
#    message into a VM that is already inside `exec` — and the busy guard
#    correctly refuses it. So nothing below drives a control from a doit.
#    `gui resize`, `gui burst`, `gui track` and `gui send` all act from the
#    host's control drain, which is the same place the pump and Windows' own
#    modal move/size loop act from.
#
# 2. **Coalescing is only observable in a BURST.** One `gui resize` per round
#    trip lets the pump service each wake before the next message arrives, and
#    a 1:1 pass-to-message ratio is the CORRECT answer at that rate — the drain
#    is not meant to skip work nothing is racing. `gui burst <n>` issues N
#    SetWindowPos calls with no pump turn between them, which is what a real
#    storm looks like (a frame drag, a cascade of layout changes) and the only
#    shape in which `drainPasses < sizeCount` means anything.
#
# 3. The vendored Tcl has `while` and `foreach` but **no `for`**, `expr` needs
#    braces and has **no `%`**, and there is **no `eq`/`ne` and no `string`**
#    (WG2 Δ 13). Every accumulation is therefore in SMALLTALK — `sizeCount`
#    counts arrivals, `drainPasses` counts passes, `layoutCount` counts work —
#    which is a better test anyway: it is the guest's own record of what
#    happened to it.

gui connect 7651
puts "WG3 ping [gui ping]"
puts "WG3 iswindow [gui eval {WinShell isOpen}]"

# ── Part 1, item 2: flags are serviced ─────────────────────────────────────
# One resize sets a flag; the pass clears it and records the SETTLED size. The
# assertion is a cross-check of two independent sources: `lastLayoutSize` is
# what the DRAIN read from GetClientRect, `client` is what GetClientRect says
# now, and `lastSizeSeen` is what the MESSAGE's lParam carried.
gui door reset
gui doit {WinShell resetDoorCounters.}
gui resize {700 500}
gui drain now
puts "WG3 flag-lastlayout [gui eval {WinShell lastLayoutSize}]"
puts "WG3 flag-client [gui eval {WinShell clientWidth}] [gui eval {WinShell clientHeight}]"
puts "WG3 flag-matches [gui eval {WinShell lastLayoutMatchesClientRect}]"
puts "WG3 flag-pending [gui eval {WinShell pendingLayout}]"
puts "WG3 flag-layouts [gui eval {WinShell layoutCount}]"

# ── Part 1, item 3: COALESCING, measured ───────────────────────────────────
# 200 resizes in one burst, then one wake. `drainPasses` must be far below
# `sizeCount`; the RATIO is reported, not asserted to a constant, because a
# constant would be a reading and not a property (WG2 Δ 14). A ratio of 1.0
# would mean the drain is not coalescing and this sprint's central claim is
# false — so the gate says so in those words if it ever sees one.
gui door reset
gui doit {WinShell resetDoorCounters.}
puts "WG3 burst [gui burst {200}]"
puts "WG3 coalesce-midburst-passes [gui eval {WinShell drainPasses}]"
gui drain now
puts "WG3 coalesce-sizecount [gui eval {WinShell sizeCount}]"
puts "WG3 coalesce-passes [gui eval {WinShell drainPasses}]"
puts "WG3 coalesce-layouts [gui eval {WinShell layoutCount}]"
puts "WG3 coalesce-matches [gui eval {WinShell lastLayoutMatchesClientRect}]"
puts "WG3 coalesce-drain [gui drain]"

# ── Part 1, item 4: TRACKING SUPPRESSES ────────────────────────────────────
# WM_ENTERSIZEMOVE -> 30 resizes -> WM_EXITSIZEMOVE, with the real messages,
# sent from the drain. ZERO passes may run while tracking: Windows' own modal
# loop pumps the queue itself, so both the posted wake and the heartbeat are
# delivered INSIDE it, and a layout pass running from that stack is the thing
# D2 exists to prevent. Exactly one pass runs after, against the final size.
gui door reset
gui doit {WinShell resetDoorCounters.}
puts "WG3 track-on [gui track on]"
gui burst {30 800 560}
gui drain now
puts "WG3 track-during-passes [gui eval {WinShell drainPasses}]"
puts "WG3 track-during-sizecount [gui eval {WinShell sizeCount}]"
puts "WG3 track-during-drain [gui drain]"
puts "WG3 track-off [gui track off]"
gui drain now
puts "WG3 track-after-passes [gui eval {WinShell drainPasses}]"
puts "WG3 track-after-layouts [gui eval {WinShell layoutCount}]"
puts "WG3 track-after-matches [gui eval {WinShell lastLayoutMatchesClientRect}]"
puts "WG3 track-enters [gui eval {WinShell trackEnterCount}]"
puts "WG3 track-exits [gui eval {WinShell trackExitCount}]"

# ── Part 1, item 5: the drain never re-enters ──────────────────────────────
# A flag set DURING a pass may not be posted from inside the pass — that is how
# a drain becomes a spin — so `drainPass` answers non-zero and the HEARTBEAT
# picks it up. The pass count must go up and then STOP: bounded, not exact.
gui door reset
gui doit {WinShell resetDoorCounters.}
gui doit {WinShell askForAnotherPass.}
gui drain now
puts "WG3 reask-immediate [gui eval {WinShell drainPasses}]"
gui sleep 400
gui drain
puts "WG3 reask-after-heartbeat [gui eval {WinShell drainPasses}]"
puts "WG3 reask-count [gui eval {WinShell reaskCount}]"
gui sleep 400
puts "WG3 reask-settled [gui eval {WinShell drainPasses}]"
puts "WG3 reask-drain [gui drain]"

# ── stress: a pass that RAISES, and one that FAULTS ────────────────────────
# The pass must complete, the flag must be CLEARED (no infinite retry), and the
# next pass must run normally. The fault is a real ACCESS_VIOLATION recovered by
# P2's VEH + non-unwinding longjmp — the same path WG2 proved for handlers, now
# one layer up, where a leaked depth guard would disable the drain forever with
# nothing in any log.
gui door reset
gui doit {WinShell resetDoorCounters.}
gui doit {WinShell raiseInNextPass.}
gui resize {820 540}
gui drain now
puts "WG3 raisepass-passes [gui eval {WinShell drainPasses}]"
puts "WG3 raisepass-pending [gui eval {WinShell pendingLayout}]"
gui resize {830 545}
gui drain now
puts "WG3 raisepass-then-layouts [gui eval {WinShell layoutCount}]"
puts "WG3 raisepass-then-matches [gui eval {WinShell lastLayoutMatchesClientRect}]"
gui doit {WinShell faultInNextPass.}
gui resize {840 550}
gui drain now
puts "WG3 faultpass-passes [gui eval {WinShell drainPasses}]"
gui resize {850 555}
gui drain now
puts "WG3 faultpass-then-matches [gui eval {WinShell lastLayoutMatchesClientRect}]"
puts "WG3 faultpass-alive [gui eval {3 + 4}]"
puts "WG3 faultpass-ping [gui ping]"
puts "WG3 faultpass-drain [gui drain]"

# ── Part 2, item 6: a BUTTON, clicked, whose meaning runs in the drain ─────
# The click is a real WM_COMMAND with the button's own id in wParam's low word
# and BN_CLICKED (0) in its high word, sent from the control drain — where
# Windows sends it from. The handler sets a flag; the ACTION (two SendMessageW
# calls into two other controls, plus a list insertion) runs one pass later,
# and is proven by SMALLTALK state and by text read back out of Win32 — not by
# a Rust counter.
#
# Observing the INTERMEDIATE state — click recorded, meaning not yet run — needs
# the drain held off for the length of one round trip, because every control
# request the script makes lets the pump turn and service the wake. D2's own
# mechanism is the way to hold it off: `gui track on` puts the door in the state
# a window drag puts it in, and drains are suppressed until it is cleared. So
# this section proves item 6 and re-proves item 4 with the same lever.
gui resize {900 600}
gui drain now
gui door reset
gui doit {WinShell resetDoorCounters.}
puts "WG3 btn-id [gui eval {(WinShell controlNamed: #button) id}]"
puts "WG3 btn-alive [gui eval {(WinShell controlNamed: #button) isAlive}]"
puts "WG3 btn-clicks-before [gui eval {WinShell clickCount}]"
gui doit {(WinShell controlNamed: #button) clearFlag.}
gui track on
gui send "0 0x111 100 0"
gui drain now
puts "WG3 btn-flag-in-door [gui eval {(WinShell controlNamed: #button) flagged}]"
puts "WG3 btn-queued-in-door [gui eval {WinShell pendingCommandCount}]"
puts "WG3 btn-clicks-in-door [gui eval {WinShell clickCount}]"
puts "WG3 btn-passes-in-door [gui eval {WinShell drainPasses}]"
gui track off
gui drain now
puts "WG3 btn-clicks-after-drain [gui eval {WinShell clickCount}]"
puts "WG3 btn-flag-after-drain [gui eval {(WinShell controlNamed: #button) flagged}]"
puts "WG3 btn-queued-after-drain [gui eval {WinShell pendingCommandCount}]"
puts "WG3 btn-status-text [gui eval {(WinShell controlNamed: #status) text}]"
puts "WG3 btn-command [gui eval {WinShell lastCommand}]"

# A click STORM: 200 synthesised WM_COMMANDs. None may be lost silently and
# none double-counted — a command is an EVENT, so it queues and every one is
# serviced, unlike a resize which is a STATE and coalesces to the last.
gui door reset
gui doit {WinShell resetDoorCounters.}
puts "WG3 storm-clicks-before [gui eval {WinShell clickCount}]"
set i 0
while {$i < 200} {
    gui send "0 0x111 100 0"
    incr i
}
gui drain now
gui drain now
puts "WG3 storm-clicks [gui eval {WinShell clickCount}]"
puts "WG3 storm-work [gui eval {WinShell drainWorkCount}]"
puts "WG3 storm-pending [gui eval {WinShell pendingCommandCount}]"
puts "WG3 storm-passes [gui eval {WinShell drainPasses}]"
puts "WG3 storm-drain [gui drain]"

# ── Part 2, item 7 + 10: LAYOUT ────────────────────────────────────────────
# After a resize and a drain, every control's real rect (GetWindowRect, in
# client coordinates) equals the rect WinLayout computed. Two independent
# productions of the same numbers.
gui resize {1000 700}
gui drain now
puts "WG3 layout-matches [gui eval {WinShell layoutMatches}]"
puts "WG3 layout-want [gui eval {WinShell expectedRects}]"
puts "WG3 layout-got [gui eval {WinShell actualRects}]"
puts "WG3 layout-dpi [gui eval {WinShell dpi}]"
# DPI: the same layout at this machine's DPI and at 1.5x it, exactly
# proportional. WG1 established the DPI contract as an EQUALITY and this keeps
# it one — the arithmetic is done in DIP space and converted once.
puts "WG3 dpi-1x [gui eval {(WinLayout shell rectsIn: 900 by: 600 dpi: 96) at: #list}]"
puts "WG3 dpi-15x [gui eval {(WinLayout shell rectsIn: 1350 by: 900 dpi: 144) at: #list}]"

# ── Part 2, item 9: WM_NOTIFY, its NMHDR read IN THE DOOR ──────────────────
# The list view's selection is changed by a message the HOST sends, pointing at
# an LVITEMW the GUEST built — because the struct can only be built by the side
# that owns the arena, and the message can only be sent by the side that is
# outside every VM entry. LVN_ITEMCHANGED comes back as WM_NOTIFY, whose lParam
# is an NMHDR valid ONLY during the call: the door reads hwndFrom/idFrom/code
# into three Integers there, and they surface in the next drain.
gui door reset
gui doit {WinShell resetDoorCounters.}
puts "WG3 list-id [gui eval {(WinShell controlNamed: #list) id}]"
puts "WG3 list-rows [gui eval {(WinShell controlNamed: #list) send: (WinApi constant: 'LVM_GETITEMCOUNT') wParam: 0 lParam: 0}]"
gui send "102 0x102B 1 [gui eval {WinShell selectRequestFor: 1}]"
gui drain now
puts "WG3 notify-count [gui eval {WinShell notifyCount}]"
puts "WG3 notify-last [gui eval {WinShell lastMeaningfulNotify}]"
puts "WG3 notify-listcode [gui eval {(WinShell controlNamed: #list) lastCode}]"
puts "WG3 notify-wantcode [gui eval {WinApi constant: 'LVN_ITEMCHANGED'}]"
puts "WG3 notify-listhandle [gui eval {(WinShell controlNamed: #list) handle}]"

# ── Part 2, item 8: THEMED, BY PIXEL ───────────────────────────────────────
# A manifest that failed to embed is otherwise invisible: comctl32 v5 registers
# the same classes, creates the same controls and passes every functional test
# above while drawing them in their Windows-95 skin. So the gate reads a pixel
# INSIDE the button out of the PNG and asserts it is not COLOR_BTNFACE, which is
# exactly what an unthemed push button's face is.
gui resize {900 600}
gui drain now
gui doit {WinShell setTitle: 'WG3 — controls, laid out by a Smalltalk drain'.}
gui sleep 300
puts "WG3 title [gui eval {WinShell title}]"
puts "WG3 winrect [gui eval {(WinShell windowRect at: 3) - (WinShell windowRect at: 1)}] [gui eval {(WinShell windowRect at: 4) - (WinShell windowRect at: 2)}]"
puts "WG3 bg [gui eval {WinShell backgroundColorRef}]"
puts "WG3 btnprobe [gui eval {(WinShell buttonProbePointInWindow) at: 1}] [gui eval {(WinShell buttonProbePointInWindow) at: 2}]"
puts "WG3 btncentre [gui eval {(WinShell buttonCentreInWindow) at: 1}] [gui eval {(WinShell buttonCentreInWindow) at: 2}]"
puts "WG3 unthemed [gui eval {WinShell unthemedButtonFace}]"
puts "WG3 snap [gui snap C:/projects/WINARM/target/winui-wg3.png]"
puts "WG3 controls [gui eval {WinShell controlReport}]"
puts "WG3 finaldoor [gui door]"
puts "WG3 finaldrain [gui drain]"
puts "WG3 done"

# ── and it still closes the way WG2 taught it to ───────────────────────────
# WM_CLOSE reaches `onClose`, which answers #defwindowproc so the DestroyWindow
# that follows happens on the FAR side of the depth guard; the WM_DESTROY it
# provokes is a fresh top-level entry and `onDestroy` posts the quit. WG3 added
# a heartbeat timer and three child windows to that sequence and must not have
# broken it. The reply may not arrive (the app is on its way down), so this is
# deliberately the last thing the script does.
puts "WG3 close [gui doit {WinShell close.}]"
