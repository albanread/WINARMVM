// WINARM (P0 D2#8): Windows-on-ARM64 JIT memory primitives — a NEW sibling in
// the MACVM family, not a vendored file.
//
// PROVENANCE (deliberately NOT the header its neighbours carry): every other
// file under `src/vendor/wfasm/` opens with "Vendored from JASM (wfasm) …
// commit 5ac1b1b5…" and is listed in this directory's `VENDOR.md`. This file
// is in neither, because it has no upstream: JASM has no Windows-ARM64
// backend to vendor. It was written for WINARM against MIGRATION.md §3.1 and
// mirrors the two siblings that DO exist:
//
//   * `native_macos.rs` (this tree) — the *AArch64* sibling. It defines the
//     exact surface `codecache` consumes: `MacJit::with_capacity` /
//     `region_raw`, and the free `jit_write_protect` / `icache_invalidate`
//     pair MACVM added at the bottom of it (sprint_s09_detail.md D1.2).
//     Those two free-function signatures are reproduced here CHARACTER FOR
//     CHARACTER — that match is the whole reason `codecache/guard.rs` can
//     stay platform-agnostic, and a mismatch here would be a design error,
//     not a typo (sprint_p01_detail.md's last pitfall).
//   * `C:\projects\WINVM\src\vendor\wfasm\native_windows.rs` — the *Windows*
//     sibling, x86-64. Taken from it: the Win32 discipline (kernel32
//     declared by hand as `extern "system"`, constants spelled out, no
//     `windows-sys`/`windows` dependency in the core crate — P0's dependency
//     delta is zero). NOT taken from it: anything x86-64 (rel32 relocation,
//     `movabs` stubs) and its near-image `VirtualAlloc2` placement, which
//     MIGRATION.md §3.1 records as a post-P3 perf option rather than a need
//     here (A64 `bl` reaches ±128 MB inside the one region, and the vendored
//     veneer path already covers far targets).
//
// It lives under `vendor/wfasm/` in spite of not being vendored because that
// is where the OS-memory seam lives: `codecache/mod.rs`'s module doc pins
// this tree plus `codecache` as the only places JIT pages may be touched.

//! Windows 11 on ARM64 — the OS half of the code cache (MIGRATION.md §3.1):
//! one executable region, the W^X bracket [`crate::codecache::guard::
//! JitWriteGuard`] opens and closes around every write, and the
//! instruction-cache invalidation that makes freshly written A64 words
//! actually execute.
//!
//! Read against the macOS sibling, this target is macOS-shaped in exactly one
//! respect and x86-64-Windows-shaped in every other:
//!
//! | | macOS / Apple Silicon | Windows on ARM64 (here) |
//! |---|---|---|
//! | region | `mmap(MAP_JIT)`, RWX for its lifetime | `VirtualAlloc(PAGE_EXECUTE_READWRITE)`, RWX for its lifetime |
//! | write window | per-thread `pthread_jit_write_protect_np` toggle | **nothing exists** — no `MAP_JIT`, no per-thread toggle |
//! | icache | `sys_icache_invalidate` — clean + invalidate bundled | `FlushInstructionCache` **and** a local `isb sy` — two explicit halves |
//! | page size | 16 KiB, hardcoded as `PAGE = 0x4000` | 4 KiB, **queried** from `GetSystemInfo` (P0 D4 audits `0x4000` out of the Windows path) |
//! | I/D caches | split — invalidation mandatory | split — invalidation mandatory (**this** is the macOS-shaped part) |
//!
//! Reduced protection, stated plainly (sprint_p01_detail.md's pitfall list):
//! a permanently-RWX region with no toggle means a wild pointer write can
//! corrupt live code silently. macOS's per-thread toggle was a tripwire that
//! turned such a write into an immediate fault; Windows offers no equivalent.
//! What replaces it is discipline that is already in place — every write goes
//! through `JitWriteGuard` (`codecache/mod.rs` D3, audited by grep, no other
//! writer exists) — plus, from P2, PROBE's code-range dossier as the
//! *detection* story. The `VirtualProtect` RW↔RX upgrade described on
//! [`jit_write_protect`] would restore the tripwire; it is a hardening option,
//! not a correctness gap.
//!
//! ## Sprint posture
//!
//! P0 (this sprint) ships the region + the W^X pair, because the interpreter
//! genuinely needs both: `VmState::with_options` reserves the code cache
//! unconditionally and `codecache::stubs::install` publishes the hand-
//! assembled A64 trampolines into it at boot **in every `JitMode`, `Off`
//! included** (`ffi_stubs::install` likewise — an FFI method runs interpreted
//! and still calls through its trampoline). A fake or zero-sized region would
//! not "defer" anything; it would abort the boot P0 exists to achieve.
//!
//! Everything above that line — module placement, relocation patching,
//! host-symbol resolution, the `Loader` builder path — is absent here and
//! marked `WINARM (P1)` at each hand-off point.

