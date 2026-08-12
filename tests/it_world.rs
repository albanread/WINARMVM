//! World + SUnit-lite integration tests (`tests_s06.md` "Integration /
//! golden tests" and "Stress / negative tests"). Most of these run
//! in-process (`Smalltalk quit:` just sets `vm.exit_code`, it never calls
//! `std::process::exit` — only the `error:` primitive does that) so only
//! the DNU/`error:` case needs a real subprocess.

mod common;

use std::path::PathBuf;
use std::process::{Command, Stdio};

use macvm::frontend::world;
use macvm::runtime::vm_state::OutputBuffer;

fn world_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("world")
}

/// Loads `world/tests/tests.list` (NOT named `world.list`, so
/// `frontend::world::load_world` doesn't apply directly) relative to
/// `world/tests/`, in order, stopping early if a file requests exit.
fn load_tests_list(vm: &mut macvm::runtime::VmState) {
    let dir = world_dir().join("tests");
    let list_src = std::fs::read_to_string(dir.join("tests.list")).expect("read tests.list");
    for raw_line in list_src.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        world::load_file(vm, &dir.join(line)).unwrap_or_else(|e| panic!("{line}: {e}"));
        if vm.exit_requested {
            break;
        }
    }
}

#[test]
fn boots_clean() {
    let mut vm = common::test_vm();
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    let loaded = world::load_world(&mut vm, &world_dir()).expect("load_world");
    assert!(loaded);
    assert_eq!(
        buf.as_string(),
        "",
        "world.list alone must produce no output"
    );
}

/// Boots the real world, runs `src`'s top-level doIts, and returns what they
/// printed to the Transcript — the end-to-end path a `.mst` file / the REPL
/// take (needs the full world: `Array`, `at:put:`, `printString`).
fn eval_world_transcript(src: &str) -> String {
    let mut vm = common::test_vm();
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    world::load_world(&mut vm, &world_dir()).expect("load_world");
    for item in macvm::frontend::parser::parse_file(src).expect("parse") {
        macvm::frontend::classdef::execute_top_item(&mut vm, item).expect("execute");
    }
    buf.as_string()
}

/// Squeak/Pharo brace (dynamic) arrays `{ e1. e2. … }`: elements are
/// runtime EXPRESSIONS (not `#( … )`'s compile-time literals), a fresh
/// `Array` per evaluation. Codegen desugars to `(Array new: n)` + `at:put:`,
/// so this rides the real world's primitives end to end — arithmetic,
/// string concat, `size`, nesting, and a genuinely runtime element (a block
/// parameter) that a literal array could not express.
#[test]
fn brace_arrays_build_runtime_arrays() {
    let out = eval_world_transcript(
        "Transcript showCr: { 1 + 1. 2 * 3. 'a', 'b' } printString.\n\
         Transcript showCr: { 10. 20. 30 } size printString.\n\
         Transcript showCr: {} printString.\n\
         Transcript showCr: { 1. { 2. 3 }. 4 } printString.\n\
         Transcript showCr: ([ :x | { x. x * x } ] value: 9) printString.\n",
    );
    assert_eq!(out, "#(2 6 'ab')\n3\n#()\n#(1 #(2 3) 4)\n#(9 81)\n");
}

/// Gate item 1 (`tests_s06.md`): boot, load world.list + tests.list, run
/// SUnit-lite, exit 0 with a `/^\d+ run, 0 failed$/` report line, total
/// assertions >= 200.
#[test]
fn suite_green() {
    let mut vm = common::test_vm();
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    world::load_world(&mut vm, &world_dir()).expect("load_world");
    load_tests_list(&mut vm);

    assert_eq!(vm.exit_code, Some(0), "stdout so far:\n{}", buf.as_string());
    let out = buf.as_string();
    let report_line = out
        .lines()
        .find(|l| l.ends_with("failed"))
        .unwrap_or_else(|| panic!("no '... failed' report line in:\n{out}"));
    assert!(
        report_line.ends_with(", 0 failed"),
        "expected 0 failures, got: {report_line}"
    );
    let run_count: u64 = report_line
        .split(' ')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("couldn't parse run count from: {report_line}"));
    assert!(
        run_count >= 200,
        "expected >= 200 assertions run, got {run_count}"
    );
}

