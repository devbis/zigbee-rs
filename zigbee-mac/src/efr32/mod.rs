//! EFR32MG24 MAC backend (Tier 2 — requires C FFI).
//!
//! Silicon Labs EFR32MG24 uses the RAIL (Radio Abstraction Interface Layer)
//! C library for 802.15.4 radio access. This backend wraps RAIL via FFI.
//!
//! # Architecture
//! ```text
//! ┌─────────────────────────────┐
//! │  Rust: MacDriver trait impl │
//! │  Frame construction/parsing │
//! └────────────┬────────────────┘
//!              │ extern "C" FFI
//! ┌────────────┴────────────────┐
//! │  C: RAIL 802.15.4 library   │
//! │  (from GSDK, linked at      │
//! │   build time)                │
//! └─────────────────────────────┘
//! ```
//!
//! # Dependencies
//! - `MG24-HAL` (Rust HAL for GPIO/SPI/etc)
//! - `EFR32MG2X-RS` PAC (register-level access)
//! - GSDK RAIL library (C, linked via build.rs)
//!
//! # Hardware
//! - Seeed XIAO MG24 (~$6)
//! - SparkFun Thing Plus MGM240P
//! - Silicon Labs xG24 Dev Kit

use crate::pib::{PibAttribute, PibValue};
use crate::primitives::*;
use crate::{MacCapabilities, MacDriver, MacError};
use zigbee_types::*;

/// EFR32MG24 MAC driver via RAIL FFI.
pub struct Efr32Mac {
    seq_number: u8,
    short_address: ShortAddress,
    pan_id: PanId,
    channel: u8,
}

impl Efr32Mac {
    pub fn new() -> Self {
        Self {
            seq_number: 0,
            short_address: ShortAddress(0xFFFF),
            pan_id: PanId(0xFFFF),
            channel: 11,
        }
    }
}

// TODO: Implement MacDriver for Efr32Mac
//
// RAIL FFI bindings needed:
// - RAIL_IEEE802154_Init() — configure 802.15.4 mode
// - RAIL_IEEE802154_SetPanId() / SetShortAddress()
// - RAIL_StartTx() / RAIL_StartRx()
// - RAIL_IEEE802154_SetPromiscuousMode()
// - Callbacks: RAIL_EVENT_TX_PACKET_SENT, RAIL_EVENT_RX_PACKET_RECEIVED
//
// build.rs must:
// 1. Find GSDK installation path
// 2. Compile RAIL shim C file
// 3. Link librail_efr32xg24.a
//
// Reference: GSDK/platform/radio/rail_lib/
