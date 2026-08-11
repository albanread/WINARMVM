//! WINARM (WG5b-2) — the Windows host service: Smalltalk's door to
//! `image_store::flows`.
//!
//! # Why this crate exists at all
//!
//! `docs/ROADMAP.md`'s WG5 row ends with *"Accept persisting to the image
//! byte-identically to the web path (the CG8 gate, re-run)"*. That gate is a
//! specification, not a nicety: two GUIs that wrote the image differently
//! would be a corruption bug with a user interface in front of it. So Accept
//! must call `image_store::flows` — the web GUI's own write path, SHARED
//! rather than reimplemented.
//!
//! Reaching it from the guest turned out to be the hard part of the slice,
//! and `docs/sprints/sprint_wg5_detail.md` records the five dead ends in
//! full. In short: `image_store` depends on `macvm`, so a primitive would be
//! a dependency cycle; a downstream crate cannot add a `PRIMITIVES` row (WG2
//! Δ 1); the Mac's answer is the ObjC runtime, which Windows has no
//! counterpart for; and the FFI could not name a library, so a DLL of one's
//! own was invisible to it.
//!
//! WG5b-2's one core change fixed the last of those — the FFI pragma now
//! takes an optional `library:` — and this crate is what it opens onto. It
//! is an ordinary DLL, downstream of the VM exactly as `cocoa_gui`'s
//! `host_service.rs` is, and the guest calls it by name:
//!
//! ```text
//! <primitive: FFI function: #MacvmHostSaveMethod
//!     library: #'winui_host.dll' ret: #g args: #(g g g)>
//! ```
//!
//! # The ABI, and why it looks like this
//!
//! The FFI marshals three shapes only — `g` (integer/pointer), `f` (double),
//! `v` (void). Everything below is therefore integers and pointers:
//!
//! * **Strings in** are NUL-terminated UTF-16, which is what
//!   `WinArena nativeUtf16:` already produces and what the rest of this port
//!   speaks. A null pointer reads as the empty string rather than faulting.
//! * **Strings out** cannot be a return value — the guest has no way to free
//!   one, and returning a pointer into a temporary would be a dangling read.
//!   So every call returns a STATUS (0 ok, non-zero failure) and parks its
//!   text in a per-process slot the guest reads afterwards with
//!   [`MacvmHostLastMessage`] + [`MacvmHostLastMessageLen`]. On success the
//!   text is the useful answer (the selector that was saved, the class that
//!   was created); on failure it is `flows`' own error string, unedited,
//!   because the whole value of sharing the write path is that the Windows
//!   GUI reports exactly what the web GUI would.
//!
//! # The image it writes
//!
//! [`image_path`] is `cocoa_gui`'s rule verbatim — `MACVM_IMAGE_PATH`, else
//! `world/image.sqlite3`. That is not incidental: the CG8 gate compares this
//! path's output against the web path's, and it can only mean something if
//! both are writing the same file by the same rule.
//!
//! # Threading
//!
//! The message slot is a `Mutex<Vec<u16>>` and the pointer it hands out stays
//! valid until the next call that replaces it. In practice only the UI VM
//! calls this — Accept is a user gesture — so the read always follows its own
//! call with nothing in between. The mutex is here so that a future caller on
//! another thread corrupts nothing; it is NOT a promise that two threads can
//! interleave a call and its message read.

use std::path::PathBuf;
use std::sync::Mutex;

use image_store::{flows, Image, Side};

/// Status: the call did what it said.
const OK: i64 = 0;
/// Status: the call failed; [`MacvmHostLastMessage`] carries `flows`' reason.
const ERR: i64 = 1;

/// The last call's text — its answer on success, its reason on failure.
/// UTF-16, NUL-terminated, replaced by each call. See the module note on
/// why the text does not come back as a return value.
static LAST_MESSAGE: Mutex<Vec<u16>> = Mutex::new(Vec::new());

