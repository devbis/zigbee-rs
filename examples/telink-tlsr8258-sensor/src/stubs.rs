//! CI link stubs for critical-section symbols.
//!
//! Provides critical-section implementations for thumbv6m CI builds where
//! LTO strips the cortex-m critical-section implementation. No radio FFI
//! stubs are needed — the radio driver uses pure-Rust register access.

// Critical-section stubs for thumbv6m CI builds (cortex-m's implementation
// is stripped by LTO since nothing in the same codegen unit references it).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _critical_section_1_0_acquire() -> bool {
    false
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _critical_section_1_0_release(_restore_state: bool) {}
