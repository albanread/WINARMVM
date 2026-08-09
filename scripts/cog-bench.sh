#!/bin/sh
# cog-bench.sh — run the micro+macro benchmark suite under Pharo/Cog and
# MACVM, same workloads, same protocol (cold, then the median of 41 single-rep
# warm samples), MICROSECOND clock on BOTH sides, interleaved back-to-back for R
# rounds on the same machine. The standing target: at least as fast as Cog.
#
# WHY microsecond: Pharo's millisecond clock and MACVM's `.as_millis()` both
# truncate, which on the sub-5 ms benches (sieve, deltablue) hid the real
# gaps and manufactured phantom ones (the WINVM investigation, PERF.md
# 2026-07-22). Both sides now read `microsecondClock` (Pharo:
# Time microsecondClockValue; MACVM: prim 252, added for this).
#
# APPLE SILICON HONESTY: unlike the WINVM/Windows harness there is NO hard
# core pinning here — macOS/arm64 exposes no per-core affinity, and thread
# affinity tags are advisory (ignored on Apple Silicon). Foreground default-
# QoS work already stays on P-cores, so the residual noise is thermal drift,
# not the P/E lottery. We control it the only honest way: a quiet machine
# (this script refuses to start if 1-min load is high), interleaved A/B
# rounds so each Cog/MACVM pair sees the same thermal state, and best-of the
# rounds. Only same-round pairs are meaningful.
#
# Setup (once): install Pharo 13 headless into $COG_DIR (default ./.cog),
# so that "$COG_DIR/pharo" and "$COG_DIR/Pharo.image" exist:
#   curl -L https://get.pharo.org/64/130 | bash    # into $COG_DIR
set -eu
cd "$(dirname "$0")/.."

ROUNDS=${ROUNDS:-3}
THRESH=${MACVM_THRESHOLD:-20}
COG_DIR=${COG_DIR:-./.cog}
PHARO="$COG_DIR/pharo"
IMG="$COG_DIR/Pharo.image"

[ -x ./target/release/macvm ] || { echo "build first: cargo build --release"; exit 2; }
{ [ -x "$PHARO" ] && [ -f "$IMG" ]; } || {
    echo "no Pharo at COG_DIR=$COG_DIR (need ./pharo + Pharo.image) — see setup comment"; exit 2; }

# Quiet-machine gate: the user works on this box; a loaded machine makes the
# comparison meaningless. Refuse above 4.0 unless FORCE=1.
LOAD1=$(uptime | sed -E 's/.*load averages?: *([0-9.]+).*/\1/')
if [ "${FORCE:-0}" != "1" ] && [ "$(printf '%.0f' "$LOAD1")" -ge 4 ]; then
    echo "1-min load $LOAD1 is too high for a clean comparison; wait for it to settle (or FORCE=1)."; exit 3
fi
# Attribution guard: every scoreboard names the EXACT tree it measured —
# without this, a commit landing from a parallel session between two runs
# makes the deltas look inexplicable (the 63.3ms->19.6ms richards "mystery"
# of 2026-07-22 was exactly this: 20b37b0 landed between two same-day runs).
GITDESC="$(git rev-parse --short HEAD 2>/dev/null || echo '?')$(git diff --quiet 2>/dev/null || echo '+dirty')"

# WINARM (P3 D5): the EMULATION AXIS. On a Windows-on-ARM64 host, Pharo's
# official download channel ships an x86-64 VM, which Windows then runs under
# its x64 translation layer. An emulated Cog loses a large factor to
# translation alone, so "MACVM beat Cog" measured against one demonstrates far
# less than the number suggests — the same class of measurement error this
# whole harness exists to remove (see the header). So: ask the BINARY what it
# is, and label every figure. `scripts/pe-machine.sh` reads the PE/COFF
# machine field; on macOS there is no PE header and `file`/`lipo` answer
# instead. A comparison whose two sides disagree on this is reported as
# INDICATIVE, never head-to-head (sprint_p03_detail.md §D5.2).
HOST_ARCH=$(uname -m)
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        COG_BIN="$PHARO"
        [ -f "$COG_BIN" ] || COG_BIN="$PHARO.exe"
        COG_MACHINE=$(sh scripts/pe-machine.sh "$COG_BIN" 2>/dev/null || echo unknown)
        ;;
    *)
        # Mach-O: `file` names the architecture directly. Unchanged behaviour
        # for the macOS runs this harness was written for — the label is new,
        # what it measures is not.
        COG_MACHINE=$(file -b "$PHARO" 2>/dev/null | grep -o 'arm64\|x86_64' | head -1)
        [ -n "$COG_MACHINE" ] || COG_MACHINE=unknown
        [ "$COG_MACHINE" = x86_64 ] && COG_MACHINE=x64
        ;;
