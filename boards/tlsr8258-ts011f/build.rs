//! Emit the flash layout linker script for the selected board flash size.
//!
//! The flash map is a *board setting*: which linker script (and, in
//! `storage.rs`, which partition offsets) to use is chosen by this crate's
//! `flash-512k` (default) / `flash-1m` Cargo features. The chosen script is
//! copied to `OUT_DIR/memory.x` and that directory is added to the linker
//! search path, so a downstream binary links it with `-Tmemory.x`.

use std::{env, fs, path::PathBuf};

fn main() {
    // `flash-1m` wins if (mis)configured with both; `storage.rs` additionally
    // emits a hard compile_error for that case so the two never diverge.
    let script = if env::var_os("CARGO_FEATURE_FLASH_1M").is_some() {
        "memory-1m.x"
    } else {
        "memory-512k.x"
    };

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    fs::copy(script, out.join("memory.x"))
        .unwrap_or_else(|e| panic!("failed to copy {script} to OUT_DIR/memory.x: {e}"));

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory-512k.x");
    println!("cargo:rerun-if-changed=memory-1m.x");
    println!("cargo:rerun-if-changed=build.rs");
}
