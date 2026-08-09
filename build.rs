//! Build script — exists solely to compile the Cocoa bridge's Objective-C
//! exception shim (docs/cocoa_bridge_design.md §5): an `NSException`
//! unwinding through Rust/JIT frames is undefined behavior, so every
//! bridged `objc_msgSend` goes through a tiny `.m` that CALLS the send
//! inside `@try` and reports a caught exception as a status + description
//! instead of unwinding.
//!
//! WINARM (P0 D2#1): gated to macOS — there is no Cocoa bridge to protect on
//! Windows, so the shim is not built and `objc` is not linked. A build
//! script's `cfg` is the HOST's, not the target's; that is correct here
//! because host == target on both machines in this pair (macOS/arm64 and
//! Windows/arm64 each build natively). Cross-compiling would need
//! `CARGO_CFG_TARGET_OS` instead.
fn main() {
    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .file("src/runtime/objc_shim.m")
            .flag("-fobjc-exceptions")
            .compile("macvm_objc_shim");
        println!("cargo:rustc-link-lib=objc");
    }
    println!("cargo:rerun-if-changed=src/runtime/objc_shim.m");
}
