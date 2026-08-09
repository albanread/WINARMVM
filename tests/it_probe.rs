//! DBG0 gate (docs/DEBUGGER.md §6): the PROBE crash dossier, driven
//! through real subprocesses — the only way to observe another process's
//! exit code, death-by-signal status, and exhaustive stderr (`main.rs`'s
//! own selftest rationale). Every assertion targets the dossier MARKER +
//! section shape, never the bare exit code: exit(70) is shared with heap
//! exhaustion and process-stack overflow.

// WINARM (P0 D3 → **P2, un-gated**): every test in this file drives the VM
// into a real fault — a `brk` assert trap, a fault inside the code cache, a
// foreign fault — and then reads the dossier PROBE prints on the way down.
// Until P2 none of those paths existed on Windows (no handler was armed at
// all), so the whole file carried a `#![cfg(target_os = "macos")]` and
// compiled to an empty test binary there.
//
// P2's VEH (MIGRATION.md §3.2) supplies all three, and `capture_regs_win`
// feeds the SAME `CAPTURED` array / `CapturedRegs` the macOS handler does, so
// the dossier itself — including the `disasm_a64` window — is shared code
// rather than a twin. What differs, and all this file needed, is the vocabulary
// a fault is described in: an NT status code where POSIX has a signal number,
// so a genuinely-foreign crash dies by unhandled exception rather than by
// re-raised SIGSEGV, and `trigger:` says `ACCESS_VIOLATION` where macOS says
// `SIGSEGV`. Each assertion below names its platform's spelling explicitly
// rather than loosening to a substring both happen to contain — the point of a
// dossier test is that the dossier says the right thing.

use std::process::Command;

/// `(exit code, signal, stderr)`. `signal` is always `None` on Windows: a
/// process there is never "killed by a signal", it exits with the unhandled
/// exception's own status code (0xC0000005 for an access violation), which
/// `code()` reports — so the two platforms answer the same question
/// ("did the default disposition kill it, or did PROBE exit cleanly?")
/// through different fields.
fn run_selftest(flag: &str, extra_env: &[(&str, &str)]) -> (Option<i32>, Option<i32>, String) {
    let exe = env!("CARGO_BIN_EXE_macvm");
    let mut cmd = Command::new(exe);
    cmd.arg(flag);
    // Inherited stress/config vars can invalidate a selftest's premise
    // (the it_memory.rs precedent).
    cmd.env_remove("MACVM_GC_STRESS");
    cmd.env_remove("MACVM_JIT");
    cmd.env_remove("MACVM_PROBE");
    cmd.env_remove("MACVM_PROBE_DUMP");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn macvm");
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        out.status.signal()
    };
    #[cfg(windows)]
    let signal: Option<i32> = None;
    (
        out.status.code(),
        signal,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The `trigger:` line a fault whose pc is inside the code cache produces —
/// the platform's own name for it, not a lowest common denominator. POSIX
/// reports the signal; Windows reports the `EXCEPTION_RECORD` status code's
/// name (`codecache::deopt_trap::fault_name`'s vocabulary), because that is
/// what a reader will be looking up.
#[cfg(unix)]
const IN_CACHE_FAULT_TRIGGER: &str = "trigger: SIGSEGV (pc in code cache)";
#[cfg(windows)]
const IN_CACHE_FAULT_TRIGGER: &str = "trigger: ACCESS_VIOLATION (pc in code cache)";

#[test]
fn probe_assert_emits_full_dossier_and_exits_70() {
    let (code, _sig, stderr) = run_selftest("--selftest-probe-assert", &[]);
    assert_eq!(code, Some(70), "stderr: {stderr}");
    assert!(
        stderr.contains("==== MACVM PROBE DOSSIER v1 ===="),
        "missing dossier marker: {stderr}"
    );
    assert!(stderr.contains("trigger: brk #0xDE02"), "stderr: {stderr}");
    // Section shape: verdict, registers, walkback, heap verify, ring, end.
    for section in [
        "[1] verdict:",
        "[3] x0 ",
        "[6] native walk:",
        "[7] heap verify:",
        "[9] recent history:",
        "==== END DOSSIER (exit 70) ====",
    ] {
        assert!(stderr.contains(section), "missing {section:?} in: {stderr}");
    }
    // The receiver (nil) travels in x0 through the call stub — the register
    // annotator must see a plausible heap oop, proving the capture is the
    // REAL register file, not zeros.
    assert!(
        stderr.contains("[3] x0 ") && stderr.contains("mark:ok"),
        "x0 should annotate as a plausible mem-oop: {stderr}"
    );
}

#[test]
fn probe_segv_in_cache_emits_dossier_and_exits_70() {
    let (code, _sig, stderr) = run_selftest("--selftest-probe-segv", &[]);
    assert_eq!(code, Some(70), "stderr: {stderr}");
    assert!(stderr.contains(IN_CACHE_FAULT_TRIGGER), "stderr: {stderr}");
    assert!(
        stderr.contains("==== END DOSSIER (exit 70) ===="),
        "dossier must run to completion: {stderr}"
    );
}

#[test]
fn probe_foreign_fault_prints_verdict_and_dies_by_signal() {
    let (code, signal, stderr) = run_selftest("--selftest-probe-foreign", &[]);
    // Killed by the default disposition, NOT a dossier exit.
    assert_ne!(
        code,
        Some(70),
        "foreign crashes must not fake a dossier exit"
    );
    // How "killed by the default disposition" is observable is the one
    // genuinely platform-shaped thing here: macOS re-raises SIG_DFL and the
    // process dies BY SIGNAL; the Windows VEH returns
    // EXCEPTION_CONTINUE_SEARCH and the process dies with the unhandled
    // exception's own status as its exit code.
    #[cfg(unix)]
    assert_eq!(signal, Some(libc_sigsegv()), "stderr: {stderr}");
    #[cfg(windows)]
    {
        let _ = signal;
        assert_eq!(
            code.map(|c| c as u32),
            Some(0xC000_0005),
            "an unrecovered foreign fault must die with STATUS_ACCESS_VIOLATION \
             as its exit code — stderr: {stderr}"
        );
    }
    assert!(
        stderr.contains("FOREIGN (not in any code cache)"),
        "missing foreign verdict: {stderr}"
    );
    assert!(
        !stderr.contains("==== MACVM PROBE DOSSIER"),
        "foreign crashes must not emit a dossier (x28 untrustworthy): {stderr}"
    );
}

#[test]
fn probe_json_dump_written_with_pinned_schema() {
    let dir = std::env::temp_dir().join(format!("macvm_probe_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dossier.json");
    let (code, _sig, stderr) = run_selftest(
        "--selftest-probe-assert",
        &[("MACVM_PROBE_DUMP", path.to_str().unwrap())],
    );
    assert_eq!(code, Some(70), "stderr: {stderr}");
    let json = std::fs::read_to_string(&path).expect("json dossier written");
    assert!(
        json.starts_with("{\"schema\": 1"),
        "schema field must be pinned first: {json}"
    );
    assert!(json.contains("\"verdict\""), "json: {json}");
    assert!(json.contains("\"registers\""), "json: {json}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
fn libc_sigsegv() -> i32 {
    11
}
