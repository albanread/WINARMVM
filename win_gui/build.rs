//! WG3 D6 — embed `macvm-winui.manifest` into the binary.
//!
//! Common controls v6 is opt-in per *executable*, and the opt-in is an assembly
//! manifest naming `Microsoft.Windows.Common-Controls` 6.0.0.0. Without it the
//! loader binds comctl32 v5, `InitCommonControlsEx` still succeeds, every
//! control still works, and every control renders in its Windows-95 skin.
//!
//! Done with two linker arguments rather than a new build dependency
//! (`embed-resource`, `winres`): MSVC's `link.exe` embeds a manifest natively
//! with `/MANIFEST:EMBED /MANIFESTINPUT:<file>`, which is exactly what those
//! crates arrange for us, and this crate's whole stated deliverable is its
//! thinness. `rustc-link-arg-bins` applies to the binary only, so nothing this
//! crate's TESTS link is affected.
fn main() {
    #[cfg(windows)]
    {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
        let manifest = std::path::Path::new(&dir).join("macvm-winui.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
        println!("cargo:rerun-if-changed=macvm-winui.manifest");
    }
}
