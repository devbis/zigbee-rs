// build.rs — Telink TLSR8258 sensor build script
//
// When TELINK_SDK_DIR is set, links the Telink driver library.
// Without it, cargo check still works (CI verification).

fn main() {
    if let Ok(sdk_dir) = std::env::var("TELINK_SDK_DIR") {
        let lib_path = format!("{}/platform/lib", sdk_dir);
        println!("cargo:rustc-link-search=native={}", lib_path);
        println!("cargo:rustc-link-lib=static=drivers_8258");

        println!("cargo:rerun-if-env-changed=TELINK_SDK_DIR");
    }
}