/// WINARM (P3, `tests_p03.md` rows `compile_count_nonzero_at_threshold1`
/// and `frame_size_under_probe_limit`) — the FALSE-GREEN tripwire plus the
/// D1 audit's live half, in one world load.
///
/// Two independent things a stress gate cannot tell you on its own:
///
/// 1. **Something actually compiled.** A suite that is green because the
///    JIT never fired proves nothing about the JIT, and the P3 gate's whole
///    claim is "tier-1 is alive here". The threshold is set in Rust rather
///    than read from the ambient `MACVM_JIT` so the tripwire is a real
///    assertion in every run, not one that quietly evaporates when the gate
///    script forgets a variable.
/// 2. **No compiled frame needs a Windows stack probe.** The high-water
///    mark `codecache::nmethod::note_nmethod_frame_bytes` keeps is read back
///    after the whole world + test corpus has been compiled — the largest
///    real frame this VM produces, not a synthetic one.
///
/// **Why 20 and not 1** (`tests_p03.md` names this row "at_threshold1", and
/// this is the Δ): `VmOptions::parse_jit` REFUSES `threshold=1` from the
/// environment and substitutes `JIT_THRESHOLD_FLOOR` = 20, so the literal
/// gate command `MACVM_JIT=threshold=1 cargo test` has always run at 20 —
/// on macOS too, since that floor is upstream MACVM, not a port change. 20
/// is therefore the aggressive setting the gate actually means. Thresholds
/// 2..5 over this corpus hit a real VM defect that is NOT a port gap; see
/// [`world_suite_at_threshold_2_hits_root_block_deopt_defect`].
#[test]
fn compile_count_nonzero_at_threshold1() {
    use macvm::codecache::nmethod::{max_nmethod_frame_bytes_seen, MAX_UNPROBED_FRAME_BYTES};

    let before = max_nmethod_frame_bytes_seen();
    let mut vm = macvm::runtime::VmState::with_options(macvm::runtime::VmOptions {
        heap_mib: 64,
        eden_kb: None,
        jit: macvm::runtime::JitMode::Threshold(20),
        ..macvm::runtime::VmOptions::from_env()
    });
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    world::load_world(&mut vm, &world_dir()).expect("load_world");
    load_tests_list(&mut vm);

    assert_eq!(vm.exit_code, Some(0), "stdout so far:\n{}", buf.as_string());
    let report_line = buf
        .as_string()
        .lines()
        .find(|l| l.ends_with("failed"))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no '... failed' report line"));
    assert!(
        report_line.ends_with(", 0 failed"),
        "the world suite must be green at threshold=1 too: {report_line}"
    );

    assert!(
        vm.stats.compilations > 0,
        "threshold=1 over the whole world + test corpus compiled NOTHING — a \
         green suite that never entered the JIT is a false green (tests_p03.md \
         Pitfalls)"
    );
    eprintln!(
        "[P3 D4] world suite at threshold=20: compilations={} recompiles={} deopt_count={}",
        vm.stats.compilations, vm.stats.recompiles, vm.stats.deopt_count
    );

    let mark = max_nmethod_frame_bytes_seen();
    assert!(
        mark > before || before > 0,
        "no compiled frame size was recorded despite {} compilations",
        vm.stats.compilations
    );
    eprintln!(
        "[P3 D1] largest compiled frame over the whole corpus: {mark} bytes \
         (limit {MAX_UNPROBED_FRAME_BYTES}, headroom {}x)",
        MAX_UNPROBED_FRAME_BYTES / mark.max(1)
    );
    assert!(
        mark < MAX_UNPROBED_FRAME_BYTES,
        "largest compiled frame {mark} >= the Windows unprobed limit \
         {MAX_UNPROBED_FRAME_BYTES}: this build would need a prologue probe loop"
    );
}

