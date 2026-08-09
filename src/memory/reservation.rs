//! Address-space reservation (SPEC §7.1): one `mmap` reservation,
//! `PROT_NONE`, committed lazily with `mprotect` — the MacNCL `Backing`
//! pattern (`sprint_s07_detail.md` §Design). `commit`/`decommit` are
//! idempotent: `mprotect`/`madvise` are themselves idempotent at the OS
//! level, so no separate range-bookkeeping structure is needed for
//! correctness (re-committing an already-committed range, or decommitting
//! an already-decommitted one, is just a repeat syscall — never an error).
//! [`test_heap`](Reservation::test_heap) swaps the mmap for a plain
//! `Box<[u8]>` for deterministic, allocation-cheap unit tests: no real
//! `mmap`/`mprotect` calls, `commit`/`decommit` are no-ops (a `Box<[u8]>`
//! is already fully readable/writable and there is nothing meaningful to
//! protect).
//!
//! WINARM (P0 D2#2): Windows has no `mmap`, and no `libc` route to Win32
//! either (the `libc` crate exposes only the CRT there — see `mod win`), so
//! the same reserve/commit/decommit/release shape is built directly on
//! kernel32 below, behind this file's *unchanged* API. Every caller in the
//! VM is untouched; the unix path is gated, never rewritten.
//!
//! ```text
//! reserve    mmap(PROT_NONE, MAP_PRIVATE|MAP_ANON)
//!         -> VirtualAlloc(MEM_RESERVE, PAGE_NOACCESS)
//! commit     mprotect(PROT_READ|PROT_WRITE)
//!         -> VirtualAlloc(MEM_COMMIT, PAGE_READWRITE)
//! decommit   madvise(MADV_FREE) + mprotect(PROT_NONE)
//!         -> VirtualFree(MEM_DECOMMIT)
//! drop       munmap
//!         -> VirtualFree(.., 0, MEM_RELEASE)
//! page size  sysconf(_SC_PAGESIZE)
//!         -> GetSystemInfo().dwPageSize
//! ```
//!
//! The idempotence argument above carries word for word: `MEM_COMMIT` over
//! an already-committed range at the same protection succeeds and changes
//! nothing, and MSDN states of `MEM_DECOMMIT` that the call "does not fail
//! if you attempt to decommit an uncommitted page" — which is precisely why
//! it needs no range-bookkeeping structure on this side either. Windows in
//! fact **exceeds** the "contents unspecified after decommit" contract:
//! `MEM_DECOMMIT` releases the physical frames and a later `MEM_COMMIT`
//! hands back zero-filled pages (`VirtualAlloc` documents the
//! zero-initialisation). Callers must still not rely on that — the contract
//! is the weaker unix one, and this file is the only place the difference
//! is allowed to be visible (it is asserted in
//! `reservation_commit_decommit_roundtrip_windows` below).
//!
//! [`test_heap`](Reservation::test_heap) is platform-independent: it is a
//! plain `Box<[u8]>` on both hosts, so the deterministic unit-test mode
//! keeps working on Windows with no Windows code in its path at all.

use std::ffi::c_void;

