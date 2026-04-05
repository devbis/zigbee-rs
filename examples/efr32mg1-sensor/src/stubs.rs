//! CI stubs for EFR32MG1P — provides no-op implementations so the build
//! succeeds without real hardware. The pure-Rust driver uses direct register
//! access, but these stubs satisfy the linker for CI artifact generation.
//!
//! Gated behind the `stubs` cargo feature.

// EFR32MG1P uses a pure-Rust radio driver with direct register access.
// No external C FFI symbols need stubbing — the driver compiles natively.
// This module exists only for consistency with other platform examples.
