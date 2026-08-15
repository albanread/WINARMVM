//! Temporary probe: does the WG10 Life shape miscompile at Threshold(2)?
mod common;
use macvm::frontend::world;
use macvm::runtime::vm_state::OutputBuffer;
use std::collections::HashSet;

fn world_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("world")
}

fn scratch() -> std::path::PathBuf {
    std::path::PathBuf::from(
        r"C:\Users\alban\AppData\Local\Temp\claude\C--projects-WINARM\ac9fb749-7d6e-4fce-9ce7-3e5bc2a8b041\scratchpad",
    )
}

#[test]
#[ignore]
fn subfloor_probe() {
    for t in [2u32, 3, 5, 10, 20] {
        let mut vm = macvm::runtime::VmState::with_options(macvm::runtime::VmOptions {
            heap_mib: 64,
            eden_kb: None,
            jit: macvm::runtime::JitMode::Threshold(t),
            ..macvm::runtime::VmOptions::from_env()
        });
        let buf = OutputBuffer::new();
        vm.out = Box::new(buf.clone());
        world::load_world(&mut vm, &world_dir()).expect("load_world");
        let p = scratch().join("life_repro.mst");
        world::load_file(&mut vm, &p).expect("load repro");
        let out = buf.as_string();
        let line = out
            .lines()
            .find(|l| l.starts_with("nils:"))
            .unwrap_or("(no line)");
        println!("threshold={t}: {line}");
    }
}

/// FULL-SUITE probe. Same load order and same `VmOptions` as
/// `it_world::world_suite_at_sub_floor_threshold_survives_root_block_deopt`,
/// except that one corpus file may be swapped for an instrumented copy in the
/// scratchpad (`MACVM_PROBE_SUBST=<name>=<path>`), so a Transcript dump can be
/// added to the failing test WITHOUT editing the corpus.
#[test]
#[ignore]
fn subfloor_suite_probe() {
    let t: u32 = std::env::var("MACVM_PROBE_T")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let subst: Option<(String, std::path::PathBuf)> =
        std::env::var("MACVM_PROBE_SUBST").ok().map(|s| {
            let (name, path) = s.split_once('=').expect("name=path");
            (name.to_string(), std::path::PathBuf::from(path))
        });
    let mut vm = macvm::runtime::VmState::with_options(macvm::runtime::VmOptions {
        heap_mib: 64,
        eden_kb: None,
        jit: macvm::runtime::JitMode::Threshold(t),
        ..macvm::runtime::VmOptions::from_env()
    });
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    world::load_world(&mut vm, &world_dir()).expect("load_world");
    let dir = world_dir().join("tests");
    let list_src = std::fs::read_to_string(dir.join("tests.list")).expect("read tests.list");
    for raw_line in list_src.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = match &subst {
            Some((name, p)) if name == line => p.clone(),
            _ => dir.join(line),
        };
        world::load_file(&mut vm, &path).unwrap_or_else(|e| panic!("{line}: {e}"));
        if vm.exit_requested {
            break;
        }
    }
    let out = buf.as_string();
    println!("--- threshold={t} ---");
    for l in out.lines() {
        if l.starts_with("DBG") || l.ends_with("failed") || l.contains("expected") {
            println!("{l}");
        }
    }
}

// ───────────────────────── bisection harness ─────────────────────────
//
// Everything below is driven from the environment so a bisection needs no
// recompile:
//
//   MACVM_PROBE_T=<n>       JIT threshold (direct VmOptions — bypasses the
//                           JIT_THRESHOLD_FLOOR clamp `parse_jit` applies to
//                           the env/CLI surface). Default 2.
//   MACVM_PROBE_KEEP=a,b,c  load ONLY these tests.list entries (the `../`
//                           winui layer files are always kept: they are
//                           compile-time prerequisites, an unresolved
//                           capitalised name is a compile error here).
//   MACVM_PROBE_FROM=<name> load only entries at/after <name>.
//   MACVM_PROBE_DROP=a,b,c  load everything except these entries.
//   MACVM_PROBE_RUNONLY=1   load the selected files but RUN only
//                           WinUiEditorWg6cTests — separates "file loaded"
//                           from "its tests executed".
//   MACVM_PROBE_SUBST=n=p   as above: swap one corpus file for a copy.
//
// 99_run_all.mst is never loaded here; the driver is synthesised from its own
// lines, in its own order, minus the `TestRunner run: X` lines whose class was
// not loaded.

fn tests_dir() -> std::path::PathBuf {
    world_dir().join("tests")
}