// WINARM (P0 D2#2): kernel32's address-space primitives, declared by hand.
// Two reasons this is not a crate dependency: on Windows `libc` binds only
// the CRT (so `mmap`/`mprotect`/`sysconf` genuinely do not exist to bind
// to), and pulling `windows-sys` into the core crate would buy a build-graph
// edge for three functions — sprint P0 keeps its dependency delta at zero.
// This is the same discipline (and the same three imports) as WINVM's copy
// of this file, and it carries to ARM64 unchanged: the Win32 virtual-memory
// API is architecture-neutral — only the page size it reports differs.
#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    // `MEM_*`/`PAGE_*` from `memoryapi.h`. These are ABI constants, fixed
    // for the life of Win32.
    //
    // NOTE for the P0 D4 page-size audit: `MEM_DECOMMIT` is 0x4000, the same
    // number as Apple's 16 KiB page size. It is a *flag bit*, not page math —
    // the audit's `rg 0x4000` hit here is a false positive, and must not be
    // "fixed" into a queried value.
    pub const MEM_COMMIT: u32 = 0x1000;
    pub const MEM_RESERVE: u32 = 0x2000;
    pub const MEM_DECOMMIT: u32 = 0x4000;
    pub const MEM_RELEASE: u32 = 0x8000;
    pub const PAGE_NOACCESS: u32 = 0x01;
    pub const PAGE_READWRITE: u32 = 0x04;

    /// `SYSTEM_INFO` (sysinfoapi.h). Only `page_size` (`dwPageSize`) is ever
    /// read, but every field is declared and in order deliberately:
    /// `GetSystemInfo` writes the WHOLE struct through the pointer it is
    /// given, so a truncated declaration would be a stack buffer overrun,
    /// not merely a wasted field. (The leading `wProcessorArchitecture` /
    /// `wReserved` pair is the readable half of the `dwOemId` union.)
    /// (No `#[allow(dead_code)]` is needed for the fields nothing reads:
    /// rustc exempts `#[repr(C)]` structs from the never-read-field lint,
    /// precisely because their layout is dictated from outside Rust.)
    #[repr(C)]
    pub struct SystemInfo {
        pub processor_arch: u16,
        pub reserved: u16,
        pub page_size: u32,
        pub min_app_addr: *mut c_void,
        pub max_app_addr: *mut c_void,
        pub active_processor_mask: usize,
        pub number_of_processors: u32,
        pub processor_type: u32,
        pub allocation_granularity: u32,
        pub processor_level: u16,
        pub processor_revision: u16,
    }

    extern "system" {
        pub fn VirtualAlloc(
            addr: *mut c_void,
            size: usize,
            alloc_type: u32,
            protect: u32,
        ) -> *mut c_void;
        pub fn VirtualFree(addr: *mut c_void, size: usize, free_type: u32) -> i32;
        pub fn GetSystemInfo(info: *mut SystemInfo);
    }
}

/// A single `mmap` address-space reservation (WINARM: a single
/// `VirtualAlloc` `MEM_RESERVE` region on Windows), OR (in test mode) a
/// plain heap allocation standing in for one. Uncommitted pages are
/// `PROT_NONE` (`PAGE_NOACCESS`) and touching them faults;
/// [`commit`](Reservation::commit) makes a sub-range read/write.
pub struct Reservation {
    base: usize,
    size: usize,
    /// `Some` in `test_heap` mode: owns the backing memory (dropped
    /// normally by Rust), and `commit`/`decommit` become no-ops.
    test_box: Option<Box<[u8]>>,
}

