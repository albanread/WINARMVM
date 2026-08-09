//! Vendored slice of JASM's `wfasm` AArch64 encoder + macOS JIT loader +
//! relocation patcher, plus a frozen-corpus replay test carrying its
//! LLVM-MC-verified trust gate into MACVM without an LLVM dependency. See
//! `VENDOR.md` for the exact file list, source commit, and per-file edits;
//! `docs/sprints/sprint_s09_detail.md` D1 for the vendoring procedure this
//! tree follows.
//!
//! `src/compiler/jasm_assembler.rs` is the only consumer outside this tree
//! (sprint_s09_detail.md D4: nothing else may `use crate::vendor::…`).

#![allow(dead_code)]
// MACVM uses a subset of the vendored surface
// Vendored/derived code (this whole tree) keeps upstream's original style
// per VENDOR.md's "minimal diff against upstream" rule — these are the
// specific clippy lints that style trips (encode.rs's bit-math OR-chains
// and wide instruction-field argument lists; a `% 2` parity check and a
// `vec![x; n]` kept from difftest.rs's own `unhex`/test code), not a
// blanket suppression for anything MACVM writes fresh under this tree.
#![allow(
    clippy::identity_op,
    clippy::too_many_arguments,
    clippy::manual_is_multiple_of,
    clippy::useless_vec
)]
pub mod a64;
pub mod backend;
#[cfg(test)]
mod corpus_replay;
#[cfg(target_os = "macos")]
pub mod native_macos;
// WINARM (P0 D2#8): the Windows-on-ARM64 sibling of `native_macos`
// (MIGRATION.md §3.1). Both are OS-gated the same way, and the file itself
// repeats its own `#![cfg]` — which is what keeps its `asm!("isb sy")` out of
// reach of any other target's parser.
#[cfg(all(windows, target_arch = "aarch64"))]
pub mod native_winarm64;
pub mod relocpatch;

// WINARM (P0 D2#8): the OS dispatch for the JIT-memory seam.
//
// MIGRATION.md §3.5's rule is "gate by capability seam, not by sprinkling":
// one sibling file selected in `mod.rs`, so that `codecache/guard.rs` and
// `codecache/mod.rs` — the only consumers — carry no `#[cfg]` of their own and
// never spell a platform. Both consumers keep the import shape they already
// had; only the module name changes (`native_macos` -> `native_jit`), which is
// the smallest diff available at those call sites.
//
// Two aliases rather than one, because the two consumers want different
// things: `guard.rs` imports the free `jit_write_protect`/`icache_invalidate`
// pair (module alias suffices), while `codecache/mod.rs` also needs the region
// type, whose honest local names differ per platform (`MacJit` vs
// `WinA64Jit`). Aliasing the type here — rather than adding a neutral alias
// inside each sibling — keeps `native_macos.rs` byte-identical to its vendored
// state, which VENDOR.md's re-vendoring diff depends on.
//
// This crate targets exactly these two platforms; on any third, both aliases
// are absent and `codecache` fails to compile with an unresolved import, which
// is the honest outcome — there is no portable fallback for "write to
// executable memory".

/// The current target's native JIT-memory module, under one name.
#[cfg(target_os = "macos")]
pub use native_macos as native_jit;
#[cfg(all(windows, target_arch = "aarch64"))]
pub use native_winarm64 as native_jit;

/// The current target's JIT region owner: `mmap(MAP_JIT)`-backed on macOS,
/// `VirtualAlloc(PAGE_EXECUTE_READWRITE)`-backed on Windows ARM64. `CodeCache`
/// uses it only for the region's lifetime + `region_raw`, never for the
/// build/load/finalize protocol `MacJit` also offers.
#[cfg(target_os = "macos")]
pub use native_macos::MacJit as NativeJit;
#[cfg(all(windows, target_arch = "aarch64"))]
pub use native_winarm64::WinA64Jit as NativeJit;
