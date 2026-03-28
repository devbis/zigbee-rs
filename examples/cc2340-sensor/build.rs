//! build.rs for cc2340-sensor
//!
//! Links TI's precompiled libraries when CC2340_SDK_DIR is set.
//! Without it, `cargo check` still works for CI verification.

use std::env;

fn main() {
    // Only link TI libraries when SDK path is provided
    if let Ok(sdk_dir) = env::var("CC2340_SDK_DIR") {
        // RCL (Radio Control Layer) library
        println!(
            "cargo:rustc-link-search={}/source/ti/drivers/rcl/lib/ticlang/m0p",
            sdk_dir
        );
        println!("cargo:rustc-link-lib=static=rcl_cc23x0r5");

        // RF firmware patches for IEEE 802.15.4
        println!(
            "cargo:rustc-link-search={}/source/ti/devices/cc23x0r5/rf_patches/lib/ticlang/m0p",
            sdk_dir
        );
        println!("cargo:rustc-link-lib=static=pbe_ieee_cc23x0r5");
        println!("cargo:rustc-link-lib=static=mce_ieee_cc23x0r5");
        println!("cargo:rustc-link-lib=static=rfe_ieee_cc23x0r5");

        // TI Drivers (Power, GPIO, etc.)
        println!(
            "cargo:rustc-link-search={}/source/ti/drivers/lib/ticlang/m0p",
            sdk_dir
        );

        // ZBOSS platform libraries (optional — only if using TI's MAC shim)
        let zb_lib = format!(
            "{}/source/third_party/zigbee/libraries/cc2340r5/ticlang",
            sdk_dir
        );
        if std::path::Path::new(&zb_lib).exists() {
            println!("cargo:rustc-link-search={}", zb_lib);
            println!("cargo:rustc-link-lib=static=zb_ti_platform_zed");
        }
    }

    // Memory layout
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rerun-if-env-changed=CC2340_SDK_DIR");
}