impl Reservation {
    /// Reserve `size` bytes of address space (`PROT_NONE`,
    /// `MAP_PRIVATE|MAP_ANON`). `size` is rounded up to the system page size.
    /// Calls `fatal_exit` (not a panic — an environment limit, not a VM bug)
    /// if the OS refuses the reservation: the whole process for the CLI/
    /// tests, or just the calling thread for an embedded `VmHandle` (S21) —
    /// see `runtime::vm_state::fatal_exit`'s own doc.
    #[cfg(unix)]
    pub fn reserve(size: usize) -> Reservation {
        let size = round_up(size, page_size());
        // SAFETY: a fixed-shape mmap call with a null hint address (the
        // kernel picks the base) and no file descriptor; the result is
        // checked for MAP_FAILED before use.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            eprintln!(
                "macvm: failed to reserve {size} bytes: {}",
                std::io::Error::last_os_error()
            );
            crate::runtime::vm_state::fatal_exit(71);
        }
        Reservation {
            base: ptr as usize,
            size,
            test_box: None,
        }
    }

    /// WINARM (P0 D2#2): `VirtualAlloc(MEM_RESERVE, PAGE_NOACCESS)` — address
    /// space claimed but no physical commitment made, and every page in it
    /// faults on touch. That is exactly the `mmap(PROT_NONE)` semantics the
    /// unix arm above gets, including the failure policy: `fatal_exit(71)`,
    /// because a refused reservation is an environment limit, not a VM bug.
    ///
    /// The null hint matters twice over: the OS picks the base, and it picks
    /// one aligned to the 64 KiB *allocation granularity* (which is coarser
    /// than the 4 KiB page — MIGRATION.md §3.4). Callers only ever see
    /// offsets from `base()`, so the granularity never enters this file's
    /// arithmetic; the page-rounded `size` we record is also exactly what
    /// `VirtualAlloc` reserves, since it rounds a NULL-hint request up to the
    /// next page boundary itself.
    #[cfg(windows)]
    pub fn reserve(size: usize) -> Reservation {
        let size = round_up(size, page_size());
        // SAFETY: a fixed-shape VirtualAlloc call with a null hint address
        // (the kernel picks the base) reserving `size` page-rounded bytes;
        // the result is checked for null before use.
        let ptr = unsafe {
            win::VirtualAlloc(
                std::ptr::null_mut(),
                size,
                win::MEM_RESERVE,
                win::PAGE_NOACCESS,
            )
        };
        if ptr.is_null() {
            eprintln!(
                "macvm: failed to reserve {size} bytes: {}",
                std::io::Error::last_os_error()
            );
            crate::runtime::vm_state::fatal_exit(71);
        }
        Reservation {
            base: ptr as usize,
            size,
            test_box: None,
        }
    }

    /// A deterministic small heap for unit tests: a plain `Box<[u8]>`
    /// (8-byte aligned via `vec![0u8; size]`'s allocator guarantee), no
    /// `mmap` involved. `commit`/`decommit` are no-ops — the memory is
    /// already fully accessible.
    pub fn test_heap(size: usize) -> Reservation {
        let size = round_up(size, 8);
        let mut v = vec![0u8; size].into_boxed_slice();
        let base = v.as_mut_ptr() as usize;
        Reservation {
            base,
            size,
            test_box: Some(v),
        }
    }

    /// Make `[off, off+len)` (page-rounded outward) readable and writable.
    /// Calls `fatal_exit` if `mprotect` fails (see `reserve`'s own doc). No-op
    /// in `test_heap` mode.
    ///
    /// WINARM (P0 D2#2): `VirtualAlloc(MEM_COMMIT, PAGE_READWRITE)` on
    /// Windows. The range math, the overflow check, the `debug_assert!` and
    /// the failure policy are shared — only the one call that changes the
    /// pages differs, so the two hosts cannot drift on the part that is
    /// actually this module's contract.
    pub fn commit(&self, off: usize, len: usize) {
        if self.test_box.is_some() {
            return;
        }
        let page = page_size();
        let start = round_down(off, page);
        let end = round_up(off.checked_add(len).expect("commit range overflow"), page);
        debug_assert!(
            end <= self.size,
            "commit range [{start},{end}) exceeds reservation of {} bytes",
            self.size
        );
        let ptr = (self.base + start) as *mut c_void;
        // SAFETY: `ptr` and `end - start` are within this reservation
        // (checked above); mprotect only changes protection, no memory is
        // read or written by this call itself.
        #[cfg(unix)]
        let ok =
            unsafe { libc::mprotect(ptr, end - start, libc::PROT_READ | libc::PROT_WRITE) } == 0;
        // WINARM (P0 D2#2): MEM_COMMIT inside an existing reservation, which
        // is idempotent by documentation — re-committing pages that are
        // already committed at the same protection succeeds and leaves their
        // contents alone. That is the property the module doc leans on to
        // avoid range bookkeeping, and it is the same property `mprotect`
        // gives the unix arm.
        //
        // SAFETY: as above — `ptr`/`end - start` lie inside this reservation;
        // VirtualAlloc only changes the commitment state of those pages and
        // neither reads nor writes through the pointer.
        #[cfg(windows)]
        let ok = unsafe {
            !win::VirtualAlloc(ptr, end - start, win::MEM_COMMIT, win::PAGE_READWRITE).is_null()
        };
        if !ok {
            eprintln!(
                "macvm: failed to commit {len} bytes at offset {off}: {}",
                std::io::Error::last_os_error()
            );
            crate::runtime::vm_state::fatal_exit(71);
        }
    }

    /// `madvise(MADV_FREE)` (the effective "give pages back" advice on
    /// macOS, not `MADV_DONTNEED`) then `mprotect(PROT_NONE)` over
    /// `[off, off+len)`, page-rounded outward. Reclaimed memory is NOT
    /// guaranteed zeroed on a later re-`commit` (lesson 5 — callers must
    /// never assume it). No-op in `test_heap` mode. Calls `fatal_exit` on
    /// failure, same policy as `commit`.
    ///
    /// WINARM (P0 D2#2): one `VirtualFree(MEM_DECOMMIT)` does both halves on
    /// Windows — it returns the physical frames (the `MADV_FREE` half) *and*
    /// leaves the range reserved-but-inaccessible, so touching it faults (the
    /// `mprotect(PROT_NONE)` half). It is idempotent for the same reason
    /// `madvise`/`mprotect` are: MSDN states the call does not fail when the
    /// pages are already decommitted, so a repeat is a repeat syscall, never
    /// an error. A later re-`commit` returns **zeroed** pages, which
    /// satisfies — in fact exceeds — the "contents unspecified" contract
    /// above; nothing may be written that depends on the difference.
    pub fn decommit(&self, off: usize, len: usize) {
        if self.test_box.is_some() {
            return;
        }
        let page = page_size();
        let start = round_down(off, page);
        let end = round_up(off.checked_add(len).expect("decommit range overflow"), page);
        debug_assert!(
            end <= self.size,
            "decommit range [{start},{end}) exceeds reservation of {} bytes",
            self.size
        );
        let ptr = (self.base + start) as *mut c_void;
        // SAFETY: as `commit` — range is within this reservation; madvise
        // is advisory (never a correctness requirement) and mprotect only
        // changes protection.
        #[cfg(unix)]
        let ok = {
            unsafe {
                libc::madvise(ptr, end - start, libc::MADV_FREE);
            }
            unsafe { libc::mprotect(ptr, end - start, libc::PROT_NONE) == 0 }
        };
        // WINARM (P0 D2#2): the single-call equivalent — see this method's
        // doc comment. Only the commitment state changes; the reservation
        // itself survives (MEM_RELEASE, the call that would destroy it, is
        // reached only from `Drop`).
        //
        // SAFETY: as the unix arm — `ptr`/`end - start` lie inside this
        // reservation, and VirtualFree neither reads nor writes through the
        // pointer.
        #[cfg(windows)]
        let ok = unsafe { win::VirtualFree(ptr, end - start, win::MEM_DECOMMIT) } != 0;
        if !ok {
            eprintln!(
                "macvm: failed to decommit {len} bytes at offset {off}: {}",
                std::io::Error::last_os_error()
            );
            crate::runtime::vm_state::fatal_exit(71);
        }
    }

    #[inline]
    pub fn base(&self) -> usize {
        self.base
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.test_box.is_some() {
            return; // Box drops itself normally — no mapping to release.
        }
        // SAFETY: `self.base`/`self.size` describe exactly the mapping
        // created in `reserve`; nothing else can alias it (Reservation is
        // not Clone).
        #[cfg(unix)]
        unsafe {
            libc::munmap(self.base as *mut c_void, self.size);
        }
        // WINARM (P0 D2#2): MEM_RELEASE frees the reservation *and* anything
        // still committed inside it, in one call. The size argument MUST be
        // 0 — MEM_RELEASE is all-or-nothing over the region returned by the
        // original `VirtualAlloc`, and passing `self.size` here would make
        // the call fail (silently, since Drop has nowhere to report).
        //
        // SAFETY: as the unix arm — `self.base` is exactly the base
        // `VirtualAlloc` returned in `reserve`, and nothing else can alias
        // this reservation (Reservation is not Clone).
        #[cfg(windows)]
        unsafe {
            win::VirtualFree(self.base as *mut c_void, 0, win::MEM_RELEASE);
        }
    }
}