/// WINARM (P3 D4) — committed as a runnable repro rather than described in
/// prose.
///
/// **FIXED (P4).** Root cause: `compiler::regalloc::regalloc`'s
/// `block_closure_vreg` pin bounded the closure's live interval by
/// `max(interval.end) + 2` — the last position at which SOME vreg is
/// defined or used — instead of by the method's actual position count. A
/// compile whose linearization ends in cold trap/fail blocks (DFS dead
/// ends, so `reverse_postorder` puts them last) has a run of trailing
/// positions past every interval end; at those positions
/// `scopes::resolve_frame_loc` found the pinned interval already expired
/// (it tests `iv.end > pos`, strictly) and returned `ValueLoc::Nil`, so
/// `driver::build_deopt_metadata` recorded the root block scope's receiver
/// as Nil. The measured failing compile: 195 IR ops (positions 0..=194,
/// safepoints out to 194) pinned only to 166. See
/// `regalloc::pin_end_bound`, and the unit regression
/// `regalloc::tests::block_closure_vreg_pin_covers_trailing_cold_block_safepoints`.
/// The threshold bound recorded below was therefore never real: whether a
/// compile is affected depends on how many trailing positions end no
/// interval — a property of the shape the profile produced, not of the
/// threshold number. Un-ignored; it now passes.
///
/// **Mainline bug, platform-dependent TRIGGER — and the distinction is
/// proven, not argued.** The bug is integer arithmetic over IR positions,
/// with no OS, register, address or exception-handling input anywhere in
/// it, and it is pinned by a pure unit test that hand-builds an `IrMethod`
/// and calls `regalloc()` directly
/// (`regalloc::tests::block_closure_vreg_pin_covers_trailing_cold_block_safepoints`)
/// — no VM execution, no trap, no host involvement; it fails under the old
/// bound on ANY host. That macOS never aborted here says only that no
/// compile in a macOS run happened to have the trailing-position shape:
/// identity hashes derive from allocation addresses, so Dictionary/Set
/// iteration, compile order and IC/profile state all differ between hosts,
/// and they decide WHICH methods compile with WHICH shape. "Passes on the
/// other host" is evidence of an absent trigger, never of an absent bug —
/// the same trap as the "passes at threshold 10" reading above.
///
/// The whole world + test corpus compiled at `threshold` 2, 3 or 5 dies
/// deterministically in `runtime::deopt.rs`'s root-block arm:
///
/// ```text
/// root-block scope's receiver ValueLoc must hold the closure
/// (driver records block_closure_vreg there)
/// ```
///
/// i.e. a STANDALONE-compiled block's deopt found something other than its
/// closure in the receiver-arg slot. That is the same family as the depth-3
/// spliced-block defect P2 filed (`it_tier1::depth3_deopt_in_block_in_
/// callee_rebuilds_all_frames`): a recorded `ValueLoc` pointing at the wrong
/// frame slot around a spilled closure.
///
/// **Why it is mainline, not the port.** `src/runtime/deopt.rs`,
/// `src/compiler/scopes.rs` and `src/compiler/regalloc.rs` are byte-identical
/// to `C:\projects\MACVM` (diffed ignoring line endings); `src/compiler/`
/// contains no `cfg(target_os)`/`cfg(windows)` at all; the failing corpus
/// file (`world/tests/49_supervisor_tests.mst`) is byte-identical too. Same
/// metadata, same nmethods, same recorded `ValueLoc`s on both hosts.
///
/// **Repro (either platform), no Rust needed:**
/// ```text
/// grep -v '^#' world/tests/tests.list | grep -v '^$' \
///   | sed 's|^|world/tests/|' | xargs cat > /tmp/macvm_world_tests.mst
/// MACVM_JIT=threshold=5 cargo run --release -- \
///   run /tmp/macvm_world_tests.mst --world world
/// ```
/// Windows ARM64 at commit 1323e1f: fails at 2, 3, 5; passes at 10, 15, 20,
/// 1000 and with the JIT off (three consecutive runs each way). Running the
/// suspect file (`49_supervisor_tests.mst`) ALONE at threshold 5 passes —
/// the site depends on cumulative profile state from the earlier corpus, so
/// a smaller repro was not reachable inside P3's budget.
///
/// `#[ignore]` on BOTH platforms, deliberately: the failure is a
/// `.expect()` inside `rt_uncommon_trap`, an `extern "C"` fn, so it is a
/// non-unwinding abort that takes the entire `it_world` binary — and every
/// other test's result — down with it. Same reasoning P2 recorded for the
/// depth-3 defect. Run it explicitly:
/// `cargo test --test it_world -- --ignored threshold_2`.
///
/// **Deliberately an OUT-OF-SPEC configuration (rule 1: never below
/// `JIT_THRESHOLD_FLOOR` = 20), and it must stay that way to be worth
/// anything.** It reaches the threshold by constructing
/// `JitMode::Threshold(2)` as a direct struct field, which is the sanctioned
/// compiler-test backdoor — `parse_jit`'s clamp only guards the env/CLI
/// surface, so nothing here is silently rewritten to 20 and this does not
/// pass vacuously. Measured: at in-spec thresholds (20/25/50) the corpus
/// never produces the triggering shape at all, with the fix or without it,
/// so an in-spec version of this test would guard nothing. Sub-floor
/// compiling is exactly what packs enough compiled blocks with cold
/// trailing traps into one run to reach the defect end-to-end; the
/// threshold-independent guard is the `regalloc` unit test named above.
/// Renamed from `..._hits_root_block_deopt_defect`: it no longer hits it.
#[test]
fn world_suite_at_sub_floor_threshold_survives_root_block_deopt() {
    let mut vm = macvm::runtime::VmState::with_options(macvm::runtime::VmOptions {
        heap_mib: 64,
        eden_kb: None,
        jit: macvm::runtime::JitMode::Threshold(2),
        ..macvm::runtime::VmOptions::from_env()
    });
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    world::load_world(&mut vm, &world_dir()).expect("load_world");
    load_tests_list(&mut vm);
    let report_line = buf
        .as_string()
        .lines()
        .find(|l| l.ends_with("failed"))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no '... failed' report line"));
    assert!(report_line.ends_with(", 0 failed"), "{report_line}");
}

