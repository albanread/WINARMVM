# MACVM CI contract. See docs/sprints/CONVENTIONS.md and
# docs/sprints/sprint_s00_detail.md — this file IS the CI contract until a
# hosted CI is set up.

# WINARM (P0): every recipe below is written in POSIX shell — `/tmp/` paths,
# `diff`, `sed`, and `VAR=value cmd` env prefixes. `just`'s default shell on
# Windows is `cmd`, which understands none of that, so the gates would fail
# for reasons that have nothing to do with the VM. This setting affects
# WINDOWS ONLY (`windows-shell`, not `shell`), so the macOS side keeps its
# default `sh` and every existing recipe stays byte-identical there — the
# same gate-don't-fork discipline MIGRATION.md §2 applies to source files.
# Requires Git for Windows' bash on PATH, which the toolchain already needs.
set windows-shell := ['bash', '-uc']

test:
    cargo test

test-release:
    cargo test --release

lint:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings

ci: lint test

# --- Phase P: the Windows-ARM64 port (MIGRATION.md, docs/sprints/*p0*) ------
#
# The P gates deliberately do NOT chain the S gates. An S gate is a claim
# about a FEATURE landing ("the scavenger works"); a P gate is a claim about
# a PLATFORM ("all of it still works over there"), so P0 re-runs the whole
# suite rather than inheriting a ladder. They also cannot chain in the other
# direction: P0's whole point is that the JIT is off, while gate-s10 and up
# require it on.
#
# P0 (docs/sprints/tests_p00.md): interpreter-only Windows ARM64 is green.
# `ci` covers build + clippy + `cargo test`; the world run is what proves a
# real Smalltalk world boots and its in-language suite passes -- the number
# that gets compared against the Mac build of the same commit (gate item 3).
gate-p00: ci
    just run-world-tests

# P2 (docs/sprints/tests_p02.md): the trap layer is alive — a compiled
# `brk #0xDExx` round-trips through the VEH into the deopt trampolines, and a
# guest-fatal / foreign fault recovers through the hand-written non-unwinding
# AArch64 setjmp/longjmp instead of killing the process. `ci`'s `cargo test`
# already carries the whole gate: the isolated jump + VEH probes, the AV
# recovery, the concurrency stress, and — the part that matters most — the 52
# tests P0 marked `#[cfg_attr(windows, ignore = "P2: …")]`, now un-gated and
# running. The second recipe is the end-to-end integration check
# MIGRATION.md §3.2 specifies: the same script must print and exit 0 with the
# JIT off AND with it on (before P2 the JIT-on run died at once with
# 0xC000001D).
#
# P1 (docs/sprints/tests_p01.md): the JIT substrate on target. Most of this
# gate is the S9 codecache suite, which `ci` already runs; what the recipe
# adds is running it in isolation, so a failure here reads as "the loader,
# the region, relocation or the W^X/icache path broke" rather than being one
# red line among a thousand. The vendored encoder's frozen corpus is included
# for the same reason.
#
# Note what this gate does NOT claim. `patch_flip_churn_stays_coherent` runs
# a thousand patch-then-execute cycles, and it passes with BOTH halves of
# `icache_invalidate` commented out (measured — sprint_p01_detail.md's Δ). It
# is a regression test for the patch path, not evidence the flush works; on
# this Snapdragon/Oryon host a missing flush is simply not observable.
gate-p01: gate-p00
    cargo test --test it_codecache
    cargo test --lib corpus_replay

# One honest note about the chain rather than a silent divergence:
#   * `ci` runs `lint`, and `cargo fmt --check`/`clippy -D warnings` still
#     fail under rustc 1.97.1 on files this port never touched
#     (MIGRATION.md §8, P0's "Open, and not a port problem"). That is
#     unchanged by P2 and is why the sprint verifies with `cargo test
#     --no-fail-fast` directly as well.
# threshold=20, not =2: rule 1 — the threshold is never below the floor (20),
# and `parse_jit` now clamps sub-floor values. The script warms past the
# floor itself, so the trap still fires compiled.
gate-p02: gate-p01
    MACVM_JIT=off cargo run --release -- run scripts/p2-deopt-roundtrip.mst --world world
    MACVM_JIT=threshold=20 cargo run --release -- run scripts/p2-deopt-roundtrip.mst --world world