esac
case "$HOST_ARCH:$COG_MACHINE" in
    aarch64:arm64|arm64:arm64) COG_LABEL="cog=native-arm64" ;;
    aarch64:x64|arm64:x64|aarch64:x86|arm64:x86)
        COG_LABEL="cog=emulated-$COG_MACHINE" ;;
    *:unknown) COG_LABEL="cog=UNKNOWN-ARCH" ;;
    *) COG_LABEL="cog=native-$COG_MACHINE" ;;
esac
echo "load=$LOAD1  rounds=$ROUNDS  macvm-threshold=$THRESH  commit=$GITDESC  $COG_LABEL  host=$HOST_ARCH  (microsecond clock, no hard pinning — Apple Silicon)"
if [ "$COG_LABEL" = "cog=UNKNOWN-ARCH" ]; then
    echo "REFUSING: could not determine the Cog binary's architecture, and an" >&2
    echo "unlabelled figure is exactly what D5 forbids. Set COG_DIR to a real" >&2
    echo "VM directory, or fix scripts/pe-machine.sh for this binary format." >&2
    exit 4
fi

# Richards + DeltaBlue are translated from world/41a on the fly so the .mst
# stays the single source of truth; the emitted fileIn carries the same
# checksums the MACVM driver asserts.
python3 scripts/mst2st.py /tmp/cog-all.st --assemble >/dev/null

RAW=/tmp/cogbench_raw.txt
: > "$RAW"
i=1
while [ "$i" -le "$ROUNDS" ]; do
    # COG then MACVM, back to back — a same-thermal-state pair.
    ( cd "$COG_DIR" && ./pharo Pharo.image st /tmp/cog-all.st </dev/null 2>/dev/null ) \
        | grep 'warm_us=' | sed "s/^/cog /" >> "$RAW"
    MACVM_JIT=threshold="$THRESH" ./target/release/macvm run scripts/cog-bench.mst --world world </dev/null 2>/dev/null \
        | grep 'warm_us=' | sed "s/^/macvm /" >> "$RAW"
    echo "  round $i done"
    i=$((i + 1))
done

# Reduce: best (min) warm_us per (vm,bench) across rounds, ms with one
# decimal, ratio and verdict. Best-of is the right summary — it strips the
# rounds that lost the core to something else.
COG_LABEL="$COG_LABEL" HOST_ARCH="$HOST_ARCH" python3 - "$RAW" <<'PY'
import sys, re, os, collections
best = collections.defaultdict(lambda: float('inf'))
order = []
# WINARM (P3 D5): the label travels into the table, not just the header, so a
# pasted scoreboard can never lose it.
COG_LABEL = os.environ.get('COG_LABEL', 'cog=UNKNOWN-ARCH')
HOST_ARCH = os.environ.get('HOST_ARCH', '?')
INDICATIVE = 'emulated' in COG_LABEL
for line in open(sys.argv[1]):
    m = re.match(r'(\w+)\s+(\S+)\s+.*warm_us=(\d+)', line)
    if not m: continue
    vm, bench, us = m.group(1), m.group(2), int(m.group(3))
    if bench not in order: order.append(bench)
    best[(vm, bench)] = min(best[(vm, bench)], us)
print(f"\n{'bench':10} {'MACVM ms':>9} {'Cog ms':>8} {'ratio':>7}  verdict")
print("-" * 48)
for b in order:
    mv, cg = best[('macvm', b)], best[('cog', b)]
    if mv == float('inf') or cg == float('inf'):
        print(f"{b:10} {'—':>9} {'—':>8}   (missing)"); continue
    r = mv / cg
    verdict = (f"MACVM {cg/mv:.2f}x faster" if r < 0.97 else
               f"Cog {r:.2f}x faster"       if r > 1.03 else "parity")
    print(f"{b:10} {mv/1000:>9.3f} {cg/1000:>8.3f} {r:>7.2f}  {verdict}")
print("\n(best-of-rounds, warm = median of 41 single-rep samples after 30 warm-up")
print(" reps, microsecond clock)")
print(f"({COG_LABEL}, host={HOST_ARCH})")
if INDICATIVE:
    print("\n*** INDICATIVE, NOT A HEAD-TO-HEAD (sprint_p03_detail.md D5.2). ***")
    print("The two sides do not share an execution mode: MACVM is native ARM64,")
    print("Cog is x86-64 under Windows' translation layer, which costs it a large")
    print("factor before any VM design difference is measured. Quote these numbers")
    print("only WITH this label. The claim that needs no caveat is the same-commit,")
    print("same-ISA MACVM-on-Windows-ARM64 vs MACVM-on-macOS-ARM64 differential.")
PY