#[cfg(unix)]
fn page_size() -> usize {
    // SAFETY: sysconf with a valid, static name; no memory access.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// WINARM (P0 D2#2, and the P0 D4 audit's whole point): the page size is
/// **queried**, never a constant. Windows on ARM64 uses 4 KiB pages where
/// Apple Silicon uses 16 KiB (MIGRATION.md §3.4), so a baked-in `0x4000`
/// would silently over-round every commit here — and a baked-in `0x1000`
/// would be just as wrong the day this file is read on the Mac side.
/// `GetSystemInfo` is a cached user-mode read rather than a syscall, so
/// calling it per commit costs what the `sysconf` it mirrors costs and
/// needs no memoisation.
#[cfg(windows)]
fn page_size() -> usize {
    // SAFETY: `GetSystemInfo` writes the whole struct through the pointer;
    // `SystemInfo` is declared in full (`mod win` explains why that is a
    // safety requirement and not a nicety), and zeroing first means every
    // field is a valid bit pattern even if a future OS leaves one alone.
    unsafe {
        let mut info = std::mem::zeroed::<win::SystemInfo>();
        win::GetSystemInfo(&mut info);
        info.page_size as usize
    }
}

fn round_up(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

fn round_down(v: usize, align: usize) -> usize {
    v & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_commit_rw() {
        let r = Reservation::reserve(64 << 20);
        r.commit(0, 1 << 20);

        let p0 = r.base() as *mut u64;
        let p_last = (r.base() + (1 << 20) - 8) as *mut u64;
        unsafe {
            p0.write(0xDEAD_BEEF_CAFE_0000);
            assert_eq!(p0.read(), 0xDEAD_BEEF_CAFE_0000);
            p_last.write(0x1234_5678_9ABC_DEF0);
            assert_eq!(p_last.read(), 0x1234_5678_9ABC_DEF0);
        }
    }

    #[test]
    fn commit_page_rounding() {
        let r = Reservation::reserve(16 << 20);
        r.commit(0, 4097);
        let p = (r.base() + 8100) as *mut u8;
        unsafe {
            p.write(42);
            assert_eq!(p.read(), 42);
        }
    }

    #[test]
    fn round_trip_helpers() {
        assert_eq!(round_up(0, 16), 0);
        assert_eq!(round_up(1, 16), 16);
        assert_eq!(round_up(16, 16), 16);
        assert_eq!(round_down(17, 16), 16);
        assert_eq!(round_down(16, 16), 16);
    }

    /// `tests_s07.md`: double `commit` of the same range succeeds; the
    /// range is still readable/writable afterwards (mprotect is naturally
    /// idempotent, but this pins the contract, not just the mechanism).
    #[test]
    fn reserve_commit_idempotent() {
        let r = Reservation::reserve(16 << 20);
        r.commit(0, 4096);
        r.commit(0, 4096);
        let p = r.base() as *mut u64;
        unsafe {
            p.write(0x1122_3344_5566_7788);
            assert_eq!(p.read(), 0x1122_3344_5566_7788);
        }
    }

    /// Apple Silicon pages are 16 KiB — committing a single byte must make
    /// the WHOLE surrounding 16 KiB page writable, not just a 4 KiB slice.
    ///
    /// WINARM (P0 D3, "comes back: never"): the 16 KiB here is an *Apple*
    /// fact, not a portable one. On Windows ARM64 pages are 4 KiB, so after
    /// a 1-byte commit the address this test pokes is genuinely still
    /// uncommitted and the write would fault — the assertion is
    /// macOS-page-size-specific, so the test is gated rather than weakened.
    /// Its Windows twin is `commit_rounds_to_host_page_windows` below.
    #[cfg(target_os = "macos")]
    #[test]
    fn commit_rounds_to_host_page() {
        let r = Reservation::reserve(1 << 20);
        r.commit(0, 1);
        let page = 16 << 10;
        let last_in_page = (r.base() + page - 8) as *mut u64;
        unsafe {
            last_in_page.write(0xAA);
            assert_eq!(last_in_page.read(), 0xAA);
        }
    }

    /// WINARM (P0 D2#2/D3, `tests_p00.md`): the Windows twin of
    /// `commit_rounds_to_host_page`. Same property — a 1-byte commit makes
    /// the whole surrounding HOST page writable — but stated against the
    /// *queried* page size, so it holds on any host instead of trading
    /// Apple's 16 KiB constant for a Windows 4 KiB one.
    #[cfg(windows)]
    #[test]
    fn commit_rounds_to_host_page_windows() {
        let r = Reservation::reserve(1 << 20);
        r.commit(0, 1);
        let page = page_size();
        let last_in_page = (r.base() + page - 8) as *mut u64;
        unsafe {
            last_in_page.write(0xAA);
            assert_eq!(last_in_page.read(), 0xAA);
        }
        // The negative half — that `base + page` is NOT writable — is
        // deliberately not asserted: catching that access violation needs the
        // VEH that P2 installs, and an uncaught faulting write here would
        // take the whole test process down rather than fail one test.
    }

    /// WINARM (P0 D2#2, `tests_p00.md` `reservation_page_size_is_native`):
    /// the page size this module rounds with must be the OS's own answer.
    /// The first assertion is the invariant that matters (`page_size()` IS
    /// `GetSystemInfo`'s `dwPageSize`, not a constant that happens to agree
    /// today); the second and third pin the two host facts MIGRATION.md
    /// §3.4 calls out — 4 KiB pages inside a 64 KiB allocation granularity —
    /// so Apple's 16 KiB (0x4000) leaking into this path would fail loudly
    /// and immediately. These literals are a tripwire in a test; the
    /// non-test code above contains no page constant at all, which is the
    /// D4 audit's actual requirement.
    #[cfg(windows)]
    #[test]
    fn reservation_page_size_is_native() {
        // SAFETY: identical to `page_size` itself — fully declared struct,
        // zeroed before the OS writes it.
        let (os_page, os_granularity) = unsafe {
            let mut info = std::mem::zeroed::<win::SystemInfo>();
            win::GetSystemInfo(&mut info);
            (
                info.page_size as usize,
                info.allocation_granularity as usize,
            )
        };
        assert_eq!(
            page_size(),
            os_page,
            "page_size() must report GetSystemInfo's dwPageSize, not a constant"
        );
        assert_eq!(os_page, 4096, "Windows on ARM64 pages are 4 KiB");
        assert_eq!(
            os_granularity, 65536,
            "allocation granularity is 64 KiB and is NOT the page size"
        );
    }

    /// WINARM (P0 D2#2, `tests_p00.md`
    /// `reservation_commit_decommit_roundtrip_windows`): commit → write →
    /// decommit → re-commit, and the re-committed range reads back **zero**.
    ///
    /// `decommit_then_recommit` above pins the portable contract (the range
    /// is usable again, contents unspecified); this pins the stronger thing
    /// the Windows mapping actually delivers — `MEM_DECOMMIT` really returns
    /// the physical frames, and `MEM_COMMIT` hands back zero-filled pages.
    /// That distinction is worth a test because a `decommit` that silently
    /// did nothing at all would still pass `decommit_then_recommit`, and
    /// would leak every dead generation's pages for the life of the process.
    /// Both calls are also issued twice, which is the idempotence contract
    /// the module doc relies on to justify keeping no range bookkeeping.
    #[cfg(windows)]
    #[test]
    fn reservation_commit_decommit_roundtrip_windows() {
        let page = page_size();
        let r = Reservation::reserve(16 << 20);
        r.commit(0, page);

        // Dirty the WHOLE page, not just its head: a decommit that only
        // reached the first page-worth of bytes, or none at all, would
        // otherwise hide behind an untouched tail.
        //
        // SAFETY: `[base, base+page)` is committed above and owned solely by
        // this Reservation; the slice is confined to this block, so it does
        // not outlive the commitment it borrows.
        {
            let dirty = unsafe { std::slice::from_raw_parts_mut(r.base() as *mut u8, page) };
            dirty.fill(0xA5);
            assert!(
                dirty.iter().all(|&b| b == 0xA5),
                "the committed page must be writable before it is decommitted"
            );
        }

        // Each call is issued twice: the module promises idempotence, and
        // both arms reach `fatal_exit` on a non-zero/NULL return, so a
        // second call that the OS rejected would kill this test process
        // outright rather than merely fail an assertion.
        r.decommit(0, page);
        r.decommit(0, page);
        r.commit(0, page);
        r.commit(0, page);

        // SAFETY: re-committed above; same exclusive-ownership argument.
        let fresh = unsafe { std::slice::from_raw_parts(r.base() as *const u8, page) };
        assert!(
            fresh.iter().all(|&b| b == 0),
            "re-committed pages must read back zeroed — MEM_DECOMMIT returns \
             the frames and MEM_COMMIT zero-fills (exceeding this module's \
             'contents unspecified' contract, which callers still must honour)"
        );
    }

    /// decommit → recommit → the range is readable/writable again
    /// (contents unspecified — lesson 5: reclaimed memory is never assumed
    /// zeroed).
    #[test]
    fn decommit_then_recommit() {
        let r = Reservation::reserve(16 << 20);
        r.commit(0, 4096);
        let p = r.base() as *mut u64;
        unsafe { p.write(0xDEAD) };
        r.decommit(0, 4096);
        r.commit(0, 4096);
        unsafe {
            p.write(0xBEEF);
            assert_eq!(p.read(), 0xBEEF);
        }
    }

    /// `test_heap` needs no `mmap`; `commit`/`decommit` are no-ops and the
    /// backing memory is immediately readable/writable.
    #[test]
    fn test_heap_deterministic() {
        let r = Reservation::test_heap(4096);
        // No commit() call at all — test_heap memory starts accessible.
        let p = r.base() as *mut u64;
        unsafe {
            p.write(0x42);
            assert_eq!(p.read(), 0x42);
        }
        r.commit(0, 4096); // no-op, must not panic or fault
        r.decommit(0, 4096); // no-op, must not panic or fault
        unsafe {
            assert_eq!(p.read(), 0x42); // decommit didn't actually reclaim it
        }
    }
}