/// Load-order torture: swapping files 12 (Dictionary, needs OrderedCollection
/// from 15... actually here we swap 12_string.mst and 19_printing.mst, which
/// both genuinely depend on earlier files) must fail fast with an
/// "undeclared variable" error — proves the ordering law is real.
#[test]
fn order_is_load_bearing() {
    let tmp = std::env::temp_dir().join(format!("macvm_order_torture_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok();
    // Copy the real world files, but list 19_printing.mst (which needs
    // WriteStream from 18 and OrderedCollection from 15) before them.
    for entry in std::fs::read_dir(world_dir()).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) == Some("mst") {
            std::fs::copy(entry.path(), tmp.join(entry.file_name())).unwrap();
        }
    }
    std::fs::write(
        tmp.join("world.list"),
        "19_printing.mst\n01_object.mst\n02_nil_boolean.mst\n04a_blockclosure.mst\n\
         04_transcript.mst\n03_behavior.mst\n05_magnitude.mst\n06_smallinteger.mst\n\
         07_largeinteger.mst\n08_double.mst\n10_array.mst\n09_character.mst\n\
         11_bytearray.mst\n12_string.mst\n13_symbol.mst\n14_association.mst\n\
         15_ordered.mst\n16_dictionary.mst\n17_interval.mst\n18_writestream.mst\n\
         20_system.mst\n",
    )
    .unwrap();

    let mut vm = common::test_vm();
    let err = world::load_world(&mut vm, &tmp).expect_err("swapped load order must fail");
    assert!(
        err.msg.contains("undeclared variable") || err.msg.contains("not understand"),
        "expected an undeclared-variable-shaped failure, got: {}",
        err.msg
    );

    std::fs::remove_dir_all(&tmp).ok();
}

/// Reopen misuse: declaring an existing class's superclass differently on
/// reopen must fail with a shape/superclass-mismatch error, not silently
/// succeed or corrupt the klass.
#[test]
fn reopen_misuse_wrong_superclass_fails() {
    let mut vm = common::test_vm();
    world::load_world(&mut vm, &world_dir()).expect("load_world");
    let items = macvm::frontend::parser::parse_file("Object subclass: Integer [ ]\n").unwrap();
    let mut saw_error = false;
    for item in items {
        if let Err(e) = macvm::frontend::classdef::execute_top_item(&mut vm, item) {
            saw_error = true;
            assert!(
                e.msg.contains("shape") || e.msg.contains("superclass"),
                "expected a shape/superclass-mismatch message, got: {}",
                e.msg
            );
        }
    }
    assert!(
        saw_error,
        "reopening Integer under the wrong superclass must fail"
    );
}

/// A deliberately-failing test class proves the suite can actually fail
/// (guards against a vacuously-green runner) — exit code 1, failure line
/// names the test.
#[test]
fn deliberately_failing_assertion_fails_the_run() {
    let mut vm = common::test_vm();
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    // WG9: SUnit moved into the world (`world/85_sunit.mst`) and is loaded by
    // `load_world` above. The explicit `00_sunit.mst` load that used to sit
    // here is gone with it — a second copy would shadow the first with an
    // identical class and hide any drift between them.
    world::load_world(&mut vm, &world_dir()).expect("load_world");

    let src = "TestCase subclass: DeliberatelyFailingTests [\n\
        runAll [ self runTest: #testBoom do: [ self testBoom ] ]\n\
        testBoom [ self assert: 1 = 2 ]\n\
    ]\n\
    TestRunner start.\n\
    TestRunner run: DeliberatelyFailingTests.\n\
    TestRunner report.\n";
    let items = macvm::frontend::parser::parse_file(src).unwrap();
    for item in items {
        macvm::frontend::classdef::execute_top_item(&mut vm, item).expect("execute");
        if vm.exit_requested {
            break;
        }
    }
    assert_eq!(vm.exit_code, Some(1));
    let out = buf.as_string();
    assert!(
        out.contains("testBoom"),
        "failure line must name the failing test, got:\n{out}"
    );
}

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_macvm"))
}