fn tests_list_entries() -> Vec<String> {
    let src = std::fs::read_to_string(tests_dir().join("tests.list")).expect("read tests.list");
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// `TestCase subclass: Foo [` → "Foo", for one corpus file.
fn classes_defined_in(path: &std::path::Path) -> Vec<String> {
    let src = std::fs::read_to_string(path).unwrap_or_default();
    src.lines()
        .filter_map(|l| l.trim().strip_prefix("TestCase subclass: "))
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn env_set(key: &str) -> Option<HashSet<String>> {
    std::env::var(key)
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
}

#[test]
#[ignore]
fn subfloor_bisect() {
    let t: u32 = std::env::var("MACVM_PROBE_T")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let from = std::env::var("MACVM_PROBE_FROM").ok();
    let keep = env_set("MACVM_PROBE_KEEP");
    let drop = env_set("MACVM_PROBE_DROP");
    let run_only_editor = std::env::var("MACVM_PROBE_RUNONLY").is_ok();
    let subst: Option<(String, std::path::PathBuf)> =
        std::env::var("MACVM_PROBE_SUBST").ok().map(|s| {
            let (name, path) = s.split_once('=').expect("name=path");
            (name.to_string(), std::path::PathBuf::from(path))
        });

    let mut selected: Vec<String> = Vec::new();
    let mut past_from = from.is_none();
    for e in tests_list_entries() {
        if e == "99_run_all.mst" {
            continue; // synthesised below
        }
        if e.starts_with("../") {
            selected.push(e); // layer file: compile-time prerequisite
            continue;
        }
        if let Some(f) = &from {
            if &e == f {
                past_from = true;
            }
        }
        let want = if let Some(k) = &keep {
            k.contains(&e)
        } else if let Some(d) = &drop {
            !d.contains(&e)
        } else {
            past_from
        };
        if want || e == "67_winui_editor_tests.mst" {
            selected.push(e);
        }
    }

    let mut vm = macvm::runtime::VmState::with_options(macvm::runtime::VmOptions {
        heap_mib: 64,
        eden_kb: None,
        jit: macvm::runtime::JitMode::Threshold(t),
        ..macvm::runtime::VmOptions::from_env()
    });
    let buf = OutputBuffer::new();
    vm.out = Box::new(buf.clone());
    world::load_world(&mut vm, &world_dir()).expect("load_world");

    let mut loaded_classes: HashSet<String> = HashSet::new();
    for e in &selected {
        let path = match &subst {
            Some((name, p)) if name == e => p.clone(),
            _ => tests_dir().join(e),
        };
        for c in classes_defined_in(&path) {
            loaded_classes.insert(c);
        }
        world::load_file(&mut vm, &path).unwrap_or_else(|err| panic!("{e}: {err}"));
        if vm.exit_requested {
            break;
        }
    }

    let run_all = std::fs::read_to_string(tests_dir().join("99_run_all.mst")).unwrap();
    let mut src = String::from("TestRunner start.\n");
    let mut ran: Vec<String> = Vec::new();
    for line in run_all.lines() {
        let l = line.trim();
        if !l.contains("TestRunner run: ") {
            continue;
        }
        let cls: String = l
            .split("TestRunner run: ")
            .nth(1)
            .unwrap()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !loaded_classes.contains(&cls) {
            continue;
        }
        if run_only_editor && cls != "WinUiEditorWg6cTests" {
            continue;
        }
        src.push_str(l);
        src.push('\n');
        ran.push(cls);
    }
    src.push_str("TestRunner report.\n");
    for item in macvm::frontend::parser::parse_file(&src).expect("parse driver") {
        macvm::frontend::classdef::execute_top_item(&mut vm, item).expect("execute driver");
        if vm.exit_requested {
            break;
        }
    }

    let out = buf.as_string();
    let report_line = out
        .lines()
        .find(|l| l.ends_with("failed"))
        .unwrap_or("(no report line)");
    println!(
        "=== BISECT threshold={t} files={} classesRun={} ===",
        selected.len(),
        ran.len()
    );
    println!("REPORT: {report_line}");
    for l in out
        .lines()
        .skip_while(|l| !l.ends_with("failed"))
        .skip(1)
        .filter(|l| !l.trim().is_empty())
    {
        println!("FAIL:   {l}");
    }
    for l in out.lines().filter(|l| l.starts_with("DBG")) {
        println!("{l}");
    }
    println!(
        "STATS:  compilations={} recompiles={} deopts={}",
        vm.stats.compilations, vm.stats.recompiles, vm.stats.deopt_count
    );
    println!(
        "FILES:  {}",
        selected
            .iter()
            .filter(|e| !e.starts_with("../"))
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    );
}