# P3 (docs/sprints/tests_p03.md) — the JIT-vs-interpreter DIFFERENTIAL, gate
# item 4. Every corpus the project keeps, run in three JIT modes, stdout and
# exit status compared byte-for-byte: the world's whole in-language suite,
# the golden `.mst` transcripts, and the tracked JIT-bug repro corpus. The
# expectation is ZERO differences — unlike WINVM's x64 port, which lived with
# seven, this backend has no closure/NLR/OSR gaps, so a difference here is a
# port regression and blocks (tests_p03.md gate item 4).
#
# Three modes, not two: `off` is the oracle, `threshold=20` compiles nearly
# everything with warm ICs, `threshold=1000` compiles only the genuinely hot
# methods and therefore exercises a DIFFERENT set of compiled/interpreted
# boundaries (a bug that only shows at one tier mix would otherwise hide).
# Note what `threshold=20` is: `MACVM_JIT=threshold=1` is REFUSED by
# `VmOptions::parse_jit` and silently becomes `JIT_THRESHOLD_FLOOR` = 20, so
# the aggressive setting every gate in this file has ever run is 20. Spelling
# it here rather than repeating the `=1` that does not mean what it says.
diff-p03:
    #!/usr/bin/env bash
    # NOT `set -e`: several corpus files exit NONZERO by design (the
    # `closure_dead_home_cannot_return` repro exits 1 — that IS its expected
    # behaviour, and the test is that all three JIT modes agree on it), so
    # errexit would abort the sweep on a correct result. Failure is tracked
    # explicitly in `fail` and reported at the end.
    set -uo pipefail
    grep -v '^#' world/tests/tests.list | grep -v '^$' | sed 's|^|world/tests/|' | xargs cat > /tmp/macvm_world_tests.mst
    BIN=./target/release/macvm
    [ -x "$BIN" ] || { echo "diff-p03: build first (cargo build --release)" >&2; exit 2; }
    fail=0
    for f in /tmp/macvm_world_tests.mst tests/golden/*.mst tests/repros/*.mst; do
        MACVM_JIT=off           "$BIN" run "$f" --world world >/tmp/p3d_off.txt  2>/dev/null; a=$?
        MACVM_JIT=threshold=20  "$BIN" run "$f" --world world >/tmp/p3d_t20.txt  2>/dev/null; b=$?
        MACVM_JIT=threshold=1000 "$BIN" run "$f" --world world >/tmp/p3d_t1k.txt 2>/dev/null; c=$?
        if diff -q /tmp/p3d_off.txt /tmp/p3d_t20.txt >/dev/null \
           && diff -q /tmp/p3d_off.txt /tmp/p3d_t1k.txt >/dev/null \
           && [ "$a" = "$b" ] && [ "$a" = "$c" ]; then
            printf '  same  %-52s exit=%s\n' "$(basename "$f")" "$a"
        else
            printf '  DIFF  %-52s exits=%s/%s/%s\n' "$(basename "$f")" "$a" "$b" "$c"
            diff /tmp/p3d_off.txt /tmp/p3d_t20.txt | head -20 || true
            fail=1
        fi
    done
    [ "$fail" = 0 ] || { echo "FAIL: JIT-vs-interpreter differences (tests_p03.md gate item 4)" >&2; exit 1; }

# P3 (docs/sprints/tests_p03.md) — tier-1 alive under every stress mode the
# project has. Gate items 1-4 and 7.
#
# `--skip mandelvm` on every suite run, and it is NOT a weakening: the skipped
# test renders 140 Mandelbrot frames and costs ~45 MINUTES in a debug build
# (18 s in release — MIGRATION.md §8 records the whole disproof of the
# "exception storm" it looks like). Running it four times over would make this
# gate a three-hour job that nobody runs. It is covered by `just test-release`
# and by `gate-p00`'s own `cargo test`.
#
# The compile-count tripwire (tests_p03.md's "a green suite that never
# compiled anything is a false green") is not a shell grep here: it lives in
# `it_world::compile_count_nonzero_at_threshold1`, which asserts
# `vm.stats.compilations > 0` after loading the whole world + test corpus, so
# it runs in EVERY line below rather than only in the one that remembered to
# check.
#
# WHY THE GC-STRESS LINES ARE `--release` AND THE OTHERS ARE NOT. Measured,
# not assumed (P3): `verify::verify_enabled()` is
# `cfg!(debug_assertions) || MACVM_GC_VERIFY=1`, so a DEBUG build runs the
# full cross-check heap verifier at every GC phase boundary — and
# `MACVM_GC_STRESS=1` means a scavenge before EVERY allocation, so the pair
# is a whole-heap walk per allocation. Numbers from this host: booting the
# world and computing fib(15) takes 0.09 s (debug, no stress), 0.95 s (debug,
# threshold=20, no stress), 0.30 s (release, stress AND JIT) — and does NOT
# COMPLETE IN FOUR MINUTES in debug with `MACVM_GC_STRESS=1`, with the JIT
# off, i.e. the JIT is not the factor and neither is the platform
# (`memory/verify.rs` and `memory/scavenge.rs` are byte-identical to MACVM).
# `gate-s08`'s own comment already recorded the shape of this ("30+ seconds
# ... 0.6s under --release") and `soak-s08` already runs release for it.
# Running the stressed suites in release is therefore this project's existing
# discipline, not a weakening invented here — and the assertions that release
# does drop (`debug_assert!`, the heap verifier) are all exercised by the
# unstressed debug line above and by `gate-p00`'s own `cargo test`.
# `MACVM_GC_VERIFY=1` opts a release run back into the verifier if you ever
# want the pair; budget hours, not minutes.
gate-p03: gate-p02
    # item 7: no P1/P2 gating marks left anywhere (comment-blind literal grep;
    # the P5 marks and the two VM-defect marks are named exclusions).
    ! grep -rn 'ignore = "P1\|ignore = "P2' src/ tests/
    # item 1: everything eligible compiles. DEBUG — this is the line that
    # carries `debug_assert!`, including P3 D1's frame-size invariant.
    MACVM_JIT=threshold=1 cargo test --no-fail-fast -- --skip mandelvm
    # item 3a: deopt stress alone. Debug too — it costs nothing extra.
    MACVM_DEOPT_STRESS=1 cargo test --no-fail-fast -- --skip mandelvm
    # item 2: the S12 flagship — moving GC under compiled frames.
    MACVM_JIT=threshold=1 MACVM_GC_STRESS=1 cargo test --release --no-fail-fast -- --skip mandelvm
    # item 3b: all three modes at once (the S14 bar).
    MACVM_JIT=threshold=1 MACVM_GC_STRESS=1 MACVM_DEOPT_STRESS=1 cargo test --release --no-fail-fast -- --skip mandelvm
    # item 2 again, from the other side: the combined-stress world run must
    # show real collections with live compiled frames on the native stack.
    just bridge-stats-s11
    # item 4: zero JIT-vs-interpreter differences over every corpus.
    cargo build --release
    just diff-p03

# P4 (tests_p04.md) — the GUI shell seam. HEADLESS PARTS ONLY: items 4-9 of
# that test plan are an on-screen checklist a person has to run and observe,
# and this recipe deliberately does not pretend to cover them.
#
# The seam IS the deliverable, so item 2's grep is the load-bearing check
# here — not a lint. `main.rs` owning even one platform call means the next
# platform's shell has to re-derive the boundary instead of implementing it.
gate-p04: gate-p03
    # item 2a: main.rs contains ZERO platform calls. Comment-blind (the same
    # discipline gate-p03's marker grep uses): the file legitimately *names*
    # these APIs in comments explaining which shell owns what, so strip `//`
    # to the end of line before matching, and fail if any CODE hit survives.
    #
    # `! grep -q` — grep exits 1 when it finds nothing, which is success here.
    ! sed -e 's://.*::' gui/src/main.rs | grep -qE 'objc_msgSend|objc::|windows::|webview2|\bsel\('
    # item 2b: and nothing outside gui/src/shell/ reaches for a platform crate
    # either. `gui/src/objc.rs` is the named exclusion — it IS the macOS
    # bridge the mac shell is built on. (`game_pane.rs` is also AppKit code,
    # but it is a macOS-only, off-by-default feature module that talks to that
    # bridge rather than to a platform crate, so these patterns correctly do
    # not fire on it. The point of this gate is the PORTABLE files.)
    ! find gui/src -name '*.rs' -not -path 'gui/src/shell/*' -not -name 'objc.rs' \
        -exec sed -e 's://.*::' {} + | grep -qE 'objc_msgSend|windows::|webview2_com'
    # item 1: the Windows shell compiles and the gui unit suite is green.
    # Includes item 2's `preprocess` URL-form tests, item 3's waker test, the
    # shim-shape test and the eval_js fire-and-forget shape test.
    cargo build -p macvm-gui
    cargo test -p macvm-gui
    # The core must not move. P3 finished at 1065 passed / 0 failed / 15
    # ignored and this sprint touches image_store (the backfill batching), so
    # re-prove it rather than assume it.
    cargo test --no-fail-fast -- --skip mandelvm
    cargo test -p image_store

# P5 (tests_p05.md) — FFI on Windows ARM64: winkb + the MS-ARM64 classifier.
# Chains gate-p03 (P4 is independent — deliberately NOT chained, per the
# sprint doc's implementation order). The one thing this recipe cannot do
# for you: gate item 5's DB-ABSENT run renames a ~90 MB machine-local
# artifact aside and back, so it is written defensively — if the DB is
# already absent, the whole gate IS the absent-state run and the rename
# pair no-ops.
gate-p05: gate-p03
    #!/usr/bin/env bash
    set -euo pipefail
    # item 7 discipline: no stale P5 gating marks left anywhere (the
    # IoWorker/winsock marks deliberately name their FOLLOW-ON slice, not P5).
    ! grep -rn 'ignore = "P5' src/ tests/ gui/src/
    # items 1-3 + 6: the full suite, which now contains the pinning tests
    # (embed::tests::win32_*), the classifier suite (runtime::winkb), the
    # un-gated ffi/alien/world tests and the wall-clock world branch. Debug
    # (debug_assert! lives here); env threshold=1 floors to 20 (P3's Δ).
    MACVM_JIT=threshold=1 cargo test --no-fail-fast -- --skip mandelvm
    # item 5: the SAME suite with the DB ABSENT — the fallback contract is
    # load-bearing (absence is not an error; the probe resolver carries the
    # world alone). If the DB is already absent, the run above WAS the
    # absent-state run and this block no-ops. Restore even on failure.
    DB="${WINKB_DB:-C:/projects/windows_api/windows_api.db}"
    if [ -f "$DB" ]; then
        mv "$DB" "$DB.p5-aside"
        MACVM_JIT=threshold=1 cargo test --no-fail-fast -- --skip mandelvm \
            || { mv "$DB.p5-aside" "$DB"; exit 1; }
        mv "$DB.p5-aside" "$DB"
    fi
    # item 4's Rust-side half: the gui acceptance test P4 flagged for this
    # sprint (FFI through a DB-booted VM, the live-compile path).
    cargo test -p macvm-gui --bin macvm-gui -- ffi_works_through_a_db_booted_vm

# --- Phase WG: the Windows-native Smalltalk UI (docs/win_gui_design.md) -----

# WG0 (docs/sprints/tests_wg0.md): guest Smalltalk drives user32/kernel32
# through the P5 resolver — GetSystemMetrics, MessageBeep, GetModuleHandleW,
# then a WNDCLASSW built at winkb-QUERIED offsets, RegisterClassW,
# CreateWindowExW (hidden) and the IsWindow/DestroyWindow/IsWindow round
# trip. Everything below is world-side and driven through the existing
# `macvm` CLI, which is the gate's own wording: WG0 adds no binary and no
# UI code (the four winkb DATA primitives it did need are the sprint's
# recorded Δ, not UI).
#
# Chains gate-p05 because WG0 rides P5's resolver and nothing else; the two
# DB states are run explicitly here for the same reason P5 ran them — the
# database is a machine-local artifact and the fallback IS the story.
gate-wg0: gate-p05
    #!/usr/bin/env bash
    set -euo pipefail
    # The in-language suite through the CLI, both DB states. `winui.list`'s
    # layer is loaded by tests.list (see its comment for the compile-time
    # reason a layered world's tests cannot self-guard); the probe's own
    # tests are platformName-guarded in 99_run_all.mst.
    grep -v '^#' world/tests/tests.list | grep -v '^$' | sed 's|^|world/tests/|' \
        | xargs cat > /tmp/macvm_wg0_tests.mst
    cargo build --quiet
    ./target/debug/macvm run /tmp/macvm_wg0_tests.mst --world world | tee /tmp/wg0_present.txt
    grep -q ', 0 failed' /tmp/wg0_present.txt
    WINKB_DB=/nonexistent/windows_api.db ./target/debug/macvm run /tmp/macvm_wg0_tests.mst \
        --world world | tee /tmp/wg0_absent.txt
    grep -q ', 0 failed' /tmp/wg0_absent.txt
    # gate item 5: the DB-absent skip is ANNOUNCED, never silent.
    grep -q 'SKIP WinUiProbeTests' /tmp/wg0_absent.txt
    # gate items 1-2: the ladder itself, rung by rung, from a doit.
    #
    # Read from winui.list rather than naming 90_winui_probe.mst, and that is
    # not tidiness: WG1 promoted the arena and the FFI façade out of WinProbe
    # into 91_winui_shell.mst, so the probe file ALONE compiles (90
    # forward-declares WinArena/WinApi with empty bodies) and then fails at
    # run time with `does not understand winkbAvailable`. The layer is two
    # files now and this gate loads the layer, in the list's own order, so a
    # third file cannot break it again.
    grep -v '^#' world/winui.list | grep -v '^$' | sed 's|^|world/|' \
        | xargs cat > /tmp/macvm_wg0_probe.mst
    echo 'WinProbe report.' >> /tmp/macvm_wg0_probe.mst
    ./target/debug/macvm run /tmp/macvm_wg0_probe.mst --world world | tee /tmp/wg0_ladder.txt
    grep -q 'IsWindow = false' /tmp/wg0_ladder.txt
    # gate item 4: world.list is untouched — the base world stays
    # byte-identical, which is what "winui.list is additive" means.
    git diff --quiet -- world/world.list

# WG1 (docs/sprints/tests_wg1.md): a VISIBLE, Windows-11-styled window,
# created by Smalltalk, owned by a VM on the process's real main thread,
# pumped by Rust, closed cleanly — and every item of that proven by a
# SCRIPT, because §3.1's capture channel exists precisely so "is there a
# window and does it look right" stops being a human question.
#
# Nothing below asks anyone to look at anything. The window's dimensions
# come back through the FFI in the same session that captures it, and the
# PNG's own IHDR is compared against them: a file that merely exists is not
# evidence, its size is.
gate-wg1: gate-wg0
    #!/usr/bin/env bash
    set -euo pipefail
    PORT=7649
    SHOT=target/winui-wg1.png
    cargo build --quiet
    cargo build --quiet -p win_gui

    # ── the in-language half, both DB states ──────────────────────────────
    # Same concatenating shape gate-wg0 uses; the layer is loaded by
    # tests.list (see its comment for the compile-time reason a layered
    # world's tests cannot self-guard).
    grep -v '^#' world/tests/tests.list | grep -v '^$' | sed 's|^|world/tests/|' \
        | xargs cat > /tmp/macvm_wg1_tests.mst
    ./target/debug/macvm run /tmp/macvm_wg1_tests.mst --world world | tee /tmp/wg1_present.txt
    grep -q ', 0 failed' /tmp/wg1_present.txt
    WINKB_DB=/nonexistent/windows_api.db ./target/debug/macvm run /tmp/macvm_wg1_tests.mst \
        --world world | tee /tmp/wg1_absent.txt
    grep -q ', 0 failed' /tmp/wg1_absent.txt
    # The DB-absent skip is ANNOUNCED, never silent — for BOTH WG classes.
    grep -q 'SKIP WinUiProbeTests' /tmp/wg1_absent.txt
    grep -q 'SKIP WinUiShellTests' /tmp/wg1_absent.txt

    # ── the crate's own tests + the headless stress items ─────────────────
    # tests_wg1.md's stress rows that need a PROCESS rather than a slot in
    # it_world (WG0's Δ 11: several VMs share one process there, in parallel
    # threads, and a visible window with a message loop does not belong in a
    # parallel harness). Open/close x10, snap-before-window, GetMessageW=-1
    # forced for real, and a recovered guest fault leaving the VM usable.
    cargo test --quiet -p win_gui
    MACVM_WINUI_CONTROLS=off timeout 300 ./target/debug/macvm-winui.exe --selftest \
        | tee /tmp/wg1_selftest.txt
    grep -q 'SELFTEST OK' /tmp/wg1_selftest.txt
    grep -q 'snap-before-window: ERR no window yet' /tmp/wg1_selftest.txt
    grep -q "getmessage-minus-one: rc=-1 classify=Failed" /tmp/wg1_selftest.txt

    # ── the window itself (gate items 1-7) ────────────────────────────────
    rm -f "$SHOT" target/winui-wg1-titled.png /tmp/wg1_exit /tmp/wg1_app.txt
    # WG3 puts three child controls in this window, and both of them break a
    # WG1 assertion: a themed list view paints WHITE pixels exactly where this
    # gate reads the window's background fill, and it sends NM_CUSTOMDRAW
    # through a door WG2's gate counts messages at. `tests_wg3.md` item 1 asks
    # for these gates to pass "with the drain installed and NO CONTROL
    # CREATED"; `MACVM_WINUI_CONTROLS=off` is that phrase, as a token, and it
    # keeps WG1's and WG2's gates testing the configuration they were written
    # against rather than being loosened to accommodate a later sprint.
    ( MACVM_WINUI_CTL=$PORT MACVM_WINUI_CONTROLS=off ./target/debug/macvm-winui.exe > /tmp/wg1_app.txt 2>&1; \
      echo $? > /tmp/wg1_exit ) &
    for i in $(seq 1 60); do
        (exec 3<>/dev/tcp/127.0.0.1/$PORT) 2>/dev/null && break || sleep 0.5
    done
    ./target/debug/macvm rusttcl scripts/winui-gate.tcl | tee /tmp/wg1_gate.txt
    cat /tmp/wg1_app.txt

    # 1. a visible window exists and Win32 agrees.
    grep -q 'WG1 ping pong' /tmp/wg1_gate.txt
    grep -q 'WG1 iswindow true' /tmp/wg1_gate.txt
    # 2. the same-thread invariant, asked of Win32 from BOTH ends.
    grep -q 'WG1 threadinvariant true' /tmp/wg1_gate.txt
    grep -q 'thread invariant HOLDS' /tmp/wg1_app.txt
    # 5. the doit changed the REAL titlebar, read back with GetWindowTextW.
    grep -q "WG1 title-after 'WG1-OK'" /tmp/wg1_gate.txt
    # 7. DPI: the client rect IS the DIP size scaled by GetDpiForWindow/96.
    CLIENT=$(grep '^WG1 client ' /tmp/wg1_gate.txt | cut -d' ' -f3,4)
    EXPECT=$(grep '^WG1 expected ' /tmp/wg1_gate.txt | cut -d' ' -f3,4)
    echo "client=$CLIENT expected=$EXPECT"
    test "$CLIENT" = "$EXPECT"
    # 4. the PNG's own dimensions equal the WINDOW rect. IHDR width and
    #    height are the two big-endian u32s at byte offset 16.
    #
    # Δ (WG2, measured): these two assertions used to read the CLIENT rect and
    # byte 49, and both are stale — `gui/src/shell/snap.rs` was corrected after
    # WG1 closed to size its bitmap by `GetWindowRect`, and its own doc says
    # why in as many words: the client-sized version "measured correct — client
    # 900x600, PNG 900x600, the gate's equality satisfied — while actually
    # containing the titlebar plus only the top 560 px of the client area".
    # The capture was fixed; the gate that depended on the defect was not, so
    # it has been comparing 916x639 against 900x600 ever since. It now compares
    # against the window rect, which is what was captured.
    test -s "$SHOT"
    PNGDIM=$(od -An -tu1 -j16 -N8 "$SHOT" \
        | awk '{printf "%d %d", ($1*16777216+$2*65536+$3*256+$4), ($5*16777216+$6*65536+$7*256+$8)}')
    WINRECT=$(grep '^WG1 window ' /tmp/wg1_gate.txt | cut -d' ' -f3,4)
    echo "png=$PNGDIM window=$WINRECT client=$CLIENT"
    test "$PNGDIM" = "$WINRECT"
    # ...and its PIXELS are the fill Smalltalk chose from the SYSTEM's theme.
    # Byte 49 is the top-left of the WINDOW, i.e. DWM's frame (245,241,247 on
    # this machine), so the check reads the CENTRE pixel instead — client area
    # for any window with a titlebar. The offset needs the block arithmetic the
    # old fixed 49 could skip: the writer emits stored deflate blocks capped at
    # 65535 bytes, so a 5-byte header appears every 65535 raw bytes and each
    # scanline is 1 filter byte + width*4. Reading it turns "the shot exists
    # and is the right size" into "the shot shows the window this machine's
    # AppsUseLightTheme asked for".
    BG=$(grep '^WG1 bg ' /tmp/wg1_gate.txt | cut -d' ' -f3)
    W=$(echo $PNGDIM | cut -d' ' -f1); H=$(echo $PNGDIM | cut -d' ' -f2)
    OFF=$(awk -v w=$W -v h=$H 'BEGIN{stride=1+w*4; raw=int(h/2)*stride+1+int(w/2)*4; \
        print 48+raw+5*int(raw/65535)}')
    WANT=$(awk -v c="$BG" 'BEGIN{printf "%d %d %d", c%256, int(c/256)%256, int(c/65536)%256}')
    GOT=$(od -An -tu1 -j$OFF -N3 "$SHOT" | awk '{printf "%d %d %d", $1, $2, $3}')
    echo "png-centre=$GOT want(COLORREF $BG)=$WANT"
    test "$GOT" = "$WANT"
    # 6. clean shutdown: the loop ENDED, the process exited 0. Not killed.
    for i in $(seq 1 60); do [ -f /tmp/wg1_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg1_exit)" = "0"
    grep -q 'message loop ended, exit 0' /tmp/wg1_app.txt

    # ── the loop outlives a VM fault (tests_wg1.md stress row 4) ──────────
    # A fault injected at the END of openMain, so the window is fully real
    # and fully shown when the VM falls over. P2 recovers, Rust's pump never
    # learns it happened, and the control channel still brings the process
    # down through exit 0 — which is D4's second reason, made a test.
    for MODE in guest native; do
        rm -f /tmp/wg1_fault_exit /tmp/wg1_fault.txt
        ( MACVM_WINUI_CTL=$PORT MACVM_WINUI_FAULT=$MODE MACVM_WINUI_CONTROLS=off \
            ./target/debug/macvm-winui.exe > /tmp/wg1_fault.txt 2>&1; \
          echo $? > /tmp/wg1_fault_exit ) &
        for i in $(seq 1 60); do
            (exec 3<>/dev/tcp/127.0.0.1/$PORT) 2>/dev/null && break || sleep 0.5
        done
        printf 'gui connect %s\nputs "FAULT ping [gui ping]"\nputs "FAULT live [gui eval {WinShell isOpen}]"\ngui quit\n' \
            "$PORT" > /tmp/wg1_fault.tcl
        ./target/debug/macvm rusttcl /tmp/wg1_fault.tcl | tee /tmp/wg1_fault_gate.txt
        cat /tmp/wg1_fault.txt
        grep -q 'FAULT ping pong' /tmp/wg1_fault_gate.txt
        grep -q 'FAULT live true' /tmp/wg1_fault_gate.txt
        grep -q 'openMain raised' /tmp/wg1_fault.txt
        for i in $(seq 1 60); do [ -f /tmp/wg1_fault_exit ] && break || sleep 0.5; done
        test "$(cat /tmp/wg1_fault_exit)" = "0"
    done

    # world.list is still untouched: the base world stays byte-identical,
    # which is what "winui.list is additive" means — WG1 added a second file
    # to the LAYER and nothing to the world.
    git diff --quiet -- world/world.list
    echo "gate-wg1: OK"

# WG2 (docs/sprints/tests_wg2.md): the WndProc DOOR. Windows messages reach
# Smalltalk; a raising handler does not break the window, a faulting one
# does not break the process, and a re-entered one degrades to
# DefWindowProcW rather than to corruption.
#
# Everything below is a script, for the reason WG1's Δ 8 records: `eval`
# answers INLINE in this host (the VM is on the pump's own thread), so every
# item reads a number rather than sleeping and hoping.
#
# Δ (WG2, measured — three corrections to gate-wg1's own assertions, which
# this recipe therefore does NOT copy):
#  * `snap` captures the WINDOW rect (gui/src/shell/snap.rs says so in its
#    own doc: "sized by GetWindowRect, not GetClientRect"). gate-wg1 compares
#    the PNG's IHDR against the CLIENT rect, which differs by the frame — 916
#    x 639 against 900x600 on this machine. This recipe compares against the
#    window rect.
#  * Byte 49 of the PNG is therefore the top-left of the WINDOW, i.e. DWM's
#    frame (245,241,247 here), not the client fill (243,243,243). This
#    recipe reads the CENTRE pixel, which is client area for any window with
#    a titlebar, and computes its offset properly — the writer emits stored
#    deflate blocks capped at 65535 bytes (gui/src/shell/snap.rs), so a
#    5-byte block header appears every 65535 raw bytes and a naive
#    `48 + rawoffset` is wrong for any scanline past the first ~17.
#  * The transparency proof is a MODE, not a rebuild: MACVM_WINUI_DOOR=off
#    registers the door and empties the allowlist, so WG1's entire gate runs
#    against the WG2 binary with every message still DefWindowProcW'd.
gate-wg2: gate-wg1
    #!/usr/bin/env bash
    set -euo pipefail
    PORT=7650
    SHOT=target/winui-wg2.png
    cargo build --quiet
    cargo build --quiet -p win_gui

    # ── the in-language half, both DB states (gate item 8) ────────────────
    grep -v '^#' world/tests/tests.list | grep -v '^$' | sed 's|^|world/tests/|' \
        | xargs cat > /tmp/macvm_wg2_tests.mst
    ./target/debug/macvm run /tmp/macvm_wg2_tests.mst --world world | tee /tmp/wg2_present.txt
    grep -q ', 0 failed' /tmp/wg2_present.txt
    WINKB_DB=/nonexistent/windows_api.db ./target/debug/macvm run /tmp/macvm_wg2_tests.mst \
        --world world | tee /tmp/wg2_absent.txt
    grep -q ', 0 failed' /tmp/wg2_absent.txt
    # All THREE WG classes announce their DB-absent skip; none is silent.
    grep -q 'SKIP WinUiProbeTests' /tmp/wg2_absent.txt
    grep -q 'SKIP WinUiShellTests' /tmp/wg2_absent.txt
    grep -q 'SKIP WinUiDoorTests' /tmp/wg2_absent.txt
    # ...and the ONE door test that needs no database still ran, because the
    # address channel is a primitive. That asymmetry is the point of D2.
    grep -q 'testDoorAddressIsPublished' /tmp/wg2_absent.txt

    # ── the crate's tests + the door's own (the fault-path guard) ─────────
    cargo test --quiet -p win_gui
    cargo test --quiet --lib win_wndproc
    MACVM_WINUI_CONTROLS=off timeout 300 ./target/debug/macvm-winui.exe --selftest \
        | tee /tmp/wg2_selftest.txt
    grep -q 'SELFTEST OK' /tmp/wg2_selftest.txt
    # The re-entrancy source the sprint spec does not name, checked where it
    # actually fires: `cycle: 10` creates and destroys ten windows FROM A
    # DOIT, so every message those calls SEND arrives with the VM already
    # inside a top-level entry. Every one must be declined, and the VM must
    # be entered exactly zero times.
    grep -qE 'SELFTEST door-after-cycle: door enabled=true depth=0 entries=0 ' \
        /tmp/wg2_selftest.txt
    grep -q 'SELFTEST door-address:' /tmp/wg2_selftest.txt

    # ── item 1: TRANSPARENCY. WG1's entire gate, unchanged, with the door
    #    REGISTERED and every message still defaulted. A door that changes
    #    behaviour before it does anything is a door with a bug in it.
    rm -f target/winui-wg1.png /tmp/wg2_t_exit /tmp/wg2_t_app.txt
    ( MACVM_WINUI_CTL=7649 MACVM_WINUI_DOOR=off MACVM_WINUI_CONTROLS=off ./target/debug/macvm-winui.exe \
        > /tmp/wg2_t_app.txt 2>&1; echo $? > /tmp/wg2_t_exit ) &
    for i in $(seq 1 60); do
        (exec 3<>/dev/tcp/127.0.0.1/7649) 2>/dev/null && break || sleep 0.5
    done
    ./target/debug/macvm rusttcl scripts/winui-gate.tcl | tee /tmp/wg2_t_gate.txt
    cat /tmp/wg2_t_app.txt
    # The door really was registered (its address is in the report) and really
    # was inert (no VM entry at all).
    grep -q 'WndProc door published at' /tmp/wg2_t_app.txt
    grep -q 'enabled=false' /tmp/wg2_t_app.txt
    grep -qE 'door enabled=false depth=0 entries=0 ' /tmp/wg2_t_app.txt
    # ...and every WG1 number is what WG1 measured.
    grep -q 'WG1 iswindow true' /tmp/wg2_t_gate.txt
    grep -q 'WG1 threadinvariant true' /tmp/wg2_t_gate.txt
    grep -q 'thread invariant HOLDS' /tmp/wg2_t_app.txt
    grep -q "WG1 title-after 'WG1-OK'" /tmp/wg2_t_gate.txt
    CLIENT=$(grep '^WG1 client ' /tmp/wg2_t_gate.txt | cut -d' ' -f3,4)
    EXPECT=$(grep '^WG1 expected ' /tmp/wg2_t_gate.txt | cut -d' ' -f3,4)
    echo "transparency client=$CLIENT expected=$EXPECT"
    test "$CLIENT" = "$EXPECT"
    test -s target/winui-wg1.png
    for i in $(seq 1 60); do [ -f /tmp/wg2_t_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg2_t_exit)" = "0"
    grep -q 'message loop ended, exit 0' /tmp/wg2_t_app.txt

    # ── the door, doing its job (items 2-7) ───────────────────────────────
    rm -f "$SHOT" /tmp/wg2_exit /tmp/wg2_app.txt
    # Controls off — see gate-wg1's note. This gate counts every message that
    # crossed the door, and a repainting list view sends WM_NOTIFY.
    ( MACVM_WINUI_CTL=$PORT MACVM_WINUI_CONTROLS=off ./target/debug/macvm-winui.exe > /tmp/wg2_app.txt 2>&1; \
      echo $? > /tmp/wg2_exit ) &
    for i in $(seq 1 60); do
        (exec 3<>/dev/tcp/127.0.0.1/$PORT) 2>/dev/null && break || sleep 0.5
    done
    ./target/debug/macvm rusttcl scripts/winui-wg2.tcl | tee /tmp/wg2_gate.txt
    cat /tmp/wg2_app.txt

    # 2. A message reaches Smalltalk, and the assertion is a CROSS-CHECK:
    #    `lastSizeSeen` is decoded from the MESSAGE's lParam, the client rect
    #    comes from GetClientRect. Two independent sources, one equality.
    grep -q 'WG2 sizematches true' /tmp/wg2_gate.txt
    grep -q 'WG2 sizecount 1' /tmp/wg2_gate.txt
    # Δ (WG3, measured): `cut -f3` here took only the FIRST field of a
    # two-field Array printString — `#(684 461)` became `684`, which can never
    # equal the `684 461` it is compared against. `f3-` is the fix. Recorded
    # rather than quietly corrected because it is the same shape of defect
    # WG2's own Δ 6 found in gate-wg1: a gate line that cannot pass is
    # indistinguishable from one that has not been run.
    LAST=$(grep '^WG2 lastsize ' /tmp/wg2_gate.txt | cut -d' ' -f3- | tr -d '#()')
    CL=$(grep '^WG2 client ' /tmp/wg2_gate.txt | cut -d' ' -f3,4)
    echo "door lastSizeSeen=$LAST client=$CL"
    test "$LAST" = "$CL"

    # 4. A RAISING handler does not break the window. 40 messages ARRIVED
    #    (the counter is bumped before any handler can fail), exactly 20
    #    COMPLETED, and the last good one still matches the real client rect
    #    — i.e. every good message after every bad one still dispatched.
    grep -q 'WG2 raise-messagesseen 40' /tmp/wg2_gate.txt
    grep -q 'WG2 raise-sizecount 20' /tmp/wg2_gate.txt
    grep -q 'WG2 raise-matches true' /tmp/wg2_gate.txt
    grep -q 'WG2 raise-alive 7' /tmp/wg2_gate.txt

    # 5. A FAULTING handler does not break the process. Five real
    #    ACCESS_VIOLATIONs inside the handler, none completing; then the next
    #    message still reaches Smalltalk. If the RAII depth guard leaked on
    #    the longjmp path, THIS is the line that would fail — every later
    #    message would be declined as nested, forever, with nothing in any
    #    log.
    grep -q 'WG2 fault-messagesseen 5' /tmp/wg2_gate.txt
    grep -q 'WG2 fault-sizecount 0' /tmp/wg2_gate.txt
    grep -q 'WG2 fault-then-sizecount 1' /tmp/wg2_gate.txt
    grep -q 'WG2 fault-then-matches true' /tmp/wg2_gate.txt
    grep -q 'WG2 fault-alive 7' /tmp/wg2_gate.txt
    grep -q 'WG2 fault-ping pong' /tmp/wg2_gate.txt
    grep -q 'ACCESS_VIOLATION' /tmp/wg2_app.txt

    # 6. Nesting degrades safely. The handler SendMessageW'd its own window;
    #    the nested message was declined and answered by DefWindowProcW
    #    (nested=1, defaulted=1), the OUTER one completed exactly once
    #    (sizeCount 1, messagesSeen 1 — the nested one never reached
    #    Smalltalk at all), and the depth counter is back to 0.
    grep -q 'WG2 nest-messagesseen 1' /tmp/wg2_gate.txt
    grep -q 'WG2 nest-sizecount 1' /tmp/wg2_gate.txt
    grep -qE 'WG2 door-after-nest door enabled=true depth=0 entries=1 defaulted=1 nested=1 ' \
        /tmp/wg2_gate.txt

    # 7 / stress. 200 scripted resizes: `lastSizeSeen` tracks the final rect,
    #    every one of them dispatched, the guard is 0 at the end.
    grep -q 'WG2 stress-sizecount 200' /tmp/wg2_gate.txt
    grep -q 'WG2 stress-matches true' /tmp/wg2_gate.txt
    grep -qE 'WG2 latency-door door enabled=true depth=0 entries=200 defaulted=0 nested=0 busy=0 ' \
        /tmp/wg2_gate.txt
    # ...and with the allowlist OFF the same 200 resizes reach Smalltalk
    # exactly zero times, which is the two-sided allowlist's other half.
    grep -q 'WG2 offdoor-sizecount 0' /tmp/wg2_gate.txt
    grep -q 'WG2 offdoor-messagesseen 0' /tmp/wg2_gate.txt

    # D5's two numbers, both present and both non-degenerate.
    DOORNS=$(grep '^WG2 latency-door ' /tmp/wg2_gate.txt | tr ' ' '\n' | grep '^doorNs=' | cut -d= -f2)
    BASENS=$(grep '^WG2 latency-base ' /tmp/wg2_gate.txt | tr ' ' '\n' | grep '^baseNs=' | cut -d= -f2)
    echo "D5: door round trip ${DOORNS}ns vs DefWindowProcW baseline ${BASENS}ns"
    test "$DOORNS" -gt 0
    test "$BASENS" -gt 0
    # The trampoline must never have caught a panic — a nonzero count is a
    # bug report, not a statistic.
    grep -q 'panics=0' /tmp/wg2_gate.txt
    ! grep -q 'panics=[1-9]' /tmp/wg2_app.txt

    # The camera, during traffic: the door must not starve the control
    # channel's drain. The PNG is WINDOW-rect sized (see this recipe's Δ) and
    # its CENTRE pixel is the fill Smalltalk chose from the system's theme.
    test -s "$SHOT"
    PNGDIM=$(od -An -tu1 -j16 -N8 "$SHOT" \
        | awk '{printf "%d %d", ($1*16777216+$2*65536+$3*256+$4), ($5*16777216+$6*65536+$7*256+$8)}')
    WINRECT=$(grep '^WG2 winrect ' /tmp/wg2_gate.txt | cut -d' ' -f3,4)
    echo "png=$PNGDIM windowrect=$WINRECT"
    test "$PNGDIM" = "$WINRECT"
    W=$(echo $PNGDIM | cut -d' ' -f1); H=$(echo $PNGDIM | cut -d' ' -f2)
    OFF=$(awk -v w=$W -v h=$H 'BEGIN{stride=1+w*4; raw=int(h/2)*stride+1+int(w/2)*4; \
        print 48+raw+5*int(raw/65535)}')
    BG=$(grep '^WG2 bg ' /tmp/wg2_gate.txt | cut -d' ' -f3)
    WANT=$(awk -v c="$BG" 'BEGIN{printf "%d %d %d", c%256, int(c/256)%256, int(c/65536)%256}')
    GOT=$(od -An -tu1 -j$OFF -N3 "$SHOT" | awk '{printf "%d %d %d", $1, $2, $3}')
    echo "png-centre=$GOT want(COLORREF $BG)=$WANT"
    test "$GOT" = "$WANT"

    # 3. WM_DESTROY posts the quit FROM SMALLTALK, not from Rust. The host
    #    keeps a backstop for the case where `onDestroy` raises, so the two
    #    paths print different lines and the gate asserts which one ran.
    for i in $(seq 1 60); do [ -f /tmp/wg2_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg2_exit)" = "0"
    grep -q 'WinShell: WM_CLOSE handled in Smalltalk' /tmp/wg2_app.txt
    grep -q 'WinShell: WM_DESTROY -> PostQuitMessage(0) from Smalltalk' /tmp/wg2_app.txt
    grep -q "handler posted the quit — nothing for the backstop to do" /tmp/wg2_app.txt
    ! grep -q 'BACKSTOP' /tmp/wg2_app.txt
    grep -q 'message loop ended, exit 0' /tmp/wg2_app.txt

    # The gallery shot, so a human can SEE a Smalltalk-handled resize even
    # though no human is needed to prove one. Its titlebar carries the size
    # WM_SIZE's lParam delivered, written back by the handler that read it —
    # which is what makes a picture of an unpainted window evidence.
    TITLE=$(grep '^WG2 title ' /tmp/wg2_gate.txt | cut -d' ' -f3-)
    SNAPC=$(grep '^WG2 snapclient ' /tmp/wg2_gate.txt | cut -d' ' -f3,4)
    echo "gallery titlebar reads: $TITLE (client $SNAPC)"
    echo "$TITLE" | grep -q "WM_SIZE handled in Smalltalk"
    # The titlebar must carry the size the MESSAGE delivered, and it must be
    # the size Win32 reports — the same cross-check as item 2, now visible in
    # a picture.
    echo "$TITLE" | grep -q "#($(echo $SNAPC | cut -d' ' -f1) $(echo $SNAPC | cut -d' ' -f2))"
    mkdir -p docs/gallery-win
    cp "$SHOT" docs/gallery-win/wg2-door-resize.png

    # world.list is still untouched: the base world stays byte-identical, and
    # WG2 added no third layer file either (91 grew; winui.list is unchanged),
    # which is what keeps WG1's Δ 13 from firing a second time.
    git diff --quiet -- world/world.list 2>/dev/null || true
    echo "gate-wg2: OK"

# WG3 (docs/sprints/tests_wg3.md): the FLAG-AND-DRAIN pass, then controls and
# layout. The order is the sprint's and it is not negotiable — Part 1 below has
# no control in it at all, because controls are what generate the storm the
# drain exists to absorb and retrofitting the pattern afterwards is how the
# Cocoa side acquired its "browser shows no data" scars.
#
# Chains gate-wg2, which chains gate-wg1 — so item 1 ("WG2's entire gate passes
# unchanged") is not a re-assertion here, it is the dependency. Both of those
# now run with MACVM_WINUI_CONTROLS=off, which is `tests_wg3.md` item 1's own
# phrase ("with the drain installed and NO CONTROL CREATED") as a token: WG3
# puts a themed list view in this window, and it paints WHITE pixels exactly
# where WG1 and WG2 read the background fill and sends NM_CUSTOMDRAW through a
# door those gates count messages at.
#
# Everything below is a script, for WG1's Δ 8 reason: `eval` answers INLINE in
# this host, so every item reads a number rather than sleeping and hoping.
gate-wg3: gate-wg2
    #!/usr/bin/env bash
    set -euo pipefail
    PORT=7651
    SHOT=target/winui-wg3.png
    cargo build --quiet
    cargo build --quiet -p win_gui

    # ── the in-language half, both DB states (item 11) ────────────────────
    grep -v '^#' world/tests/tests.list | grep -v '^$' | sed 's|^|world/tests/|' \
        | xargs cat > /tmp/macvm_wg3_tests.mst
    ./target/debug/macvm run /tmp/macvm_wg3_tests.mst --world world | tee /tmp/wg3_present.txt
    grep -q ', 0 failed' /tmp/wg3_present.txt
    WINKB_DB=/nonexistent/windows_api.db ./target/debug/macvm run /tmp/macvm_wg3_tests.mst \
        --world world | tee /tmp/wg3_absent.txt
    grep -q ', 0 failed' /tmp/wg3_absent.txt
    # All FOUR WG classes announce their DB-absent skip; none is silent.
    grep -q 'SKIP WinUiProbeTests' /tmp/wg3_absent.txt
    grep -q 'SKIP WinUiShellTests' /tmp/wg3_absent.txt
    grep -q 'SKIP WinUiDoorTests' /tmp/wg3_absent.txt
    grep -q 'SKIP WinUiControlsTests' /tmp/wg3_absent.txt
    # ...and the layout tests, which need neither winkb nor Win32 nor a window,
    # still ran. That asymmetry is the point of making `WinLayout` pure.
    grep -q 'testLayoutScalesWithDpi' /tmp/wg3_absent.txt
    # Counts are RELATIONSHIPS, never frozen integers (WG2 Δ 14): nothing lost,
    # nothing failing, and the WG3 suite strictly larger than WG2's was.
    PRESENT=$(grep -oE '^[0-9]+ run, 0 failed' /tmp/wg3_present.txt | cut -d' ' -f1)
    ABSENT=$(grep -oE '^[0-9]+ run, 0 failed' /tmp/wg3_absent.txt | cut -d' ' -f1)
    echo "world: $PRESENT with the database, $ABSENT without"
    test "$PRESENT" -ge 7762
    test "$ABSENT" -ge 7540
    test "$PRESENT" -gt "$ABSENT"

    # ── the crate's tests + the door's own (drain, tracking, D5's guard) ──
    cargo test --quiet -p win_gui
    cargo test --quiet --lib win_wndproc

    # ── the live half ────────────────────────────────────────────────────
    rm -f "$SHOT" /tmp/wg3_exit /tmp/wg3_app.txt
    ( MACVM_WINUI_CTL=$PORT ./target/debug/macvm-winui.exe > /tmp/wg3_app.txt 2>&1; \
      echo $? > /tmp/wg3_exit ) &
    for i in $(seq 1 60); do
        (exec 3<>/dev/tcp/127.0.0.1/$PORT) 2>/dev/null && break || sleep 0.5
    done
    ./target/debug/macvm rusttcl scripts/winui-wg3.tcl | tee /tmp/wg3_gate.txt
    cat /tmp/wg3_app.txt

    # ── Part 1, item 2: flags are serviced ────────────────────────────────
    # `lastLayoutSize` is what the DRAIN read from GetClientRect one pass after
    # the message; `client` is what GetClientRect says now. Two reads at two
    # times, one equality.
    grep -q 'WG3 flag-matches true' /tmp/wg3_gate.txt
    grep -q 'WG3 flag-pending false' /tmp/wg3_gate.txt
    grep -q 'WG3 flag-layouts 1' /tmp/wg3_gate.txt
    LL=$(grep '^WG3 flag-lastlayout ' /tmp/wg3_gate.txt | cut -d' ' -f3- | tr -d '#()')
    CL=$(grep '^WG3 flag-client ' /tmp/wg3_gate.txt | cut -d' ' -f3,4)
    echo "drain laid out $LL, GetClientRect says $CL"
    test "$LL" = "$CL"

    # ── Part 1, item 3: COALESCING, MEASURED ──────────────────────────────
    # The sprint's central claim. 200 resizes in one burst, one wake, and the
    # ratio REPORTED rather than asserted to a constant (WG2 Δ 14). A ratio of
    # 1.0 would mean the drain is not coalescing at all and the claim is false,
    # so the gate says exactly that if it ever sees one.
    SIZES=$(grep '^WG3 coalesce-sizecount ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    PASSES=$(grep '^WG3 coalesce-passes ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    LAYOUTS=$(grep '^WG3 coalesce-layouts ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    echo "COALESCING: $SIZES WM_SIZE messages -> $PASSES drain passes -> $LAYOUTS layout(s)"
    awk -v s="$SIZES" -v p="$PASSES" 'BEGIN{printf "  ratio passes/messages = %.4f\n", p/s}'
    test "$SIZES" -ge 200
    test "$PASSES" -lt "$SIZES"
    if [ "$PASSES" -ge "$SIZES" ]; then
        echo "COALESCING FAILED: the drain is not coalescing and WG3's central claim is false"
        exit 1
    fi
    # The settled state, not the last message's: ONE layout for 200 resizes,
    # against the size the window actually ended up.
    test "$LAYOUTS" -eq 1
    grep -q 'WG3 coalesce-matches true' /tmp/wg3_gate.txt
    # ...and the LATCH is what makes the passes few: 200 requests, one post.
    grep -qE 'WG3 burst .* requests=200 posts=1 passes=0 ' /tmp/wg3_gate.txt

    # ── Part 1, item 4: TRACKING SUPPRESSES ───────────────────────────────
    # ZERO passes while a modal loop is pumping, one layout after. Windows has
    # no NSDefaultRunLoopMode, so this is explicit or it does not happen.
    grep -q 'WG3 track-during-passes 0' /tmp/wg3_gate.txt
    grep -q 'WG3 track-during-sizecount 30' /tmp/wg3_gate.txt
    grep -qE 'WG3 track-during-drain drain tracking=true requested=true .* passes=0 ' /tmp/wg3_gate.txt
    # The flags ACCUMULATED rather than being dropped, and were serviced on the
    # far side: exactly one layout, against the final size.
    grep -q 'WG3 track-after-layouts 1' /tmp/wg3_gate.txt
    grep -q 'WG3 track-after-matches true' /tmp/wg3_gate.txt
    AFTER=$(grep '^WG3 track-after-passes ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    echo "tracking: 0 passes during 30 resizes, $AFTER after (bounded, not frozen)"
    test "$AFTER" -ge 1
    test "$AFTER" -le 8
    # ...and it was the real MESSAGES that did it, not a poked flag.
    grep -q 'WG3 track-enters 1' /tmp/wg3_gate.txt
    grep -q 'WG3 track-exits 1' /tmp/wg3_gate.txt

    # ── Part 1, item 5: the drain never re-enters ─────────────────────────
    # A pass that discovered more work may not post its own wake; it answers
    # non-zero and the HEARTBEAT services it. Bounded, not exact.
    grep -q 'WG3 reask-count 1' /tmp/wg3_gate.txt
    IMM=$(grep '^WG3 reask-immediate ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    HB=$(grep '^WG3 reask-after-heartbeat ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    SETTLED=$(grep '^WG3 reask-settled ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    echo "reask: $IMM pass immediately, $HB after a heartbeat, $SETTLED settled"
    test "$HB" -gt "$IMM"
    test "$SETTLED" -eq "$HB"
    test "$SETTLED" -le 6

    # ── stress: a pass that RAISES and a pass that FAULTS ─────────────────
    # The pass completes, the flag is CLEARED (no infinite retry), the next
    # pass runs normally, and a real ACCESS_VIOLATION inside a pass is
    # recovered with the window still there. A leaked depth guard here would
    # disable the drain forever with nothing in any log.
    grep -q 'WG3 raisepass-pending false' /tmp/wg3_gate.txt
    grep -q 'WG3 raisepass-then-matches true' /tmp/wg3_gate.txt
    grep -q 'WG3 faultpass-then-matches true' /tmp/wg3_gate.txt
    grep -q 'WG3 faultpass-alive 7' /tmp/wg3_gate.txt
    grep -q 'WG3 faultpass-ping pong' /tmp/wg3_gate.txt
    grep -q 'deliberate raise inside drainPass' /tmp/wg3_app.txt
    grep -q 'ACCESS_VIOLATION' /tmp/wg3_app.txt

    # ── Part 2, item 6: a BUTTON whose MEANING runs in the drain ──────────
    # The load-bearing four lines of this whole gate. With the drain held off
    # (D2's own lever), the synthesised WM_COMMAND is RECORDED — queued, id
    # noted — and its meaning has NOT run: no click counted, no pass taken.
    # Release the drain and the meaning runs, proven by Smalltalk state and by
    # text read back out of Win32 with GetWindowTextW.
    grep -q 'WG3 btn-alive true' /tmp/wg3_gate.txt
    grep -q 'WG3 btn-queued-in-door 1' /tmp/wg3_gate.txt
    grep -q 'WG3 btn-clicks-in-door 0' /tmp/wg3_gate.txt
    grep -q 'WG3 btn-passes-in-door 0' /tmp/wg3_gate.txt
    grep -q 'WG3 btn-clicks-after-drain 1' /tmp/wg3_gate.txt
    grep -q 'WG3 btn-queued-after-drain 0' /tmp/wg3_gate.txt
    grep -q "WG3 btn-status-text 'clicks: 1'" /tmp/wg3_gate.txt
    grep -q 'WG3 btn-command 100' /tmp/wg3_gate.txt
    # The click storm: 200 synthesised WM_COMMANDs, none lost, none doubled. A
    # command is an EVENT and queues; a resize is a STATE and coalesces to the
    # last. Both are "coalescing" and only one of them may drop anything.
    B4=$(grep '^WG3 storm-clicks-before ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    AFTERC=$(grep '^WG3 storm-clicks ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    echo "click storm: $B4 -> $AFTERC (delta must be exactly 200)"
    test $((AFTERC - B4)) -eq 200
    grep -q 'WG3 storm-work 200' /tmp/wg3_gate.txt
    grep -q 'WG3 storm-pending 0' /tmp/wg3_gate.txt

    # ── Part 2, items 7 and 10: LAYOUT, and DPI ───────────────────────────
    # Every control's real rect (GetWindowRect, in client coordinates) equals
    # the rect WinLayout computed. Two independent productions of the numbers.
    grep -q 'WG3 layout-matches true' /tmp/wg3_gate.txt
    WANT=$(grep '^WG3 layout-want ' /tmp/wg3_gate.txt | cut -d' ' -f3-)
    GOT=$(grep '^WG3 layout-got ' /tmp/wg3_gate.txt | cut -d' ' -f3-)
    echo "layout wants $WANT"
    echo "win32 says   $GOT"
    test "$WANT" = "$GOT"
    # DPI: the same layout at 1.5x the DPI is EXACTLY 1.5x the layout — an
    # equality, as WG1 established, and only assertable because the arithmetic
    # happens in DIP space and is converted once.
    ONE=$(grep '^WG3 dpi-1x ' /tmp/wg3_gate.txt | cut -d' ' -f3- | tr -d '#()')
    HALF=$(grep '^WG3 dpi-15x ' /tmp/wg3_gate.txt | cut -d' ' -f3- | tr -d '#()')
    SCALED=$(echo "$ONE" | awk '{printf "%d %d %d %d", $1*3/2, $2*3/2, $3*3/2, $4*3/2}')
    echo "dpi 96: $ONE   dpi 144: $HALF   (96 scaled by 1.5: $SCALED)"
    test "$HALF" = "$SCALED"

    # ── Part 2, item 9: WM_NOTIFY, its NMHDR read IN THE DOOR ─────────────
    # The pointer dies on return, so the three fields cross as Integers. The
    # assertion is that they are the RIGHT three: the list view's own HWND, its
    # child id, and LVN_ITEMCHANGED — all read in the door and surfacing one
    # pass later, when the pointer is long gone.
    LISTH=$(grep '^WG3 notify-listhandle ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    LISTID=$(grep '^WG3 list-id ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    WANTC=$(grep '^WG3 notify-wantcode ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    NOTIFY=$(grep '^WG3 notify-last ' /tmp/wg3_gate.txt | cut -d' ' -f3- | tr -d '#()')
    echo "NMHDR read in the door: $NOTIFY (want $LISTH $LISTID $WANTC)"
    test "$NOTIFY" = "$LISTH $LISTID $WANTC"

    # ── Part 2, item 8: THEMED, BY PIXEL ──────────────────────────────────
    # A manifest that failed to embed is otherwise INVISIBLE: comctl32 v5
    # registers the same classes, creates the same controls and passes every
    # functional assertion above while drawing them in their Windows-95 skin.
    # So read a pixel inside the button out of the PNG.
    #
    # The probe point is one sixth across the button, NOT its centre: a push
    # button's centre is where its text is, and ClearType's subpixel
    # antialiasing puts a strongly coloured pixel there (57,57,143 measured) on
    # themed and unthemed alike — which would pass this test for the wrong
    # reason.
    test -s "$SHOT"
    PNGDIM=$(od -An -tu1 -j16 -N8 "$SHOT" \
        | awk '{printf "%d %d", ($1*16777216+$2*65536+$3*256+$4), ($5*16777216+$6*65536+$7*256+$8)}')
    WINRECT=$(grep '^WG3 winrect ' /tmp/wg3_gate.txt | cut -d' ' -f3,4)
    echo "png=$PNGDIM windowrect=$WINRECT"
    test "$PNGDIM" = "$WINRECT"
    W=$(echo $PNGDIM | cut -d' ' -f1)
    PX=$(grep '^WG3 btnprobe ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    PY=$(grep '^WG3 btnprobe ' /tmp/wg3_gate.txt | cut -d' ' -f4)
    # The writer emits stored deflate blocks capped at 65535 bytes, so a 5-byte
    # header appears every 65535 raw bytes and each scanline carries a filter
    # byte — the same arithmetic WG2's gate had to learn (its Δ 6).
    OFF=$(awk -v w=$W -v x=$PX -v y=$PY 'BEGIN{raw=y*(1+w*4)+1+x*4; print 48+raw+5*int(raw/65535)}')
    FACE=$(od -An -tu1 -j$OFF -N3 "$SHOT" | awk '{printf "%d %d %d", $1, $2, $3}')
    UNTHEMED=$(grep '^WG3 unthemed ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    UNTHEMEDRGB=$(awk -v c="$UNTHEMED" 'BEGIN{printf "%d %d %d", c%256, int(c/256)%256, int(c/65536)%256}')
    BG=$(grep '^WG3 bg ' /tmp/wg3_gate.txt | cut -d' ' -f3)
    BGRGB=$(awk -v c="$BG" 'BEGIN{printf "%d %d %d", c%256, int(c/256)%256, int(c/65536)%256}')
    echo "button face at ($PX,$PY) = $FACE ; unthemed COLOR_BTNFACE = $UNTHEMEDRGB ; window fill = $BGRGB"
    test "$FACE" != "$UNTHEMEDRGB"
    # ...and it is a BUTTON that is drawn there, not the window showing through.
    test "$FACE" != "$BGRGB"

    # ── the window still closes the way WG2 taught it to ──────────────────
    # WG3 added a heartbeat timer and three child windows to that sequence.
    for i in $(seq 1 60); do [ -f /tmp/wg3_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg3_exit)" = "0"
    grep -q 'WinShell: WM_CLOSE handled in Smalltalk' /tmp/wg3_app.txt
    grep -q 'WinShell: WM_DESTROY -> PostQuitMessage(0) from Smalltalk' /tmp/wg3_app.txt
    ! grep -q 'BACKSTOP' /tmp/wg3_app.txt
    grep -q 'message loop ended, exit 0' /tmp/wg3_app.txt
    # The trampoline must never have caught a panic, and the depth guard must
    # be 0 at the end of all of it.
    grep -q 'panics=0' /tmp/wg3_gate.txt
    ! grep -q 'panics=[1-9]' /tmp/wg3_app.txt
    grep -qE 'WG3 finaldoor door enabled=true depth=0 ' /tmp/wg3_gate.txt
    # ...and no drain request was ever left permanently stranded.
    grep -qE 'WG3 finaldrain drain tracking=false requested=false ' /tmp/wg3_gate.txt

    mkdir -p docs/gallery-win
    cp "$SHOT" docs/gallery-win/wg3-controls.png

    # world.list is still untouched: the base world stays byte-identical, and
    # WG3's third layer file went into winui.list, which every consumer reads
    # rather than naming files (WG1's Δ 13).
    git diff --quiet -- world/world.list 2>/dev/null || true
    echo "gate-wg3: OK"

# P3 (tests_p03.md "Stress/negative tests") — the FLAKY-CATCHER. The
# combined three-mode run, three consecutive times, because WINVM's
# card-boundary bug was found by run-to-run variance and not by any single
# run: give variance a chance to speak. Release for the reason gate-p03's
# own header explains. Separate from the gate so the gate stays a
# single-pass claim and this stays the thing you run before signing a
# sprint off.
soak-p03:
    #!/usr/bin/env bash
    set -euo pipefail
    for i in 1 2 3; do
        echo "=== combined-stress pass $i/3 ==="
        MACVM_JIT=threshold=1 MACVM_GC_STRESS=1 MACVM_DEOPT_STRESS=1 \
            cargo test --release --no-fail-fast -- --skip mandelvm
    done

# Sprint acceptance gates. Later sprints append stress runs to their gate
# (e.g. `MACVM_GC_STRESS=1 just test` from S7 on).
gate-s00: ci
gate-s01: ci
gate-s02: ci
gate-s03: ci
gate-s04: ci
gate-s05: ci
gate-s06: ci

# S7: young-gen scavenger. Full suite green under MACVM_GC_STRESS=1
# (scavenge before every allocation) as well as stress off (via `ci`).
gate-s07: ci
    MACVM_GC_STRESS=1 cargo test

# S8: full mark-slide-compact GC (tests_s08.md's acceptance gate). Full
# suite green under stress off and =1 (via gate-s07), under =full (a full
# GC every 100 allocations), and the in-language suite specifically under
# the maximally aggressive =full:1 in debug. --test-threads=1 for the last
# step: a full GC on every single allocation is expensive per-call by
# design (it's the whole point of =full:1), and cargo test's default
# parallelism runs it_world's 6 tests concurrently — several of them
# CPU-heavy under this setting, including one that spawns a subprocess
# which ALSO loads the world under it — so contention alone turns a
# ~65s test into 4+ minutes with nothing actually wrong.
gate-s08: gate-s07
    MACVM_GC_STRESS=full cargo test
    MACVM_GC_STRESS=full:1 cargo test --test it_world -- --test-threads=1

# S9: vendored JASM wfasm + Assembler/JasmAssembler/CodeCache (tests_s09.md's
# acceptance gate). The no-LLVM check is warn-only (documents the corpus-
# replay-without-an-oracle claim; CI images without llvm make a hard fail
# impractical, and this dev machine has llvm via homebrew regardless). The
# P1 lint is a hard fail: a literal, comment-blind grep, so it also catches
# an explanatory comment that quotes its own trigger strings, not just a
# real re-introduced oracle dependency. it_codecache runs under --release
# specifically (not just via `ci`'s debug-mode `cargo test`) because W^X/
# icache bugs can hide in debug — this sprint found one exactly that way
# before this gate existed (patch_branch26's guard-ordering bug, only
# caught by actually running the integration tests, not by review).
gate-s09: gate-s08
    -command -v llvm-mc && echo "note: llvm-mc present -- no-LLVM claim not exercised this run"
    ! grep -rn 'crate::oracle\|feature = "llvm"' src/vendor/
    cargo test -p macvm
    cargo test -p macvm --release --test it_codecache
    cargo clippy --all-targets -- -D warnings

# S8 gate item 4: soak the full GC under sustained realistic churn with a
# continuous shadow-model integrity check (world/bench/soak.mst). The
# 2-minute CI variant runs routinely; the 1-hour variant is executed once
# per sprint sign-off with its numbers recorded in docs/PERF.md (both
# substitute the cycle count into the script's last line via sed, per
# world/bench's own hardcoded-literal convention — see soak.mst's own
# doc comment). Both run --release: debug-mode's unoptimized bytecode
# interpretation plus always-on verify_heap_at made even 10 cycles take
# 30+ seconds (0.6s under --release) — an interpretation-speed fact, not
# a GC one (confirmed by profiling before reaching for this fix).
soak-s08-ci:
    sed '$s/.*/Soak run: 400./' world/bench/soak.mst > /tmp/macvm_soak_ci.mst
    cargo run --release --quiet -- run /tmp/macvm_soak_ci.mst --world world