/// `cocoa_gui::host_service::image_path`, verbatim. Shared by rule rather
/// than by accident: the CG8 gate compares what this writes against what the
/// web path writes, which is only a comparison if both name the same file.
fn image_path() -> PathBuf {
    std::env::var_os("MACVM_IMAGE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("world/image.sqlite3"))
}

fn writer() -> Result<Image, String> {
    Image::open(&image_path()).map_err(|e| e.to_string())
}

/// Read a guest-supplied NUL-terminated UTF-16 pointer.
///
/// # Safety
///
/// `p` must be null or point to a NUL-terminated UTF-16 sequence that stays
/// valid for the duration of the call — which `WinArena`'s buffers do, as
/// they live in an arena the guest owns and does not release mid-call.
///
/// A null pointer answers the empty string rather than faulting: a guest that
/// forgot to marshal an argument should get `flows`' own "enter a name"
/// refusal, which is a message the user can act on, not an access violation.
unsafe fn utf16_in(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0usize;
    while *p.add(n) != 0 {
        n += 1;
        // A runaway scan means the guest handed over something that is not a
        // string at all. Stop at a length no real method source reaches
        // rather than walking into unmapped memory.
        if n > 4 * 1024 * 1024 {
            break;
        }
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

fn set_message(s: &str) {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    if let Ok(mut slot) = LAST_MESSAGE.lock() {
        *slot = buf;
    }
}

/// Both halves of every entry point below: park the text, answer the status.
fn finish(r: Result<String, String>) -> i64 {
    match r {
        Ok(msg) => {
            set_message(&msg);
            OK
        }
        Err(e) => {
            set_message(&e);
            ERR
        }
    }
}

/// `flows::save_method` — the web GUI's NewMethod/EditMethod path, which is
/// what the browser's **Accept** on a method calls.
///
/// `side` is 0 for the instance side, non-zero for the class side. `flows`
/// parses the selector out of the source itself and tolerates a
/// `Foo class >> bar` prefix, so the guest does not have to strip one.
///
/// # Safety
///
/// `class` and `source` must satisfy [`utf16_in`]'s contract.
#[no_mangle]
pub unsafe extern "C" fn MacvmHostSaveMethod(
    class: *const u16,
    side: i64,
    source: *const u16,
) -> i64 {
    let class_name = utf16_in(class);
    let text = utf16_in(source);
    let side = if side == 0 {
        Side::Instance
    } else {
        Side::Class
    };
    finish(writer().and_then(|img| flows::save_method(&img, &class_name, side, &text, None)))
}

/// `flows::new_class_from_source` — the web GUI's NewClass path. Answers the
/// created class's name in the message slot.
///
/// # Safety
///
/// `source` must satisfy [`utf16_in`]'s contract.
#[no_mangle]
pub unsafe extern "C" fn MacvmHostNewClass(source: *const u16) -> i64 {
    let text = utf16_in(source);
    finish(writer().and_then(|img| flows::new_class_from_source(&img, &text, None)))
}

/// `flows::add_variable` — the `SmapplAddVar` image sequence. `is_class_var`
/// is 0 for an instance variable, non-zero for a class variable.
///
/// An instance variable changes SHAPE, so it takes effect on the next boot —
/// there is no `become:`. That is `flows`' own documented behaviour and the
/// caller's half (compiling `class_var_reopen` to make a class variable live
/// immediately) stays in the caller, exactly as it does on the Mac.
///
/// # Safety
///
/// `class` and `name` must satisfy [`utf16_in`]'s contract.
#[no_mangle]
pub unsafe extern "C" fn MacvmHostAddVariable(
    class: *const u16,
    is_class_var: i64,
    name: *const u16,
) -> i64 {
    let class_name = utf16_in(class);
    let var = utf16_in(name);
    finish(
        writer()
            .and_then(|img| flows::add_variable(&img, &class_name, is_class_var != 0, &var))
            .map(|()| var.clone()),
    )
}

/// The last call's text, as a NUL-terminated UTF-16 pointer. Valid until the
/// next call replaces it — read it before making another.
///
/// Answers a pointer to a static empty string when nothing has been said yet,
/// so the guest never has to null-check.
#[no_mangle]
pub extern "C" fn MacvmHostLastMessage() -> *const u16 {
    static EMPTY: [u16; 1] = [0];
    match LAST_MESSAGE.lock() {
        Ok(slot) if !slot.is_empty() => slot.as_ptr(),
        _ => EMPTY.as_ptr(),
    }
}

/// The last message's length in UTF-16 code units, NOT counting the NUL.
///
/// The guest reads with `WinArena stringFromUtf16At:units:`, which takes a
/// count rather than scanning for a terminator — so it needs this, and
/// getting it from the same lock that owns the buffer is what keeps the two
/// consistent.
#[no_mangle]
pub extern "C" fn MacvmHostLastMessageLen() -> i64 {
    match LAST_MESSAGE.lock() {
        Ok(slot) if !slot.is_empty() => (slot.len() - 1) as i64,
        _ => 0,
    }
}

/// `Image::method_source` — the READ half, which the browser's source pane
/// needs and which `98_winui_browser.mst` explicitly promised WG5b-2 would
/// wire up.
///
/// Source is IMAGE data by the same deliberate split the Mac uses
/// (`66_cocoabrowser.mst`'s own note): the hierarchy and selector rows come
/// from the primary's LIVE reflection, but text comes from the image, so what
/// the user edits is what the next boot will read.
///
/// A method with no stored source is not an error — it answers OK with an
/// empty message, because "this selector exists live but has no image text"
/// is an ordinary state during a session, not a failure to report.
///
/// # Safety
///
/// `class` and `selector` must satisfy [`utf16_in`]'s contract.
#[no_mangle]
pub unsafe extern "C" fn MacvmHostMethodSource(
    class: *const u16,
    side: i64,
    selector: *const u16,
) -> i64 {
    let class_name = utf16_in(class);
    let sel = utf16_in(selector);
    let side = if side == 0 {
        Side::Instance
    } else {
        Side::Class
    };
    finish(
        Image::open_read_only(&image_path())
            .map_err(|e| e.to_string())
            .and_then(|img| {
                img.method_source(&class_name, side, &sel)
                    .map_err(|e| e.to_string())
            })
            .map(|opt| opt.unwrap_or_default()),
    )
}

/// A liveness probe: answers 0x5747 (`"WG"`) if the DLL loaded and the FFI
/// found it.
///
/// It exists because the failure it distinguishes is otherwise silent and
/// baffling. If `library:` resolution is wrong — DLL not next to the exe,
/// name misspelled, export mangled — the guest sees only "the primitive
/// failed", identically to a genuine `flows` refusal. One call to this
/// separates "the channel is not there" from "the write was rejected", which
/// are entirely different problems with entirely different fixes.
#[no_mangle]
pub extern "C" fn MacvmHostPing() -> i64 {
    0x5747
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test below that touches `MACVM_IMAGE_PATH` takes this first.
    /// Environment variables are PROCESS-global and Rust runs tests in
    /// parallel by default, so two of them setting the path at once would
    /// each read the other's image — a failure that reproduces about one run
    /// in three and looks like a bug in `flows`.
    static ENV: Mutex<()> = Mutex::new(());

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("winui_host_{name}.sqlite3"));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn wide(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        v
    }

    /// The `#[no_mangle]` entry points cannot be exercised through the FFI
    /// from a unit test, but their marshalling can — and marshalling is where
    /// a host service actually breaks.
    #[test]
    fn utf16_in_reads_a_guest_string_and_tolerates_null() {
        let mut buf: Vec<u16> = "Point class >> x: aNumber".encode_utf16().collect();
        buf.push(0);
        unsafe {
            assert_eq!(utf16_in(buf.as_ptr()), "Point class >> x: aNumber");
            assert_eq!(
                utf16_in(std::ptr::null()),
                "",
                "a null argument must read as empty so the guest gets flows' own \
                 refusal rather than an access violation"
            );
        }
    }

    #[test]
    fn the_message_slot_round_trips_and_reports_its_length_without_the_nul() {
        set_message("Could not parse a message pattern from the method source.");
        let n = MacvmHostLastMessageLen() as usize;
        let p = MacvmHostLastMessage();
        let got = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(p, n)) };
        assert_eq!(
            got,
            "Could not parse a message pattern from the method source."
        );
        assert_eq!(
            n,
            "Could not parse a message pattern from the method source."
                .encode_utf16()
                .count(),
            "the length must EXCLUDE the NUL — the guest reads by count, and one \
             extra unit puts a stray \\0 on the end of every transcript line"
        );
    }

    #[test]
    fn a_failure_answers_err_and_parks_flows_own_words() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        // No image at this path, so `writer()` fails — which is the cheapest
        // way to prove the status/message split works end to end without
        // touching a real image.
        std::env::set_var("MACVM_IMAGE_PATH", "\\\\?\\nonexistent-dir\\no.sqlite3");
        let mut src: Vec<u16> = "foo ^1".encode_utf16().collect();
        src.push(0);
        let mut cls: Vec<u16> = "Object".encode_utf16().collect();
        cls.push(0);
        let rc = unsafe { MacvmHostSaveMethod(cls.as_ptr(), 0, src.as_ptr()) };
        std::env::remove_var("MACVM_IMAGE_PATH");
        assert_eq!(rc, ERR, "an unopenable image must answer ERR, not OK");
        assert!(
            MacvmHostLastMessageLen() > 0,
            "a failure must leave a reason the user can read"
        );
    }

    /// **The CG8 gate, re-run for Windows** (`docs/SPRINTS.md`: *a
    /// `#saveMethod` round-trips through `image_store` byte-identically to
    /// the web edit path*).
    ///
    /// It is a DIFFERENTIAL, and it has to be: "byte-identical" cannot mean
    /// the two SQLite files compare equal — page layout, rowids and creation
    /// order differ between any two independently built databases, and a gate
    /// asserting that would fail for reasons that have nothing to do with
    /// what was written. What it means, and what is checked here, is that
    /// every stored consequence of the save is the same: the source text, the
    /// selector parsed out of it, the side it landed on, its home file, and
    /// the version count.
    ///
    /// The two paths:
    ///
    /// * **Windows GUI** — the real exported entry point, called with real
    ///   UTF-16 pointers, exactly as the FFI calls it. Only the trampoline is
    ///   absent, and `65_winui_host_tests.mst` covers that end.
    /// * **Web** — `flows::save_method` directly, which is what the web GUI
    ///   itself calls.
    ///
    /// If these ever diverge the two GUIs are writing the image differently,
    /// which is a corruption bug with a user interface in front of it. That
    /// is why the gate exists and why it is not negotiable.
    #[test]
    fn cg8_accept_persists_exactly_as_the_web_path_does() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());

        const CLASS: &str = "Alpha";
        const SOURCE: &str = "doubled ^self value * 2";

        // Two images built the same way, so any difference below is the
        // WRITE's doing and nothing else.
        let (gui_path, web_path) = (scratch("gui"), scratch("web"));
        for p in [&gui_path, &web_path] {
            let img = Image::open(p).expect("scratch image");
            img.create_or_reopen_class(CLASS, Some("Object"), "", "", "value")
                .expect("seed class");
        }

        // Path A: the Windows GUI's own entry point, called as the FFI does.
        std::env::set_var("MACVM_IMAGE_PATH", &gui_path);
        let rc = unsafe { MacvmHostSaveMethod(wide(CLASS).as_ptr(), 0, wide(SOURCE).as_ptr()) };
        std::env::remove_var("MACVM_IMAGE_PATH");
        assert_eq!(rc, OK, "the GUI path must accept: {}", {
            let n = MacvmHostLastMessageLen() as usize;
            unsafe {
                String::from_utf16_lossy(std::slice::from_raw_parts(MacvmHostLastMessage(), n))
            }
        });
        let gui_selector = {
            let n = MacvmHostLastMessageLen() as usize;
            unsafe {
                String::from_utf16_lossy(std::slice::from_raw_parts(MacvmHostLastMessage(), n))
            }
        };

        // Path B: the web GUI's own call, same input.
        let web_img = Image::open(&web_path).expect("web image");
        let web_selector =
            flows::save_method(&web_img, CLASS, Side::Instance, SOURCE, None).expect("web accept");

        assert_eq!(
            gui_selector, web_selector,
            "both paths must parse the SAME selector out of the same source"
        );

        let gui_img = Image::open(&gui_path).expect("gui image");
        for (what, a, b) in [
            (
                "source",
                format!(
                    "{:?}",
                    gui_img.method_source(CLASS, Side::Instance, &gui_selector)
                ),
                format!(
                    "{:?}",
                    web_img.method_source(CLASS, Side::Instance, &web_selector)
                ),
            ),
            (
                "home file",
                format!(
                    "{:?}",
                    gui_img.method_source_file(CLASS, Side::Instance, &gui_selector)
                ),
                format!(
                    "{:?}",
                    web_img.method_source_file(CLASS, Side::Instance, &web_selector)
                ),
            ),
            (
                "version count",
                format!(
                    "{:?}",
                    gui_img.method_version_count(CLASS, Side::Instance, &gui_selector)
                ),
                format!(
                    "{:?}",
                    web_img.method_version_count(CLASS, Side::Instance, &web_selector)
                ),
            ),
        ] {
            assert_eq!(
                a, b,
                "CG8: the Windows Accept and the web Accept disagree about {what} — \
                 two GUIs writing the image differently is a corruption bug with a \
                 user interface in front of it"
            );
        }

        // And the source really is the text that went in, not merely the same
        // on both sides -- two identically WRONG writes would otherwise pass.
        assert_eq!(
            gui_img
                .method_source(CLASS, Side::Instance, &gui_selector)
                .unwrap()
                .unwrap(),
            SOURCE,
            "the stored source must be what was typed"
        );

        let _ = std::fs::remove_file(&gui_path);
        let _ = std::fs::remove_file(&web_path);
    }

    /// The EDIT case, which is the one with a trap in it: saving over an
    /// existing method must not clobber its recorded home file with the
    /// synthetic interactive marker. `flows` guards this deliberately (it
    /// only assigns a home when the method has none), and the guard is worth
    /// a gate because the symptom -- a method that quietly forgets which file
    /// it came from -- shows up as a bad diff long after the edit.
    #[test]
    fn cg8_an_edit_keeps_the_methods_real_home_file() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());

        const CLASS: &str = "Beta";
        let path = scratch("edit");
        let img = Image::open(&path).expect("scratch image");
        img.create_or_reopen_class(CLASS, Some("Object"), "", "", "")
            .expect("seed class");
        img.create_or_reopen_method(
            CLASS,
            Side::Instance,
            "tag",
            "as yet unclassified",
            "tag ^1",
        )
        .expect("seed method");
        img.set_method_home_file(CLASS, Side::Instance, "tag", "world/42_real.mst")
            .expect("seed home");

        std::env::set_var("MACVM_IMAGE_PATH", &path);
        let rc = unsafe { MacvmHostSaveMethod(wide(CLASS).as_ptr(), 0, wide("tag ^2").as_ptr()) };
        std::env::remove_var("MACVM_IMAGE_PATH");
        assert_eq!(rc, OK);

        let img = Image::open(&path).expect("reopen");
        assert_eq!(
            img.method_source_file(CLASS, Side::Instance, "tag")
                .unwrap(),
            Some("world/42_real.mst".to_string()),
            "an edit must keep the method's real home file"
        );
        assert_eq!(
            img.method_source(CLASS, Side::Instance, "tag").unwrap(),
            Some("tag ^2".to_string()),
            "and must still have saved the new body"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The class side, because `side` crosses the ABI as a bare integer and
    /// an inverted test would be invisible from either end -- the method just
    /// lands on the other side of the class and cannot be found.
    #[test]
    fn cg8_the_class_side_lands_on_the_class_side() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());

        const CLASS: &str = "Gamma";
        let path = scratch("side");
        let img = Image::open(&path).expect("scratch image");
        img.create_or_reopen_class(CLASS, Some("Object"), "", "", "")
            .expect("seed class");

        std::env::set_var("MACVM_IMAGE_PATH", &path);
        let rc = unsafe {
            MacvmHostSaveMethod(wide(CLASS).as_ptr(), 1, wide("make ^self new").as_ptr())
        };
        std::env::remove_var("MACVM_IMAGE_PATH");
        assert_eq!(rc, OK);

        let img = Image::open(&path).expect("reopen");
        assert_eq!(
            img.method_source(CLASS, Side::Class, "make").unwrap(),
            Some("make ^self new".to_string()),
            "side=1 must mean the CLASS side"
        );
        assert_eq!(
            img.method_source(CLASS, Side::Instance, "make").unwrap(),
            None,
            "and must not also have written the instance side"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A refusal must be `flows`' own words, unedited. The whole value of
    /// sharing the write path is that the Windows GUI reports exactly what
    /// the web GUI would; a message rewritten on the way out would make the
    /// two describe the same refusal differently.
    #[test]
    fn a_refusal_carries_flows_own_wording() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());

        let path = scratch("refuse");
        let img = Image::open(&path).expect("scratch image");
        img.create_or_reopen_class("Delta", Some("Object"), "", "", "")
            .expect("seed class");

        std::env::set_var("MACVM_IMAGE_PATH", &path);
        // Not a message pattern, so `flows` refuses to parse a selector.
        let rc = unsafe { MacvmHostSaveMethod(wide("Delta").as_ptr(), 0, wide("42").as_ptr()) };
        std::env::remove_var("MACVM_IMAGE_PATH");
        assert_eq!(rc, ERR);

        let n = MacvmHostLastMessageLen() as usize;
        let got = unsafe {
            String::from_utf16_lossy(std::slice::from_raw_parts(MacvmHostLastMessage(), n))
        };
        let web = flows::save_method(&img, "Delta", Side::Instance, "42", None).unwrap_err();
        assert_eq!(
            got, web,
            "the Windows refusal must be the web refusal, word for word"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ping_answers_its_sentinel() {
        assert_eq!(
            MacvmHostPing(),
            0x5747,
            "the probe that separates 'the channel is missing' from 'the write \
             was refused' must itself be unmistakable"
        );
    }
}