#![cfg(all(windows, target_arch = "aarch64"))]

use std::ffi::c_void;
use std::ptr;

use anyhow::{bail, Result};

// ── kernel32 FFI ────────────────────────────────────────────────────────────
//
// `libc` on Windows exposes the CRT, not Win32 (sprint_p00_detail.md's
// pitfall list), so these are declared by hand — same convention as
// `memory/reservation.rs`'s `win` module and WINVM's loader.

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;

/// `SYSTEM_INFO` (`sysinfoapi.h`). Only `page_size` is read; the rest of the
/// layout has to be spelled out anyway so the struct's size is right for the
/// callee to fill. The leading `u16` pair is the `dwOemId` union viewed as
/// `wProcessorArchitecture` + `wReserved`, which is how every modern header
/// documents it.
#[repr(C)]
struct SystemInfo {
    processor_arch: u16,
    reserved: u16,
    page_size: u32,
    min_app_addr: *mut c_void,
    max_app_addr: *mut c_void,
    active_processor_mask: usize,
    number_of_processors: u32,
    processor_type: u32,
    allocation_granularity: u32,
    processor_level: u16,
    processor_revision: u16,
}

extern "system" {
    fn VirtualAlloc(addr: *mut c_void, size: usize, alloc_type: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, free_type: u32) -> i32;
    /// Returns the current-process *pseudo*-handle (`-1`), not a real handle —
    /// nothing to close, and cheap enough to call per invalidation.
    fn GetCurrentProcess() -> *mut c_void;
    /// The documented Win32 primitive for cross-modifying code: on ARM64 it
    /// performs the data-cache clean to the point of unification and the
    /// instruction-cache invalidate, broadcasting to the other cores. `size`
    /// is in **bytes**. See [`icache_invalidate`] for the half it does *not*
    /// do.
    fn FlushInstructionCache(process: *mut c_void, base: *const c_void, size: usize) -> i32;
    fn GetSystemInfo(info: *mut SystemInfo);
}

/// The native page size, queried once. **Not** a constant: MIGRATION.md §3.4
/// and P0's D4 audit both single out the macOS sibling's hardcoded
/// `PAGE = 0x4000` (16 KiB, an Apple Silicon fact) as the exact value that
/// must never leak into the Windows path — pages are 4 KiB here. Allocation
/// *granularity* is 64 KiB, but that only constrains the base address, which
/// `VirtualAlloc` aligns itself; adding alignment math the API already does
/// would be the other half of the same mistake.
fn page_size() -> usize {
    static PAGE_SIZE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *PAGE_SIZE.get_or_init(|| {
        // SAFETY: `GetSystemInfo` writes the whole struct and touches no
        // other memory; the zeroed value is a valid starting state.
        unsafe {
            let mut info = std::mem::zeroed::<SystemInfo>();
            GetSystemInfo(&mut info);
            info.page_size as usize
        }
    })
}

fn round_up(n: usize, to: usize) -> usize {
    (n + to - 1) & !(to - 1)
}

// ── The region owner ────────────────────────────────────────────────────────

/// The Windows-ARM64 JIT region owner — the counterpart of the macOS
/// `MacJit`, reachable under the platform-neutral name
/// [`crate::vendor::wfasm::NativeJit`] that `codecache` actually imports.
///
/// It carries only what P0 needs: the region and its size. `MacJit`'s
/// `symbols` / `externs` / `placed` / `pending_text` / `finalized` fields all
/// belong to its build-load-finalize protocol, which `CodeCache` does not use
/// — `CodeCache` takes [`region_raw`](WinA64Jit::region_raw) and does its own
/// segment allocation, publishing, and patching (sprint_s09_detail.md D2.3).
/// `writable` has no analogue at all: there is no per-thread W^X state on
/// this platform to track.
///
// WINARM (P1): the real loader lands here — see sprint_p01_detail.md D1.
// It adds, on top of this region: `load_module` + `finalize` (relocation
// resolution via the **unchanged** vendored `relocpatch.rs` — `Branch26`
// patched in place, far/absolute targets through the existing
// `movz/movk x16; br x16` veneer emitter), `define_extern`/`lookup`, the
// `backend::Loader` impl for the builder path, and `dlsym_resolve`'s Windows
// twin (`GetModuleHandleA`/`LoadLibraryA` + `GetProcAddress`), which is also
// P5's FFI substrate. None of it is reachable in P0: nothing calls the
// loader, because nothing compiles.
pub struct WinA64Jit {
    region: *mut u8,
    cap: usize,
}