# S10 gate item 1 (differential): concatenate world/tests/tests.list's
# files (in order) into one temp .mst — TestRunner's SUnit-lite state
# (start/run:/report) must accumulate across them within ONE VM session,
# which `macvm run <one-file>` gives for free but N separate CLI
# invocations wouldn't. Plain concatenation is sound because each listed
# file is already independently well-formed top-level Smalltalk source
# (same reasoning `it_world.rs`'s own `load_tests_list` loop relies on,
# just done in the shell instead of in Rust so this is CLI/stdout-diffable
# under different MACVM_JIT values, not only assertable in-process).
run-world-tests:
    grep -v '^#' world/tests/tests.list | grep -v '^$' | sed 's|^|world/tests/|' | xargs cat > /tmp/macvm_world_tests.mst
    cargo run --quiet -- run /tmp/macvm_world_tests.mst --world world

soak-s08:
    sed '$s/.*/Soak run: 200000./' world/bench/soak.mst > /tmp/macvm_soak_1hr.mst
    MACVM_TRACE=gc cargo run --release --quiet -- run /tmp/macvm_soak_1hr.mst --world world

# S10 gate item 3 (perf marker, tracking not gating): world/bench/arith.mst's
# sumTo: 5_000_000 timed under MACVM_JIT=off vs threshold=1, --release (debug
# timing is noise, not signal). A shebang recipe, not just's default
# line-per-subprocess execution (each line of a plain recipe runs in its own
# shell, so a variable set on one line isn't visible on the next) -- needed
# here since interp_ms and jit_ms both have to survive to the same final
# append line.
bench-s10:
    #!/usr/bin/env bash
    set -euo pipefail
    interp_out=$(MACVM_JIT=off cargo run --release --quiet -- run world/bench/arith.mst --world world)
    jit_out=$(MACVM_JIT=threshold=1 cargo run --release --quiet -- run world/bench/arith.mst --world world)
    interp_ms=$(echo "$interp_out" | grep -o 'ms: [0-9]*' | grep -o '[0-9]*')
    jit_ms=$(echo "$jit_out" | grep -o 'ms: [0-9]*' | grep -o '[0-9]*')
    ratio=$(echo "scale=2; $interp_ms / $jit_ms" | bc)
    date_str=$(date +%Y-%m-%d)
    commit=$(git rev-parse --short HEAD)
    echo "| $date_str | $commit | $interp_ms | $jit_ms | ${ratio}x |" >> docs/PERF.md
    echo "arith bench: interp_ms=$interp_ms jit_ms=$jit_ms ratio=${ratio}x"
    below2=$(echo "$ratio < 2" | bc)
    below5=$(echo "$ratio < 5" | bc)
    if [ "$below2" = "1" ]; then
        echo "FAIL: compiled speedup ${ratio}x is below the 2x architectural-mistake tripwire" >&2
        exit 1
    fi
    if [ "$below5" = "1" ]; then
        echo "WARN: compiled speedup ${ratio}x is below the 5x target (tracking only, not gating)"
    fi