/// `error:`'s primitive calls `std::process::exit` directly — the only
/// case in this file that genuinely needs a subprocess rather than
/// in-process `VmState` inspection.
#[test]
fn error_kills_with_trace() {
    let dir = std::env::temp_dir().join(format!("macvm_error_trace_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("boom.mst");
    std::fs::write(&script, "nil foo.\n").unwrap();

    let out = Command::new(bin_path())
        .args([
            "run",
            script.to_str().unwrap(),
            "--world",
            world_dir().to_str().unwrap(),
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn macvm");

    assert_ne!(out.status.code().unwrap_or(-1), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("foo"),
        "expected the selector name in the error, got stdout:\n{stdout}"
    );
    assert!(
        stdout.lines().count() >= 2,
        "expected the message line plus >=1 stack-trace line, got:\n{stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Regression: raising an error from a frame that was entered from COMPILED
/// code must not crash the VM's own error reporter. `print_stack_trace` chases
/// `saved_fp` frame-to-frame; at the interpreter/compiled boundary that slot
/// isn't a valid interpreter word, and the old panicking readers aborted the
/// whole process (`Frame::saved_fp: not a smi`, SIGABRT — a GUI bug report). A
/// clean guest error exits with a normal code; a SIGABRT is a SIGNAL kill, so
/// `status.code()` is `None` — that (not the exit VALUE) is what this asserts.
#[test]
fn error_from_compiled_frame_does_not_abort() {
    let dir = std::env::temp_dir().join(format!("macvm_compiled_err_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("boom.mst");
    // `work:` is called ~100x so it tiers up; at i>40 it raises from inside the
    // now-compiled frame — the exact shape of the reported crash.
    std::fs::write(
        &script,
        "Object subclass: BoomC [\n\
         \x20 BoomC class >> work: i [ i > 40 ifTrue: [ ^self error: 'boom' ]. ^i + 1 ]\n\
         \x20 BoomC class >> run [ | s | s := 0. 1 to: 100 do: [:i | s := s + (self work: i) ]. ^s ]\n\
         ]\n\
         BoomC run.\n",
    )
    .unwrap();

    let out = Command::new(bin_path())
        .args([
            "run",
            script.to_str().unwrap(),
            "--world",
            world_dir().to_str().unwrap(),
        ])
        .env("MACVM_JIT", "threshold=10")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn macvm");

    // The crash was a SIGABRT → killed by a signal → `code()` is None. A clean
    // guest error exits with an actual code. So: it must have exited normally.
    assert!(
        out.status.code().is_some(),
        "VM was killed by a signal (the print_stack_trace abort), status: {:?}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("not a smi"),
        "the error reporter panicked, stderr:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("boom"),
        "expected the error message in stdout, got:\n{stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// S24 A1 (adversarial-review BLOCKER 2): a compiled NLR block whose home
/// frame already returned must deliver `#cannotReturn:` to the ORIGINATING
/// closure — `rt_nlr_originate` parks it in `NlrState.closure`; the block's
/// own native frame is gone by liveness-check time, so reading the current
/// frame's receiver slot would name the value-sender's receiver instead.
/// The world defines no `cannotReturn:`, so the correct observable is the
/// fatal DNU cascade NAMING the closure's class — identically under both
/// tiers. (Repro ledger: tests/repros/closure_dead_home_cannot_return.mst.)
#[test]
fn dead_home_nlr_names_the_closure_both_tiers() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/repros/closure_dead_home_cannot_return.mst");
    for jit in ["off", "threshold=1"] {
        let out = Command::new(bin_path())
            .args([
                "run",
                script.to_str().unwrap(),
                "--world",
                world_dir().to_str().unwrap(),
            ])
            .env("MACVM_JIT", jit)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn macvm");

        assert_ne!(
            out.status.code().unwrap_or(-1),
            0,
            "dead-home NLR must be fatal (MACVM_JIT={jit})"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("invoking"),
            "the doit must reach the invocation first (MACVM_JIT={jit}), got:\n{stdout}"
        );
        assert!(
            stdout.contains("cannotReturn:") && stdout.contains("BlockClosure"),
            "expected the DNU to name #cannotReturn: on the closure's class \
             (MACVM_JIT={jit}), got:\n{stdout}"
        );
        assert!(
            !stdout.contains("unreachable"),
            "execution must not continue past the dead-home NLR (MACVM_JIT={jit})"
        );
    }
}
