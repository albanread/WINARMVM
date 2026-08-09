//! MACVM — a research virtual machine in the Self → Strongtalk lineage,
//! implemented in Rust and targeting macOS on Apple Silicon (arm64).
//!
//! See `docs/SPEC.md` for the full engineering specification and
//! `docs/DESIGN.md` for the high-level architecture. The native code
//! generator is deliberately abstract (see [`compiler::assembler`]) so the
//! backend choice — JASM AArch64 encoder, LLVM, or interpreter-first — can be
//! made later without disturbing the rest of the VM.
//!
//! `unsafe` is confined to a small set of modules (object memory, codegen);
//! everywhere else it is denied at the crate root (CONVENTIONS §1).

#![deny(unsafe_code)]

pub mod bundle; // .app payload self-bootstrap (both GUI shells)
pub mod bytecode; // opcode set, CompiledMethod, builder, disassembler
pub mod codecache; // nmethod code cache: CodeCache, CodeHandle, JitWriteGuard
pub mod compiler; // adaptive optimizing compiler + abstract codegen
pub mod embed; // VmHandle embedding API (SPEC §16.2): boot/eval/set_transcript for GUI/library callers
pub mod frontend; // lexer, parser, AST, capture analysis, codegen, class loader
pub mod interpreter; // baseline threaded-code interpreter
pub mod memory; // object memory, allocation, garbage collection
pub mod oops; // object references, tagging, 2-word headers, classes
pub mod runtime; // stacks, activation frames, method lookup, inline caches, primitives
pub mod rusttcl; // live VM-introspection shell (disasm/methods/nmethods/ic/stats/trace), built on vendored rust-tcl
pub mod types; // optional Strongtalk-style type checker (docs/typechecker_design.md) — advisory, off the run path; reachable ONLY from the `macvm typecheck` subcommand, never from interpreter/compiler/JIT/GC/world boot
pub mod utils; // shared utilities
pub mod vendor; // vendored third-party source (S9: JASM's wfasm AArch64 encoder; rust-tcl)

// ── WINARM (P0 D5, actually landed in P3) — the emulation tripwire ─────────
//
// Δ against MIGRATION.md §8: P0's status entry says the startup banner
// reports the architecture, and `tests_p00.md` lists an
// `arch_assert_native_arm64` unit test. Neither existed in the tree — the
// PE-machine-type check P0 really did was performed by hand, once, outside
// the program. P3 needs it for real: `sprint_p03_detail.md`'s Pitfalls make
// "the P0 arch assert is required passing in the same process that produces
// PERF.md numbers" a condition of the benchmark table, and an x64-emulated
// build would produce numbers that look plausible and mean nothing.
//
// Windows 11 on ARM64 runs x86-64 binaries transparently under its
// translation layer, so "it ran" proves nothing about what it ran as. The
// check is a build fact, not a runtime probe, and that is the point: a
// binary compiled for `x86_64-pc-windows-msvc` has
// `cfg!(target_arch = "aarch64") == false` and cannot lie about it, whereas
// anything it could ask the OS at runtime would be answered by the
// translator. `GetNativeSystemInfo`/`IsWow64Process2` would add a second,
// weaker opinion and a Win32 declaration; this needs neither.

/// The architecture this binary was COMPILED for, as a stable token.
pub fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "other"
    }
}

/// The OS this binary was compiled for.
pub fn host_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "other"
    }
}

/// Panics if this is a non-ARM64 build on Windows — i.e. a build that would
/// run under x64 emulation on this host and quietly benchmark the
/// translator. Called once from `main`, so EVERY `macvm run` that produces a
/// PERF.md number has passed it. Free at runtime (both branches are
/// compile-time constants) and deliberately NOT `debug_assert!`: the
/// benchmark numbers come from a release build.
pub fn assert_native_host() {
    assert!(
        !(cfg!(windows) && !cfg!(target_arch = "aarch64")),
        "macvm: this is a {} build on Windows. On a Windows-on-ARM64 host it \
         runs under the x64 translation layer, so every timing it produces \
         measures emulation (MIGRATION.md §6 risk table). Build with \
         --target aarch64-pc-windows-msvc.",
        host_arch()
    );
}

#[cfg(test)]
mod arch_tests {
    /// `tests_p00.md`'s `arch_assert_native_arm64`, finally written.
    #[test]
    fn arch_assert_native_arm64() {
        super::assert_native_host();
        if cfg!(windows) {
            assert_eq!(
                super::host_arch(),
                "aarch64",
                "the Windows port targets ARM64 only; an x86_64 build here is \
                 the emulation trap P0's risk table names"
            );
            assert_eq!(super::host_os(), "windows");
        }
    }
}