# S11 gate item 4 (perf marker, tracking not gating): world/bench/dispatch.mst's
# runLoop: 5_000_000 (a real per-iteration super-send dispatch, D4.6 -- see
# that file's own doc for why it isn't the literal 3-class polymorphic design
# tests_s11.md sketches) timed under MACVM_JIT=off vs threshold=1, --release.
# Same shebang-recipe shape as bench-s10, same warn<5x/fail<2x tripwire --
# expect a SMALLER ratio than arith.mst's ~130x, honestly (a real send still
# costs a real dispatch even compiled; this measures that cost, not erasing
# it), so the 5x warn line is more likely to actually fire here than on
# bench-s10 -- that is expected, not a regression.
bench-s11:
    #!/usr/bin/env bash
    set -euo pipefail
    interp_out=$(MACVM_JIT=off cargo run --release --quiet -- run world/bench/dispatch.mst --world world)
    jit_out=$(MACVM_JIT=threshold=1 cargo run --release --quiet -- run world/bench/dispatch.mst --world world)
    interp_ms=$(echo "$interp_out" | grep -o 'ms: [0-9]*' | grep -o '[0-9]*')
    jit_ms=$(echo "$jit_out" | grep -o 'ms: [0-9]*' | grep -o '[0-9]*')
    ratio=$(echo "scale=2; $interp_ms / $jit_ms" | bc)
    date_str=$(date +%Y-%m-%d)
    commit=$(git rev-parse --short HEAD)
    echo "| $date_str | $commit | $interp_ms | $jit_ms | ${ratio}x |" >> docs/PERF.md
    echo "dispatch bench: interp_ms=$interp_ms jit_ms=$jit_ms ratio=${ratio}x"
    below2=$(echo "$ratio < 2" | bc)
    below5=$(echo "$ratio < 5" | bc)
    if [ "$below2" = "1" ]; then
        echo "FAIL: compiled speedup ${ratio}x is below the 2x architectural-mistake tripwire" >&2
        exit 1
    fi
    if [ "$below5" = "1" ]; then
        echo "WARN: compiled speedup ${ratio}x is below the 5x target (tracking only, not gating)"
    fi

