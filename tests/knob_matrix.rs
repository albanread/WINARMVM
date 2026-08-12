//! WG3 knob matrix for the sub-floor world-suite failure
//! (`it_world::world_suite_at_sub_floor_threshold_survives_root_block_deopt`).
//!
//! Runs EXACTLY what that test runs — `load_world` + every file in
//! `world/tests/tests.list`, one file at a time (it_world.rs:22-35) — with the
//! VM knobs driven from the environment so a shell loop can sweep them, and
//! with the report line, the failure lines, and the JIT/GC counters printed on
//! one machine-readable line per run.
//!
//! Knobs (all optional):
//!   PROBE_THRESHOLDS=2,3,5       comma list; each entry `off` or a number
//!   PROBE_HEAP=64                heap_mib (default 64 — what it_world uses)
//!   PROBE_EDEN=1024              eden_kb (default: the VM's own default)
//!   PROBE_REPS=5                 repeat each config N times in ONE process
//!   PROBE_WORLD=<dir>            alternate world dir (default `<crate>/world`)
//!   MACVM_GC_STRESS=1|full[:N]   read through `VmOptions::from_env()` below
//!
//! Kept out of the default test run's way by name only — it is a probe, not a
//! gate. `cargo test --release --test knob_matrix -- --nocapture`.
mod common;
use macvm::frontend::world;
use macvm::runtime::vm_state::OutputBuffer;

fn world_dir() -> std::path::PathBuf {
    match std::env::var("PROBE_WORLD") {
        Ok(d) => std::path::PathBuf::from(d),
        Err(_) => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("world"),
    }
}

/// Byte-for-byte the loader `it_world::load_tests_list` uses.
fn load_tests_list(vm: &mut macvm::runtime::VmState, dir: &std::path::Path) {
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

struct Run {
    report: String,
    detail: Vec<String>,
    compilations: u64,
    recompiles: u64,
    deopts: u64,
    osr: u64,
    scavenges: u64,
    full_gcs: u64,
}

fn one_run(jit: macvm::runtime::JitMode, heap_mib: usize, eden_kb: Option<usize>) -> Run {
    let mut vm = macvm::runtime::VmState::with_options(macvm::runtime::VmOptions {
        heap_mib,
        eden_kb,
        jit,
        ..macvm::runtime::VmOptions::from_env()
    });
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    world::load_world(&mut vm, &world_dir()).expect("load_world");
    load_tests_list(&mut vm, &world_dir().join("tests"));
    let out = buf.as_string();
    let report = out
        .lines()
        .find(|l| l.ends_with("failed"))
        .unwrap_or("(NO REPORT LINE)")
        .to_string();
    let detail: Vec<String> = out
        .lines()
        .skip_while(|l| !l.ends_with("failed"))
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .map(str::to_string)
        .collect();
    Run {
        report,
        detail,
        compilations: vm.stats.compilations,
        recompiles: vm.stats.recompiles,
        deopts: vm.stats.deopt_count,
        osr: vm.stats.osr_entries,
        scavenges: vm.universe.gc_stats.scavenge_count,
        full_gcs: vm.universe.gc_stats.full_gc_count,
    }
}

#[test]
fn knob_matrix() {
    let thresholds = std::env::var("PROBE_THRESHOLDS").unwrap_or_else(|_| "2".into());
    let heap: usize = std::env::var("PROBE_HEAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let eden: Option<usize> = std::env::var("PROBE_EDEN").ok().and_then(|s| s.parse().ok());
    let reps: usize = std::env::var("PROBE_REPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let stress = std::env::var("MACVM_GC_STRESS").unwrap_or_else(|_| "-".into());

    for spec in thresholds.split(',').filter(|s| !s.trim().is_empty()) {
        let spec = spec.trim();
        let jit = if spec == "off" {
            macvm::runtime::JitMode::Off
        } else {
            macvm::runtime::JitMode::Threshold(spec.parse().expect("threshold"))
        };
        for rep in 0..reps {
            let r = one_run(jit, heap, eden);
            println!(
                "KNOB t={spec} heap={heap} eden={eden:?} stress={stress} rep={rep} \
                 | {} | comp={} recomp={} deopt={} osr={} scav={} fullgc={} | {}",
                r.report,
                r.compilations,
                r.recompiles,
                r.deopts,
                r.osr,
                r.scavenges,
                r.full_gcs,
                r.detail.join(" ;; ")
            );
        }
    }
}
