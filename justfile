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
    cat world/90_winui_probe.mst > /tmp/macvm_wg0_probe.mst
    echo 'WinProbe report.' >> /tmp/macvm_wg0_probe.mst
    ./target/debug/macvm run /tmp/macvm_wg0_probe.mst --world world | tee /tmp/wg0_ladder.txt
    grep -q 'IsWindow = false' /tmp/wg0_ladder.txt
    # gate item 4: world.list is untouched — the base world stays
    # byte-identical, which is what "winui.list is additive" means.
    git diff --quiet -- world/world.list

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