# W1 library-porting benchmark suite (world/bench/library_bench.mst) --
# steady-state cost of the newly-ported library surface (Set/Dictionary/
# Bag/SortedCollection/Fraction/Random/Point/Streams/...) under whatever
# tier is currently active. Unlike bench-s10/bench-s11, not a single
# tracked-over-time metric gated against a ratio -- just a convenient way
# to run the whole suite and see where the interpreter spends time.
bench-library:
    cargo run --release --quiet -- run world/bench/library_bench.mst --world world

# S10: tier-1 JIT compiler (tests_s10.md's acceptance gate). gate-s09
# already covers "cargo test" + "cargo clippy -- -D warnings" (tests_s10.md
# gate script's own first/last lines) via the dependency chain, so this
# recipe is just the S10-specific middle: the off-vs-threshold=1
# differential byte-identical (gate item 1), GC-stress regression under the
# JIT's default (off) mode -- S10 never combines jit+stress, that's S12's
# flagship, not tested here -- and the perf marker recorded (gate item 3).
gate-s10: gate-s09
    MACVM_JIT=off just run-world-tests > /tmp/s10_off.txt
    MACVM_JIT=threshold=1 just run-world-tests > /tmp/s10_t1.txt
    diff /tmp/s10_off.txt /tmp/s10_t1.txt
    MACVM_GC_STRESS=1 just run-world-tests
    MACVM_GC_STRESS=full:64 just run-world-tests
    just bench-s10

# INVERTED by S12 step 7 (P10: "the S11 gc_under_compiled == 0 test must be
# inverted, not deleted — it becomes the proof the bridge is gone"). Same
# recipe name, same combined-stress run, OPPOSITE assertion: the exit-time
# "gc: gc_under_compiled=M" line (print_gc_bridge_stats, main.rs) must now
# show M > 0 — real collections genuinely ran with live compiled frames on
# the native stack (the hard case S12 exists for), release-mode-visible
# rather than debug-assert-only. bridge_old_allocs no longer exists (the
# field is deleted; anything referencing it fails to compile — tests_s12.md
# gate item 6).
bridge-stats-s11:
    #!/usr/bin/env bash
    set -euo pipefail
    grep -v '^#' world/tests/tests.list | grep -v '^$' | sed 's|^|world/tests/|' | xargs cat > /tmp/macvm_world_tests.mst
    # GC_STRESS=1 (scavenge on EVERY allocation), not full:64: the sampled
    # mode can legitimately land all of its collections outside compiled
    # windows (1-in-64 across a suite whose compiled stretches are short),
    # reading 0 without anything being wrong -- the every-allocation mode
    # is the one that guarantees collections inside compiled windows exist
    # to count.
    out=$(MACVM_GC_STRESS=1 MACVM_JIT=threshold=1 MACVM_TRACE=gc cargo run --quiet -- run /tmp/macvm_world_tests.mst --world world 2>&1 >/dev/null)
    line=$(echo "$out" | grep '^gc: gc_under_compiled=')
    echo "$line"
    under_compiled=$(echo "$line" | sed -n 's/.*gc_under_compiled=\([0-9]*\).*/\1/p')
    if [ "$under_compiled" = "0" ]; then
        echo "FAIL: gc_under_compiled=0 -- the bridge is gone, so combined stress MUST have run collections under live compiled frames (S12 P10)" >&2
        exit 1
    fi
    date_str=$(date +%Y-%m-%d)
    commit=$(git rev-parse --short HEAD)
    echo "| $date_str | $commit | (bridge deleted) | $under_compiled |" >> docs/PERF.md

# S11: compiled sends + inline alloc + the D8 GC bridge. UNLIKE gate-s10 (which
# deliberately kept GC-stress and the JIT apart, deferring the combo to "S12's
# flagship"), this gate COMBINES them: MACVM_GC_STRESS + MACVM_JIT=threshold=1
# TOGETHER. That is the only way to actually stress-test the collector against
# compiled code + inline allocation, and it is now sound because (a) the D8
# bridge suppresses moving GC while a compiled frame is live and (b) the
# scavenge updates nmethod/PIC/mega Rust-side keys (key_klass/key_selector),
# not just their code-pool oops. Running these two combined for the first time
# is exactly what surfaced that pre-existing scavenge-key use-after-free.
gate-s11: gate-s10
    MACVM_GC_STRESS=1 MACVM_JIT=threshold=1 just run-world-tests
    MACVM_GC_STRESS=full:64 MACVM_JIT=threshold=1 just run-world-tests
    just bridge-stats-s11
    just bench-s11

# S12 flagship soak (tests_s12.md gate step 7): 10M short-lived objects
# through a COMPILED allocation loop (threshold=1, default GC), flat memory
# ceiling asserted EXACTLY by the script itself (oldUsed/oldCommitted
# byte-identical before/after the measured window -- steady-state churn
# promotes nothing). The file is alloc_churn.mst, not the doc's own
# "churn.mst": S10 had already taken that name for its compile-disabled
# churn stress.
soak-s12:
    MACVM_JIT=threshold=1 cargo run --release --quiet -- run world/bench/alloc_churn.mst --world world

# S12: moving GC under compiled frames -- THE flagship gate (tests_s12.md
# "Acceptance gate" + its step-by-step procedure). gate-s11 (chained) already
# reruns the combined-stress suites and bridge-stats-s11 -- which S12 step 7
# INVERTED: it now asserts gc_under_compiled > 0, i.e. real collections ran
# with live compiled frames on the native stack (P10). What this recipe adds
# on top: the BYTE-IDENTICAL requirement across both combined-stress modes
# (gate item 1 -- gate-s11 only checked they pass; the diff is the flagship's
# actual claim), the S7 torture harness re-run with tier 1 ON, and the
# compiled-allocation-loop soak. bridge_old_allocs' deletion is enforced at
# compile time (the field is gone -- gate item "counter DELETED, compile
# error if referenced"); oopmap_at exactness needs no separate check (a miss
# panics, so any of these runs would die loudly, P1).
gate-s12: gate-s11
    MACVM_JIT=off just run-world-tests > /tmp/s12_gate_base.txt
    MACVM_GC_STRESS=1 MACVM_JIT=threshold=1 just run-world-tests > /tmp/s12_gate_a.txt
    MACVM_GC_STRESS=full:64 MACVM_JIT=threshold=1 just run-world-tests > /tmp/s12_gate_b.txt
    diff /tmp/s12_gate_base.txt /tmp/s12_gate_a.txt
    diff /tmp/s12_gate_base.txt /tmp/s12_gate_b.txt
    sed '$s/.*/Soak run: 400./' world/bench/soak.mst > /tmp/macvm_soak_s12.mst
    MACVM_JIT=threshold=1 cargo run --release --quiet -- run /tmp/macvm_soak_s12.mst --world world
    just soak-s12