impl WinA64Jit {
    /// Reserve and commit `cap` bytes (rounded up to a whole page) of
    /// `PAGE_EXECUTE_READWRITE` code space.
    ///
    /// Committing the whole region up front matches both siblings — the code
    /// cache does its own bump/freelist management inside it and would fault
    /// on a reserve-only page the moment it published. It costs working set
    /// only as pages are actually touched.
    ///
    /// Unlike `MacJit::with_capacity`, this leaves *no* per-thread W^X state
    /// behind, because none exists: the region is simultaneously writable and
    /// executable from this call until `Drop`. `CodeCache::new`'s follow-up
    /// `jit_write_protect(true)` is therefore a no-op here rather than the
    /// meaningful flip it is on macOS — see [`jit_write_protect`].
    pub fn with_capacity(cap: usize) -> Result<Self> {
        let page = page_size();
        let cap = round_up(cap.max(page), page);
        // SAFETY: fixed-shape allocation call with a null (let-the-OS-choose)
        // base; the result is null-checked before it is stored.
        let region = unsafe {
            VirtualAlloc(
                ptr::null_mut(),
                cap,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        } as *mut u8;
        if region.is_null() {
            bail!("VirtualAlloc(PAGE_EXECUTE_READWRITE) failed — JIT memory unavailable");
        }
        Ok(WinA64Jit { region, cap })
    }

    /// Hand the raw region to `CodeCache`, which manages it directly —
    /// identical signature and identical contract to the macOS sibling's
    /// `MacJit::region_raw`. `cap` is the **rounded** size, so the caller's
    /// bump allocator can trust every byte of it to be committed.
    pub fn region_raw(&self) -> (*mut u8, usize) {
        (self.region, self.cap)
    }
}

impl Drop for WinA64Jit {
    fn drop(&mut self) {
        if !self.region.is_null() {
            // SAFETY: `region` is exactly the base `with_capacity` got back
            // from `VirtualAlloc`, and nothing aliases it (`WinA64Jit` is
            // neither `Clone` nor `Copy`). `MEM_RELEASE` requires a size of 0
            // and frees the whole allocation.
            //
            // No write-mode restoration is needed before releasing (the macOS
            // twin flips back to writable here so the next builder on this
            // thread starts clean) — there is no per-thread state to leave
            // behind.
            unsafe {
                VirtualFree(self.region as *mut c_void, 0, MEM_RELEASE);
            }
            self.region = ptr::null_mut();
        }
    }
}

// ── The W^X pair — same names, same signatures as the macOS twin ────────────
//
// `codecache/guard.rs` imports exactly these two, by these names, with these
// signatures, and knows nothing else about either platform. Changing either
// signature here breaks that neutrality (sprint_p01_detail.md: "no unit slip
// possible if the pair's signature matches `native_macos.rs` exactly — it
// must").

/// No-op. Windows has no `MAP_JIT` and no per-thread write-protect toggle;
/// the region is `PAGE_EXECUTE_READWRITE` for its whole lifetime, so there is
/// no state for `exec` to select. Identical posture to WINVM's x86-64 loader.
///
/// It must stay **cheap and infallible**: `JitWriteGuard` calls it twice per
/// publish, per branch patch, per literal-pool word — i.e. once per inline-
/// cache update at steady state. Nothing here may allocate, lock, or fail.
///
/// The RW↔RX `VirtualProtect` hygiene upgrade remains available later
/// **without changing a single call site**, which is the point worth
/// recording: `guard.rs` already brackets every write correctly (construct →
/// write → drop), so a future implementation can flip the region's protection
/// on each edge and the callers never learn about it. What that upgrade would
/// buy is the tripwire this module's doc describes losing; what it costs is a
/// syscall pair on a hot path and a `Result` where there is currently none —
/// which is why it is deferred to a measurement, not adopted on principle
/// (MIGRATION.md §3.1, sprint_p01_detail.md "Out of scope").
#[inline(always)]
pub fn jit_write_protect(_exec: bool) {}

/// Make `[start, start+len)` safe to execute after it has been written.
/// `len` is in **bytes** (`JitWriteGuard::note` records byte lengths, and the
/// macOS twin takes bytes too — the signatures match so no unit can slip).
///
/// **Both halves are mandatory on ARM64, and this is the single most
/// important correctness point in the Windows-ARM64 port.** AArch64 has split
/// instruction and data caches, exactly like Apple Silicon and *unlike*
/// x86-64; the architecture requires a specific sequence before a core may
/// execute words another store just produced:
///
/// 1. `FlushInstructionCache` does the shared part — clean the data cache to
///    the point of unification, invalidate the instruction cache, and let the
///    kernel broadcast the invalidate to the other cores (the `dc cvau` /
///    `dsb ish` / `ic ivau` / `dsb ish` sequence, performed on our behalf).
/// 2. `isb sy` does the part **no other core can do for this one**: discard
///    the instructions this thread has *already* fetched and decoded past the
///    range. A pipeline holding pre-write fetches is invisible to any cache
///    maintenance, so nothing but a local context-synchronizing event clears
///    it — and the thread that just wrote the code is precisely the thread
///    about to call into it.
///
/// Omitting either half produces the classic stale-icache failure: rare,
/// unreproducible, wrong-instruction execution, most likely under exactly the
/// patch-and-immediately-rerun pattern inline caches create. It is
/// probabilistic, so a green test run is not evidence of correctness — which
/// is why the pair goes in together on day one rather than after a bug hunt.
/// The macOS twin's `sys_icache_invalidate` bundles both concerns into one
/// libSystem call; here they are two explicit statements.
///
/// For contrast worth keeping: WINVM's x86-64 sibling calls
/// `FlushInstructionCache` *pro forma* — x86 has coherent I/D caches and the
/// call is a documented formality there. On this target it is load-bearing.
pub fn icache_invalidate(start: *const u8, len: usize) {
    // SAFETY: `GetCurrentProcess` yields the current-process pseudo-handle;
    // `[start, start+len)` is a range the caller just wrote inside the live
    // code region, and `FlushInstructionCache` reads no memory through it.
    // `isb sy` is a context-synchronization barrier: it touches no memory and
    // no registers, hence `nostack`/`preserves_flags`. It is deliberately NOT
    // marked `nomem` — the compiler must not be told it is free to move
    // memory operations across it.
    unsafe {
        FlushInstructionCache(GetCurrentProcess(), start as *const c_void, len);
        core::arch::asm!("isb sy", options(nostack, preserves_flags));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The region is real, committed, and page-rounded — and the rounding
    /// used the *queried* page size, not Apple's 16 KiB. A request of one
    /// byte therefore yields exactly one 4 KiB page on this machine, and the
    /// assertion is written against `page_size()` so it stays true if Windows
    /// ever reports something else.
    #[test]
    fn region_is_page_rounded() {
        let jit = WinA64Jit::with_capacity(1).expect("VirtualAlloc");
        let (base, cap) = jit.region_raw();
        assert!(!base.is_null());
        assert_eq!(cap, page_size(), "one byte must round to exactly one page");
        assert_ne!(cap, 0x4000, "0x4000 is the Apple page size — see page_size()");
    }

    /// The smallest possible end-to-end proof of this whole module: write one
    /// A64 `ret` (`0xD65F03C0`) into the region, invalidate, and call it.
    /// Exercises `VirtualAlloc(PAGE_EXECUTE_READWRITE)` → a raw store into
    /// executable memory → [`icache_invalidate`]'s flush+`isb` → execution of
    /// the result on the same thread, which is precisely the sequence
    /// `CodeCache::publish` performs under a `JitWriteGuard`.
    ///
    /// `ret` alone is a valid identity function under AAPCS64: x0 is both the
    /// first argument and the return value, so a returned `42` proves control
    /// actually reached and left the written word.
    #[test]
    fn written_word_executes() {
        let jit = WinA64Jit::with_capacity(1).expect("VirtualAlloc");
        let (base, _cap) = jit.region_raw();
        // SAFETY: `base` owns at least a page; 4 bytes is in bounds. The
        // region is writable and executable, and the transmute matches the
        // AAPCS64 signature the emitted `ret` implements.
        let f: extern "C" fn(u64) -> u64 = unsafe {
            std::ptr::write(base as *mut u32, 0xD65F_03C0);
            icache_invalidate(base, 4);
            std::mem::transmute(base)
        };
        assert_eq!(f(42), 42);
    }
}