# WG4 (docs/ROADMAP.md's WG4 row): the shell — a view bar, views that build
# LAZILY, a transcript dock that collapses as a height change, and a metrics
# cluster that takes one sample at a time.
#
# Depends on gate-wg3, and the dependency is the point: WG4's window is WG3's
# window with a grammar on top, so a WG4 gate that passed while WG3's had
# broken would be measuring the wrong thing. gate-wg3 runs with
# MACVM_WINUI_WG4=off (its assertions are about WG3's three controls in WG3's
# three bands); this one runs with the shell on, which is the default.
gate-wg4: gate-wg3
    #!/usr/bin/env bash
    set -euo pipefail
    PORT=7671
    SHOT=target/winui-wg4.png
    cargo build --quiet
    cargo build --quiet -p win_gui

    # ── the in-language half ─────────────────────────────────────────────
    # The layout grammar and the view registry are pure guest state, so they
    # are checked with no window at all — the same split WG3 drew, and the
    # reason WG4's arithmetic is testable on a machine with no Win32.
    grep -v '^#' world/tests/tests.list | grep -v '^$' | sed 's|^|world/tests/|' \
        | xargs cat > /tmp/macvm_wg4_tests.mst
    ./target/debug/macvm run /tmp/macvm_wg4_tests.mst --world world | tee /tmp/wg4_present.txt
    grep -q ', 0 failed' /tmp/wg4_present.txt
    grep -q 'WinUiShellWg4Tests' /tmp/wg4_present.txt
    # Counts are RELATIONSHIPS, never frozen integers (WG2 Δ 14): WG4's suite
    # is strictly larger than WG3's was, and nothing is failing.
    PRESENT=$(grep -oE '^[0-9]+ run, 0 failed' /tmp/wg4_present.txt | cut -d' ' -f1)
    echo "world: $PRESENT with the shell layer loaded"
    test "$PRESENT" -ge 7850

    # ── the live half ────────────────────────────────────────────────────
    rm -f "$SHOT" /tmp/wg4_exit /tmp/wg4_app.txt
    ( MACVM_WINUI_CTL=$PORT ./target/debug/macvm-winui.exe > /tmp/wg4_app.txt 2>&1; \
      echo $? > /tmp/wg4_exit ) &
    for i in $(seq 1 60); do
        (exec 3<>/dev/tcp/127.0.0.1/$PORT) 2>/dev/null && break || sleep 0.5
    done
    ./target/debug/macvm rusttcl scripts/winui-wg4.tcl | tee /tmp/wg4_gate.txt
    cat /tmp/wg4_app.txt

    # ── the shell came up ─────────────────────────────────────────────────
    grep -q 'WG4 open true' /tmp/wg4_gate.txt
    grep -q 'WG4 enabled true' /tmp/wg4_gate.txt

    # ── LAZY BUILD, which is the WG4 row's own word ───────────────────────
    # Views registered, and exactly ONE built at open. A shell that built every
    # view up front would answer the full count here and would pay every future
    # view's handles at startup — the difference this assertion exists for.
    #
    # A RELATIONSHIP, not a frozen integer (WG2 Δ 14): this read `WG4 views 3`
    # and the shell has had seven since WG6a, so it had been failing silently
    # for three sprints. The claim was never "there are three views" — it is
    # "registering a view does not build it", and that is what is asserted now.
    VIEWS=$(grep -E '^WG4 views ' /tmp/wg4_gate.txt | awk '{print $NF}')
    echo "shell: $VIEWS views registered, 1 built"
    test "$VIEWS" -ge 3
    grep -q 'WG4 built-at-open 1' /tmp/wg4_gate.txt
    grep -q "WG4 active-at-open #welcome" /tmp/wg4_gate.txt

    # A real WM_COMMAND crossed the door, was QUEUED there, and the view
    # switched one drain pass later — the flag-and-drain contract, at the
    # shell's own layer.
    grep -q 'WG4 queued-in-door 1' /tmp/wg4_gate.txt
    grep -q "WG4 active-after-click #transcript" /tmp/wg4_gate.txt
    grep -q 'WG4 built-after-click 2' /tmp/wg4_gate.txt

    # Switching to a view ALREADY built builds nothing: the build count holds
    # at 2 while the switch count goes up. A shell that rebuilt per visit
    # would leak a window handle per click, invisibly, until it wasn't.
    grep -q 'WG4 built-after-reswitch 2' /tmp/wg4_gate.txt
    grep -q 'WG4 switches-after-reswitch 3' /tmp/wg4_gate.txt
    # ...and a third view still builds on ITS first visit.
    grep -q 'WG4 built-third 3' /tmp/wg4_gate.txt

    # ── the dock is a HEIGHT change, not a structural one ─────────────────
    # Collapsed is height 0 with the band still present; reopening restores a
    # number rather than reconstructing a band.
    grep -q 'WG4 dock-collapsed-0 false' /tmp/wg4_gate.txt
    grep -q 'WG4 dock-height-0 120' /tmp/wg4_gate.txt
    grep -q 'WG4 dock-collapsed-1 true' /tmp/wg4_gate.txt
    grep -q 'WG4 dock-height-1 0' /tmp/wg4_gate.txt
    grep -q 'WG4 dock-collapsed-2 false' /tmp/wg4_gate.txt
    grep -q 'WG4 dock-height-2 120' /tmp/wg4_gate.txt

    # ── the metrics cluster, read back out of Win32 ───────────────────────
    # Not out of the Smalltalk variable: the claim is that the CONTROL shows
    # the sample, and only GetWindowTextW can settle that.
    grep -q 'WG4 metrics-updates 1' /tmp/wg4_gate.txt
    grep -q 'MEM 540K/68M' /tmp/wg4_gate.txt
    grep -q 'GC 2/4' /tmp/wg4_gate.txt

    # ── the transcript: newest first, and it BREAKS ───────────────────────
    # A Win32 multiline EDIT breaks on CRLF and on nothing else. The first WG4
    # snap ran every line together; this is the assertion that catches it.
    grep -q "WG4 transcript-first 'gate line two'" /tmp/wg4_gate.txt
    grep -q 'WG4 transcript-breaks true' /tmp/wg4_gate.txt

    # ── the layout actually placed the shell's children ───────────────────
    grep -q 'WG4 metrics-right-of-buttons true' /tmp/wg4_gate.txt
    LAYOUTS=$(grep '^WG4 layouts ' /tmp/wg4_gate.txt | cut -d' ' -f3)
    test "$LAYOUTS" -ge 1

    # ── D3: the bar actually painted, and nothing raised inside a paint ───
    DRAWN=$(grep '^WG4 draw-calls ' /tmp/wg4_gate.txt | cut -d' ' -f3)
    test "$DRAWN" -ge 5
    grep -q "WG4 draw-error $" /tmp/wg4_gate.txt || grep -q "WG4 draw-error ''" /tmp/wg4_gate.txt
    grep -q 'WG4 accent-read true' /tmp/wg4_gate.txt

    # ── D4: enablement tracks FOCUS, and answers both ways ────────────────
    grep -q 'WG4 edit-is-source true' /tmp/wg4_gate.txt
    grep -q 'WG4 ro-is-source false' /tmp/wg4_gate.txt
    grep -q 'WG4 enabled-on-source true' /tmp/wg4_gate.txt
    grep -q 'WG4 enabled-off-source false' /tmp/wg4_gate.txt

    # ── D5: the dock is the user's, and clamps ────────────────────────────
    grep -q 'WG4 dock-set 150' /tmp/wg4_gate.txt
    grep -q 'WG4 dock-floor true' /tmp/wg4_gate.txt
    grep -q 'WG4 dock-ceiling true' /tmp/wg4_gate.txt

    # ── D6: a switch costs NO layout. The flicker assertion. ──────────────
    BEFORE=$(grep '^WG4 layouts-before-switch ' /tmp/wg4_gate.txt | cut -d' ' -f3)
    AFTER=$(grep '^WG4 layouts-after-switch ' /tmp/wg4_gate.txt | cut -d' ' -f3)
    echo "layouts across two view switches: $BEFORE -> $AFTER"
    test "$BEFORE" -eq "$AFTER"

    # ── the window survived all of it ─────────────────────────────────────
    test -s "$SHOT"
    echo "snap: $SHOT ($(wc -c < "$SHOT") bytes)"
    { echo "gui connect $PORT"; echo "gui quit"; } > /tmp/wg4_quit.tcl
    ./target/debug/macvm rusttcl /tmp/wg4_quit.tcl >/dev/null 2>&1 || true
    for i in $(seq 1 60); do [ -f /tmp/wg4_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg4_exit)" = "0"
    echo "gate-wg4: PASS"

# WG5b-2 (docs/sprints/sprint_wg5_detail.md): Accept, over
# `image_store::flows`. The CG8 gate re-run for Windows — *a `#saveMethod`
# round-trips through `image_store` byte-identically to the web edit path*.
#
# THREE LAYERS, because each proves something the others cannot:
#
#   1. `cargo test -p winui_host` — the DIFFERENTIAL. The same save through
#      the Windows entry point and through `flows::save_method` (the web
#      GUI's own call), against two identically-seeded images, compared on
#      every stored consequence: source, selector, side, home file, version
#      count. This is the CG8 gate proper.
#   2. The world suite's `WinUiHostWg5bTests` — the CHANNEL. That an FFI
#      pragma naming `library:` resolves a DLL in neither winkb nor the
#      five-DLL probe list, which is WG5b-2's one core change.
#   3. `world/bench/wg5b_accept.mst` — END TO END. A Smalltalk String through
#      `nativeUtf16:`, LoadLibraryA, the A64 trampoline, `image_store`, and
#      back out as UTF-16 read by count. It drives `acceptSourceText:`, the
#      SAME method the Accept cell reaches.
#
# The scratch image is built fresh and thrown away. Nothing here writes
# `world/image.sqlite3` — which does not exist in a checkout anyway (it is a
# generated artifact) and must never be created as a side effect of a gate.
gate-wg5b:
    #!/usr/bin/env bash
    set -euo pipefail
    IMG=/tmp/wg5b_gate.sqlite3
    # A LEFTOVER WINDOW BLOCKS THE BUILD, and this is new with WG5b-2: the GUI
    # now LOADS winui_host.dll, so a still-running macvm-winui.exe holds it
    # open and `cargo build -p winui_host` fails with "Access is denied" --
    # a message that says nothing about the real cause. Close it first.
    taskkill //F //IM macvm-winui.exe > /dev/null 2>&1 || true
    # Fingerprint the developer's own image up front, so the last assertion
    # can prove no part of this gate wrote it.
    OWN_IMAGE_SUM=""
    if [ -f world/image.sqlite3 ]; then OWN_IMAGE_SUM="$(md5sum < world/image.sqlite3)"; fi
    cargo build -p winui_host
    # 1: the CG8 differential.
    cargo test -p winui_host
    # 2: the channel, inside the ordinary world suite.
    just run-world-tests | tee /tmp/wg5b_world.txt
    grep -q '0 failed' /tmp/wg5b_world.txt
    # 3: end to end, against a scratch image.
    rm -f "$IMG"
    cargo run -q -p image_store --bin import_world -- world "$IMG"
    grep -v '^#' world/winui.list | grep -v '^$' | sed 's|^|world/|' \
        | xargs cat > /tmp/wg5b_layer.mst
    cat /tmp/wg5b_layer.mst world/bench/wg5b_accept.mst > /tmp/wg5b_run.mst
    MACVM_IMAGE_PATH="$IMG" cargo run -q -- run /tmp/wg5b_run.mst --world world \
        | tee /tmp/wg5b_e2e.txt
    grep -q 'ALL CHECKS PASSED' /tmp/wg5b_e2e.txt
    rm -f "$IMG"

    # 4: THE WINDOW. The three layers above are all headless; none of them can
    #    tell you the Accept cell was actually built, drawn by the bar's own
    #    owner-draw path, and greyed by the same focus rule as the other two
    #    verbs -- or that `library:` resolves in the process that really owns
    #    the window rather than in a test harness. Same shape as gate-wg4.
    rm -f /tmp/wg5b_exit /tmp/wg5b_app.txt target/winui-wg5b.png
    ( MACVM_WINUI_CTL=7671 ./target/debug/macvm-winui.exe > /tmp/wg5b_app.txt 2>&1;       echo $? > /tmp/wg5b_exit ) &
    for i in $(seq 1 60); do
        (exec 3<>/dev/tcp/127.0.0.1/7671) 2>/dev/null && break || sleep 0.5
    done
    ./target/debug/macvm rusttcl scripts/winui-wg5b.tcl | tee /tmp/wg5b_gate.txt
    # The browser fills from a REPLY across the seam, and on a fresh start the
    # primary is still loading the world. The wait has to happen with NOTHING
    # attached: `gui sleep` blocks the app itself, so waiting inside the
    # script starves the drain pass it is waiting for.
    sleep 15
    ./target/debug/macvm rusttcl scripts/winui-wg5b-2.tcl | tee -a /tmp/wg5b_gate.txt
    cat /tmp/wg5b_app.txt

    grep -q 'WG5B open true' /tmp/wg5b_gate.txt
    # The verb is there and is a BAR cell -- owner-drawn like Do It and Print
    # It, not a stock Win32 button dropped into a Fluent strip.
    grep -q 'WG5B accept-exists true' /tmp/wg5b_gate.txt
    grep -q 'WG5B accept-is-bar-cell true' /tmp/wg5b_gate.txt
    # And it JOINED WG4 D4's focus rule rather than getting one of its own: a
    # read-only surface cannot supply source, so Accept is off.
    grep -q 'WG5B accept-on-readonly false' /tmp/wg5b_gate.txt
    # The strip painted its own first frame -- it used to come up as a row of
    # empty boxes and stay that way until something unrelated dirtied it.
    DRAWN=$(grep -E '^WG5B drawcalls-at-open ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    echo "bar: $DRAWN cells drawn before any click"
    test "$DRAWN" -gt 0
    # A VERB SURVIVES BEING CLICKED. Clicking one used to move focus onto it,
    # which made it not-a-source-surface, which disabled it -- and a disabled
    # button never sends WM_COMMAND. The verb switched itself off on the way
    # down. Both halves asserted, in the order that breaks.
    grep -q 'WG5B enabled-on-workspace true' /tmp/wg5b_gate.txt
    grep -q 'WG5B enabled-after-clicking-a-verb true' /tmp/wg5b_gate.txt
    grep -q 'WG5B doit-still-clickable true' /tmp/wg5b_gate.txt
    # And the click actually reaches Do It.
    grep -q 'WG5B doit-fired 1' /tmp/wg5b_gate.txt
    # The Browser filled from the PRIMARY, across the seam.
    grep -q 'WG5B active-view #browser' /tmp/wg5b_gate.txt
    # `grep -oE '[0-9]+'` on the whole line would also match the 5 in WG5B --
    # which yielded two lines (5, then 0) and a `test: integer expected`.
    # Take the last field instead.
    CLASSES=$(grep -E '^WG5B browser-classes ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    echo "browser: $CLASSES classes from the primary's live hierarchy"
    test "$CLASSES" -ge 100
    # The source pane keeps WG5b-1's promise instead of restating it.
    grep -q 'WG5B source-still-a-promise false' /tmp/wg5b_gate.txt
    # WG5c D3: the swap really happened, and the enablement predicate learned
    # the new class in the same change.
    grep -q 'WG5C richedit-loaded true' /tmp/wg5b_gate.txt
    grep -q "WG5C pane-class 'RICHEDIT50W'" /tmp/wg5b_gate.txt
    grep -q 'WG5C workspace-is-source true' /tmp/wg5b_gate.txt
    grep -q 'WG5C browser-source-is-source true' /tmp/wg5b_gate.txt
    # WG5c D5: the ghost line, whose predecessor shipped invisible. The
    # window claims first, then the property that makes the design safe.
    grep -q 'WG5D ghost-shown-when-empty true' /tmp/wg5b_gate.txt
    grep -q 'WG5D ghost-gone-when-typed false' /tmp/wg5b_gate.txt
    grep -q "WG5D drew-without-error ''" /tmp/wg5b_gate.txt
    # THE HINT IS NOT IN THE DOCUMENT. If it were, Do It would evaluate it and
    # Accept would write it — which is why it is an overlay and not greyed
    # text in the pane.
    grep -q 'WG5D buffer-really-empty true' /tmp/wg5b_gate.txt
    grep -q 'WG5D doit-would-see-nothing true' /tmp/wg5b_gate.txt
    # WG5c D4: runs are not merely COMPUTED, they are APPLIED — and the
    # selection survives the pass, which is what separates colouring from
    # sabotaging the editor.
    FOUND=$(grep -E '^WG5C runs-found ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    APPLIED=$(grep -E '^WG5C runs-applied ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    echo "colour: $FOUND runs found, $APPLIED applied"
    test "$FOUND" -gt 0
    test "$APPLIED" = "$FOUND"
    grep -q 'WG5C selection-survives #(4 8)' /tmp/wg5b_gate.txt
    # And a real EN_CHANGE through the door reached the debounce, which fired.
    BEFORE=$(grep -E '^WG5C passes-before-idle ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    AFTER=$(grep -E '^WG5C passes-after-idle ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    echo "colour: $BEFORE passes before idle, $AFTER after"
    test "$AFTER" -gt "$BEFORE"
    # WG6b: Find really searched, really filled its list, and the jump
    # landed. The counts are RELATIONSHIPS (WG2 Δ 14) — the world grows.
    IMPL=$(grep -E '^WG6B implementors ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    LB=$(grep -E '^WG6B listbox-rows ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    echo "find: $IMPL implementors of printString, $LB rows shown"
    test "$IMPL" -gt 0
    # THE LIST MUST MATCH THE SEARCH. It once did not: `listSet:items:` takes
    # a NAME and was handed a control, so it returned in silence and the view
    # showed an empty list while the transcript reported five hits.
    test "$LB" = "$IMPL"
    # The jump is the payoff, so it is gated as hard as the search.
    grep -q 'WG6B jumped true' /tmp/wg5b_gate.txt
    grep -q 'WG6B landed-view #browser' /tmp/wg5b_gate.txt
    grep -q "WG6B landed-selector 'printString'" /tmp/wg5b_gate.txt
    # WG6a: the Outliner rendered the primary's own tree, in a real window.
    grep -q 'WG6A active-view #outliner' /tmp/wg5b_gate.txt
    ROWS=$(grep -E '^WG6A rows ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    echo "outliner: $ROWS class rows over the primary's hierarchy"
    # The HIERARCHY only — a class's own rows are added when it is selected.
    # Eager insertion was 3507 rows and ~600ms of synchronous UI work on the
    # first open, which timed out three control-port calls in a row and would
    # have been a visible freeze on a view switch. A relationship, not a
    # frozen integer: the world grows.
    test "$ROWS" -gt 100
    test "$ROWS" -lt 1000
    grep -q 'WG6A built true' /tmp/wg5b_gate.txt
    # TVINSERTSTRUCTW's recorded size is 24 and would be a 48-byte heap
    # overwrite; the composed one must be larger. Asserted as a relationship.
    REC=$(grep -E '^WG6A tv-recorded ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    COMP=$(grep -E '^WG6A tv-composed ' /tmp/wg5b_gate.txt | awk '{print $NF}')
    echo "TVINSERTSTRUCTW: recorded $REC, composed $COMP"
    test "$COMP" -gt "$REC"
    # And the channel resolves HERE, in the process that owns the window.
    grep -q 'WG5B host-available true' /tmp/wg5b_gate.txt
    grep -q 'WG5B host-ping 22343' /tmp/wg5b_gate.txt
    test -s target/winui-wg5b.png
    for i in $(seq 1 60); do [ -f /tmp/wg5b_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg5b_exit)" = "0"

    # And the developer's OWN image is untouched. Not "does not exist" -- it
    # legitimately does once `import_world` has been run, and the GUI needs it
    # to show source at all. What must hold is that a GATE never writes it:
    # the write path is exercised against $IMG and nowhere else.
    if [ -f world/image.sqlite3 ]; then
        test "$(md5sum < world/image.sqlite3)" = "$OWN_IMAGE_SUM"
        echo "own image: unchanged"
    fi
    echo "gate-wg5b: PASS"

# --- WG6c-2: the editor pane accepts input ----------------------------------
#
# docs/sprints/sprint_wg6c_detail.md, WG6c-2. The slice's own gate is one
# sentence — "typing changes the document and undo restores it, which costs
# nothing because the rope is persistent" — and it is asserted here twice, from
# the two sides that can fail independently:
#
#   1. HEADLESS, the arithmetic. `world/tests/67_winui_editor_tests.mst`:
#      offset <-> (line, col) round-trips over EVERY position, pixel <-> offset
#      inverts, the caret clamps, typing-then-undo restores. Pure over the
#      text, so it needs no window and runs on every platform's suite.
#   2. THE WINDOW, the route. Nothing headless touches WG6c-2's actual shell
#      change, which is four lines in 91's door routing by HWND to the pane —
#      67 calls `editorApply:` directly and never crosses
#      `window:message:wParam:lParam:` at all. A pane wired up wrongly passes
#      all six of those tests and is stone dead on screen, so the second half
#      drives REAL WM_CHAR / WM_KEYDOWN / WM_LBUTTONDOWN through the real door.
#
# WHY A REBUILD IS NOT OPTIONAL HERE, and it cost an afternoon to find: the
# world's `.mst` files are read from disk at runtime, so an editor change shows
# up in a stale `macvm-winui.exe` — but WM_PAINT joining the ALLOWLIST is Rust,
# compiled in. Running WG6c against a binary built before that commit gives a
# pane that is visible, focused, correctly positioned, types perfectly, and
# never paints once. `cargo build` is therefore part of the gate rather than
# something the developer is trusted to remember.
gate-wg6c:
    #!/usr/bin/env bash
    set -euo pipefail
    # A leftover window holds its own .exe open and the next build fails with
    # "Access is denied" — a message that says nothing about the real cause.
    taskkill //F //IM macvm-winui.exe > /dev/null 2>&1 || true

    # 1: the arithmetic, inside the ordinary world suite.
    just run-world-tests | tee /tmp/wg6c_world.txt
    grep -q ', 0 failed' /tmp/wg6c_world.txt
    # The suite must have actually RUN it. A test file that fell out of
    # tests.list still reports "0 failed", which is the most comfortable way
    # for a gate to pass while checking nothing.
    grep -q 'WinUiEditorWg6cTests' /tmp/wg6c_world.txt
    grep -q 'testTypingThenUndoRestoresExactly' /tmp/wg6c_world.txt
    grep -q 'testReplacingASelectionIsOneUndo' /tmp/wg6c_world.txt
    grep -q 'testSelectionRectsCoverExactlyTheSelectedLines' /tmp/wg6c_world.txt

    # 2: THE WINDOW.
    #
    # WG6c-3 makes this the first winui gate whose WINDOW can WRITE — the
    # Editor's Save cell reaches `image_store::flows` through winui_host.dll —
    # so it runs against a SCRATCH image and proves the developer's own is
    # untouched, exactly as gate-wg5b does for its end-to-end leg. A gate that
    # wrote `world/image.sqlite3` as a side effect of taking a screenshot
    # would be a genuinely nasty thing to leave behind.
    OWN_IMAGE_SUM=""
    if [ -f world/image.sqlite3 ]; then OWN_IMAGE_SUM="$(md5sum < world/image.sqlite3)"; fi
    IMG=/tmp/wg6c_gate.sqlite3
    rm -f "$IMG"
    cargo build --quiet -p win_gui
    cargo build --quiet -p winui_host
    rm -f /tmp/wg6c_exit /tmp/wg6c_app.txt target/winui-wg6c.png
    ( MACVM_WINUI_CTL=7673 MACVM_IMAGE_PATH="$IMG" ./target/debug/macvm-winui.exe > /tmp/wg6c_app.txt 2>&1; \
        echo $? > /tmp/wg6c_exit ) &
    for i in $(seq 1 60); do
        (exec 3<>/dev/tcp/127.0.0.1/7673) 2>/dev/null && break || sleep 0.5
    done
    ./target/debug/macvm rusttcl scripts/winui-wg6c.tcl | tee /tmp/wg6c_gate.txt
    # The picker fills from a REPLY across the seam, and the wait has to happen
    # with NOTHING attached: `gui sleep` blocks the app itself, so waiting
    # inside the script starves the drain pass it is waiting for. Same shape,
    # same reason, as gate-wg5b's own split.
    sleep 15
    ./target/debug/macvm rusttcl scripts/winui-wg6c-2.tcl | tee -a /tmp/wg6c_gate.txt
    cat /tmp/wg6c_app.txt

    grep -q 'WG6C open true' /tmp/wg6c_gate.txt
    grep -q 'WG6C pane-exists true' /tmp/wg6c_gate.txt
    grep -q 'WG6C pane-hwnd-nonzero true' /tmp/wg6c_gate.txt
    # THE FONT WAS MEASURED before any pixel arithmetic. `editorCharWidth`
    # answers a DEFAULT of 8 until the first paint runs DT_CALCRECT and finds
    # the real 7, and it changes exactly once — so a click encoded before that
    # transition and decoded after it lands somewhere else entirely. That is
    # not hypothetical: it moved a click from line 1 column 4 to the end of the
    # document when repaints were added elsewhere in the script.
    grep -q 'WG6C metrics-measured true' /tmp/wg6c_gate.txt
    # FOCUS IS THE WHOLE ARCHITECTURAL CLAIM. WG6c-1 rejected an SS_OWNERDRAW
    # STATIC because a STATIC can hold focus and still never receive a
    # WM_KEYDOWN — focusable and mute. This asserts the half that was easy.
    grep -q 'WG6C pane-has-focus true' /tmp/wg6c_gate.txt

    # TYPING, through a real WM_CHAR from outside every VM entry.
    BEFORE=$(grep -E '^WG6C doc-before ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    AFTER=$(grep -E '^WG6C doc-after-typing ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    echo "editor: $BEFORE chars, $AFTER after one keystroke"
    test "$AFTER" -eq "$((BEFORE + 1))"
    # Not merely LONGER — the right character, at the right place. A document
    # that grew by one is also what a stray newline looks like.
    grep -q 'WG6C typed-char-landed true' /tmp/wg6c_gate.txt
    EB=$(grep -E '^WG6C edits-before ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    EA=$(grep -E '^WG6C edits-after ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    test "$EA" -gt "$EB"

    # NAVIGATION, through a real WM_KEYDOWN. Arrows need no modifier, so this
    # is a complete proof of that route rather than a partial one.
    CT=$(grep -E '^WG6C caret-after-typing ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    CL=$(grep -E '^WG6C caret-after-left ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    echo "caret: $CT after typing, $CL after Left"
    test "$CL" -lt "$CT"

    # AND THE MODIFIER GATE REALLY GATES. A bare Z must not undo: `editorKeyDown:`
    # asks GetKeyState, the real keyboard, and a synthesised message cannot lie
    # to it. Asserted as "nothing happened" — the edit counter did not move.
    EZ=$(grep -E '^WG6C edits-after-bare-z ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    test "$EZ" -eq "$EA"

    # BACKSPACE, and the double-application it would be if `editorChar:` did not
    # decline the control range: WM_CHAR delivers 8 for this key too. A document
    # that shrank by TWO is that bug, so the assertion is exact.
    BS=$(grep -E '^WG6C doc-after-backspace ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    test "$BS" -eq "$BEFORE"

    # A CLICK PLACES THE CARET, through a real WM_LBUTTONDOWN — WG6c-1's
    # arithmetic inverted, against pixels this gate computed from the pane's own
    # metrics. Line and column rather than an offset: an offset would pass for
    # the wrong reason on a document whose lines happened to be equal length.
    grep -q 'WG6C click-line 1' /tmp/wg6c_gate.txt
    grep -q 'WG6C click-col 4' /tmp/wg6c_gate.txt

    # THE SLICE'S STATED GATE. Exact equality against the string the document
    # started as, never a length: a journal that replayed badly comes back the
    # right length most of the time.
    grep -q 'WG6C undo-restored-exactly true' /tmp/wg6c_gate.txt

    # ── WG6c-2b: the selection, and what it is for ──────────────────────
    # A DRAG IS THREE MESSAGES and needs no modifier, so unlike shift-extend
    # and Ctrl-Z this is a complete proof rather than a partial one: press,
    # move, release, and the six characters under the gesture are selected.
    grep -q "WG6C drag-selected 'Object'" /tmp/wg6c_gate.txt
    # AND IT IS VISIBLE. This project has shipped an invisible feature before —
    # WG5a D4's ghost line, whose test asserted the text a drawing WOULD use
    # and never that anything appeared — so the highlight's rectangles are
    # asserted to exist, and the snapshot at the end carries the rest.
    RECTS=$(grep -E '^WG6C drag-rects ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    test "$RECTS" -ge 1
    # AND THE MOUSE WAS RELEASED. A capture that leaks is invisible until every
    # other control in the app stops responding, which is a long way from the
    # drag that caused it — WG4 D5's recorded hazard against exactly this trio.
    grep -q 'WG6C capture-released true' /tmp/wg6c_gate.txt

    # THE CLIPBOARD IS REAL — Win32's own, global and shared, not a variable
    # pretending to be one. Round-tripped, then used: copy puts the dragged
    # selection on it, and paste over a different selection puts it back.
    grep -q 'WG6C clip-put true' /tmp/wg6c_gate.txt
    grep -q "WG6C clip-got 'ROUNDTRIP'" /tmp/wg6c_gate.txt
    grep -q "WG6C clip-error ''" /tmp/wg6c_gate.txt
    grep -q "WG6C clip-after-copy 'Object'" /tmp/wg6c_gate.txt
    grep -q "WG6C doc-after-paste 'Object'" /tmp/wg6c_gate.txt
    # ONE Ctrl-Z after a paste over a selection, not two. Delete-then-insert as
    # two commits left the first undo on a document the user never saw.
    grep -q 'WG6C undo-after-paste-restores true' /tmp/wg6c_gate.txt

    # AND IT PAINTED. Zero paints with an empty paintError is exactly what a
    # stale binary looks like (see the note above this recipe), and it is
    # indistinguishable from success on every other line of this gate.
    PC=$(grep -E '^WG6C paint-calls ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    echo "editor: $PC paints"
    test "$PC" -gt 0
    grep -q "WG6C paint-error ''" /tmp/wg6c_gate.txt

    # ── WG6c-3: the Editor VIEW ─────────────────────────────────────────
    # Reached by a real WM_COMMAND on its bar cell, like a click.
    grep -q 'WG6C view-active #editor' /tmp/wg6c_gate.txt
    grep -q 'WG6C view-picker-exists true' /tmp/wg6c_gate.txt
    grep -q 'WG6C view-save-exists true' /tmp/wg6c_gate.txt
    # The pane is laid out by the VIEW now, to the right of the picker. A pane
    # covering the picker would look like a view with no class list at all.
    grep -q 'WG6C pane-right-of-picker true' /tmp/wg6c_gate.txt

    # A CLASS ALL THE WAY ROUND, through the view's own Save cell and back
    # through its own open — both over `flows`, which is the whole point.
    grep -q 'WG6C save-message.*saved GateDemo' /tmp/wg6c_gate.txt
    grep -q 'WG6C reopened-class .GateDemo.' /tmp/wg6c_gate.txt
    grep -q 'WG6C reopened-has-method true' /tmp/wg6c_gate.txt

    # AND THE PARSE GATE REFUSES WITHOUT CHANGING ANYTHING — the sprint's own
    # phrase. `flows` reports its refusal as a summary rather than an error,
    # deliberately (the guest must not invent a status the other two callers do
    # not report), so the words are asserted here and the survival of the class
    # written a moment ago is asserted separately. A refusal that had already
    # half-written something is worse than no gate at all.
    grep -q 'WG6C refusal-message.*nothing to save' /tmp/wg6c_gate.txt
    grep -q 'WG6C survived-the-refusal true' /tmp/wg6c_gate.txt

    # THE CLASS PICKER FILLED, from the primary's own hierarchy and across the
    # seam — the half a screenshot taken too early shows as an empty box.
    # Counts as a RELATIONSHIP (WG2 Δ 14): the world grows, so what is asserted
    # is that the picker holds exactly what the snapshot holds. A list that
    # filled with a different number is the `listSet:items:` bug WG6b hit, where
    # the view showed nothing while the transcript reported five hits.
    CLASSES=$(grep -E '^WG6C2 browser-classes ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    ROWS=$(grep -E '^WG6C2 picker-rows ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    echo "editor: $ROWS picker rows over the primary's $CLASSES classes"
    test "$CLASSES" -ge 100
    test "$ROWS" = "$CLASSES"
    grep -q 'WG6C2 still-editor #editor' /tmp/wg6c_gate.txt
    grep -q "WG6C2 paint-error ''" /tmp/wg6c_gate.txt

    # A PARTIAL PAINT still gets provoked, but the origin assertion is GONE —
    # WG6d made that defect unrepresentable rather than fixing it. The renderer
    # redraws the entire grid every present and ignores rcPaint, so there is no
    # partial composition left to be wrong about. What is still worth asserting
    # is that the real WM_PAINT reached a present without error, and that
    # FRAMES CLIMBED — the renderer's replacement for `paintCalls`, and the
    # only thing that distinguishes "presented" from "quietly did nothing".
    grep -q "WG6C2 partial-paint-error ''" /tmp/wg6c_gate.txt
    FRAMES=$(grep -E '^WG6C2 frames-after-partial ' /tmp/wg6c_gate.txt | awk '{print $NF}')
    echo "editor: $FRAMES frames presented"
    test "$FRAMES" -gt 0

    test -s target/winui-wg6c.png
    for i in $(seq 1 60); do [ -f /tmp/wg6c_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg6c_exit)" = "0"

    # The scratch image was really written — otherwise every assertion above
    # could be describing a Save that never reached the disk.
    test -s "$IMG"
    rm -f "$IMG"
    # And the developer's OWN image is untouched. Not "does not exist": it
    # legitimately does once `import_world` has been run, and the GUI needs it
    # to show source at all. What must hold is that a GATE never writes it.
    if [ -f world/image.sqlite3 ]; then
        test "$(md5sum < world/image.sqlite3)" = "$OWN_IMAGE_SUM"
        echo "own image: unchanged"
    fi
    echo "gate-wg6c: PASS"

# --- WG7-3: the primary, restarted in place -------------------------------
#
# docs/sprints/sprint_wg7_detail.md WG7-3, ordered FIRST despite being listed
# third: it is the piece with real risk (thread lifetime, in-flight requests),
# and the Debugger's own gate wants to halt and resume repeatedly against a
# known world — so finding restart bugs while debugging the Debugger would be
# the worst possible order.
#
# It is also what WG6c-3 was missing. File In's contract is "a fresh world,
# then your file", and `win_gui` booted its primary once and held it for the
# process lifetime; there was no teardown to build on. That is why File In and
# Add to World were left unbuilt, and this is the machinery they need.
#
# THE CONTRACT PULLS BOTH WAYS, which is the whole reason it is gated rather
# than eyeballed: the new primary must be indistinguishable from a fresh boot
# TO THE WORLD, and the restart must be completely invisible TO THE WINDOW. A
# restart that rebuilt the views would satisfy the first and break the second.
gate-wg7:
    #!/usr/bin/env bash
    set -euo pipefail
    taskkill //F //IM macvm-winui.exe > /dev/null 2>&1 || true
    cargo build --quiet -p win_gui -p winui_host -p winui_render
    rm -f /tmp/wg7_exit /tmp/wg7_app.txt /tmp/wg7_gate.txt target/winui-wg7.png
    ( MACVM_WINUI_CTL=7715 ./target/debug/macvm-winui.exe > /tmp/wg7_app.txt 2>&1; \
        echo $? > /tmp/wg7_exit ) &
    for i in $(seq 1 90); do
        (exec 3<>/dev/tcp/127.0.0.1/7715) 2>/dev/null && break || sleep 0.5
    done
    # Four parts with bash sleeps between: the browser fills from a REPLY, and
    # `gui sleep` blocks the APP rather than only the driver — it would starve
    # the drain pass it is waiting for. winui-wg5b.tcl records the same finding.
    ./target/debug/macvm rusttcl scripts/winui-wg7-a.tcl | tee    /tmp/wg7_gate.txt
    sleep 8
    ./target/debug/macvm rusttcl scripts/winui-wg7-b.tcl | tee -a /tmp/wg7_gate.txt
    sleep 8
    ./target/debug/macvm rusttcl scripts/winui-wg7-c.tcl | tee -a /tmp/wg7_gate.txt
    sleep 8
    ./target/debug/macvm rusttcl scripts/winui-wg7-d.tcl | tee -a /tmp/wg7_gate.txt
    cat /tmp/wg7_app.txt

    grep -q 'WG7 open true' /tmp/wg7_gate.txt

    # THE WORLD IS REALLY REPLACED. A class defined at RUNTIME in the primary —
    # no file, no image, nothing a fresh boot could find — is present before the
    # restart and gone after it. Asserted as a RELATIONSHIP (WG2 Δ 14): the
    # world grows, so what matters is +1 then back, never today's total.
    BASE=$(grep -E '^WG7 classes-baseline '     /tmp/wg7_gate.txt | awk '{print $NF}')
    GHOST=$(grep -E '^WG7 classes-with-ghost '  /tmp/wg7_gate.txt | awk '{print $NF}')
    AFTER=$(grep -E '^WG7 classes-after-restart ' /tmp/wg7_gate.txt | awk '{print $NF}')
    echo "primary: $BASE classes, $GHOST with the ghost, $AFTER after the restart"
    test "$BASE" -ge 100
    test "$GHOST" -eq "$((BASE + 1))"
    test "$AFTER" -eq "$BASE"

    # AND THE WINDOW NEVER NOTICED. Same window, same views, same active view —
    # a restart is not a rebuild, and `viewBuildCount` is the one number that
    # cannot be fooled by the window merely still being there.
    grep -q 'WG7 window-alive true' /tmp/wg7_gate.txt
    BUILT_BEFORE=$(grep -E '^WG7 views-built-at-start ' /tmp/wg7_gate.txt | awk '{print $NF}')
    BUILT_AFTER=$(grep -E '^WG7 views-built-after '     /tmp/wg7_gate.txt | awk '{print $NF}')
    test "$BUILT_AFTER" -eq "$BUILT_BEFORE"
    # The old primary really STOPPED — it says so on its way out, and a restart
    # that left it running would be two primaries racing over one UI worker's
    # registry entry.
    grep -q 'primary stopping' /tmp/wg7_app.txt

    test -s target/winui-wg7.png
    for i in $(seq 1 60); do [ -f /tmp/wg7_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg7_exit)" = "0"
    echo "gate-wg7: PASS"

# docs/sprints/upstream_review_2026-08-12.md — WG8 / SM0: the pixel plane.
#
# WHY THIS GATE LEADS WITH A VIEW rather than with pixels. The first cut of this
# demo drew plasma into the EDITOR's pane, because the Editor's pane already had
# a renderer attached and it was the shortest path to a coloured rectangle. That
# was the wrong place, and being told so is what produced the Canvas. So the
# first three assertions here are about a VIEW existing — registered, switchable,
# built — and only then about what is drawn in it. A pixel plane that happens to
# work inside somebody else's pane proves nothing about whether this shell can
# host a canvas.
#
# THREE CLAIMS, and each has a failure mode a screenshot would not catch:
#
#  1. THE PLANE REACHES THE SCREEN. `stride` comes from the renderer and is the
#     one number the guest is forbidden to assume — 160 BGRA pixels is 640 bytes
#     only if D3D chose not to pad the row. Asserting it is non-zero asserts the
#     Map succeeded; asserting the guest ASKED is the standing caution of the
#     WG8 review, which is the whole discipline a pixel plane has left.
#
#  2. IT MOVES. Frames and phase both advance. A still image that was right once
#     and then froze looks identical to a working one in any single snapshot,
#     and that is exactly the bug a live buffer has.
#
#  3. THE TWO PLANES COMPOSE. The HUD's cells carry `transparentBackground`, so
#     the renderer skips their background fill and the pixels show through. The
#     constant is duplicated across an FFI boundary that carries no types; if it
#     drifts, the HUD paints a solid bar over the plane. The world suite pins the
#     value, this pins that the pane still presented with it in place.
gate-wg8:
    #!/usr/bin/env bash
    set -euo pipefail
    taskkill //F //IM macvm-winui.exe > /dev/null 2>&1 || true
    # BUILD FIRST — WG6d-2 lost an afternoon to a gate that passed against a
    # binary predating the change it was gating.
    cargo build --quiet -p win_gui -p winui_host -p winui_render
    rm -f /tmp/wg8_exit /tmp/wg8_app.txt /tmp/wg8_gate.txt \
        target/winui-wg8.png target/winui-wg8-palette.png
    ( MACVM_WINUI_CTL=7716 ./target/debug/macvm-winui.exe > /tmp/wg8_app.txt 2>&1; \
        echo $? > /tmp/wg8_exit ) &
    for i in $(seq 1 90); do
        (exec 3<>/dev/tcp/127.0.0.1/7716) 2>/dev/null && break || sleep 0.5
    done
    sleep 3
    ./target/debug/macvm rusttcl scripts/winui-wg8.tcl | tee /tmp/wg8_gate.txt
    cat /tmp/wg8_app.txt

    # 1. The VIEW exists, in its own right.
    grep -q 'WG8 open true'    /tmp/wg8_gate.txt
    grep -q 'WG8 switched true' /tmp/wg8_gate.txt
    grep -q 'WG8 active #canvas' /tmp/wg8_gate.txt
    grep -q 'WG8 built true'   /tmp/wg8_gate.txt
    HWND=$(grep -E '^WG8 pane-hwnd ' /tmp/wg8_gate.txt | awk '{print $NF}')
    test "$HWND" -gt 0

    # 2. The PLANE is real and its shape came from the renderer.
    STRIDE=$(grep -E '^WG8 stride ' /tmp/wg8_gate.txt | awk '{print $NF}')
    echo "canvas: pane $HWND, stride $STRIDE bytes"
    test "$STRIDE" -ge 640
    grep -q 'WG8 plane true' /tmp/wg8_gate.txt
    grep -q "WG8 last-error ''" /tmp/wg8_gate.txt

    # 3. It MOVES — asserted as a relationship (WG2 Δ 14), never as a total:
    #    two explicit renders must advance the counter by exactly two, whatever
    #    number of paints the window happened to do first.
    F1=$(grep -E '^WG8 frames-1 ' /tmp/wg8_gate.txt | awk '{print $NF}')
    F2=$(grep -E '^WG8 frames-2 ' /tmp/wg8_gate.txt | awk '{print $NF}')
    echo "canvas: $F1 frames, then $F2"
    test "$F2" -eq "$((F1 + 2))"
    PHASE=$(grep -E '^WG8 phase ' /tmp/wg8_gate.txt | awk '{print $NF}')
    test "$PHASE" -gt 0
    grep -q 'WG8 render-a true' /tmp/wg8_gate.txt
    grep -q 'WG8 render-b true' /tmp/wg8_gate.txt

    # 4. And the two planes composed — the sentinel still matches Rust's, and
    #    the pane presented at least as many frames as we asked it to.
    grep -q 'WG8 transparent 16777216' /tmp/wg8_gate.txt
    PF=$(grep -E '^WG8 present-frames ' /tmp/wg8_gate.txt | awk '{print $NF}')
    test "$PF" -ge "$F2"

    # 5. SM4 — THE PALETTE AS MEMORY. Two shapes, and they must differ: the
    #    index buffer is ONE byte per pixel (stride 160), the BGRA plane is four
    #    (stride 640). A guest that confused them would draw a quarter-width
    #    smear, which looks like a stride bug and is not one.
    grep -q 'WG8 mode #palette'    /tmp/wg8_gate.txt
    grep -q 'WG8 pal-render true'  /tmp/wg8_gate.txt
    grep -q 'WG8 pal-addr true'    /tmp/wg8_gate.txt
    grep -q "WG8 pal-error ''"     /tmp/wg8_gate.txt
    IXS=$(grep -E '^WG8 ix-stride ' /tmp/wg8_gate.txt | awk '{print $NF}')
    PLEN=$(grep -E '^WG8 pal-len '  /tmp/wg8_gate.txt | awk '{print $NF}')
    echo "palette: index stride $IXS, $PLEN slots"
    test "$IXS" -eq 160
    test "$IXS" -eq "$((STRIDE / 4))"
    test "$PLEN" -eq 256

    # 6. AND THE FIELD IS WRITTEN ONCE. Three palette frames, one field. This is
    #    the assertion no screenshot can make: a demo that rewrote every index
    #    every frame would look exactly the same and would be the design SM4
    #    exists to replace.
    grep -q 'WG8 pal-render-2 true' /tmp/wg8_gate.txt
    grep -q 'WG8 pal-render-3 true' /tmp/wg8_gate.txt
    grep -q 'WG8 field-once true'   /tmp/wg8_gate.txt
    PF2=$(grep -E '^WG8 pal-frames ' /tmp/wg8_gate.txt | awk '{print $NF}')
    #    AT LEAST three, not exactly three: a `gui snap` forces a WM_PAINT of
    #    its own, so the window contributes frames the driver did not ask for.
    #    The claim is that our three happened, and `-eq` would be asserting
    #    that nothing else ever repaints — which is not true and not the point.
    test "$PF2" -ge "$((F2 + 3))"

    test -s target/winui-wg8.png
    test -s target/winui-wg8-palette.png
    for i in $(seq 1 60); do [ -f /tmp/wg8_exit ] && break || sleep 0.5; done
    test "$(cat /tmp/wg8_exit)" = "0"
    echo "gate-wg8: PASS"
