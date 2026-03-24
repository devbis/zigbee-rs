//! STM32WB55 MAC backend.
//!
//! The STM32WB55 is a dual-core MCU: Cortex-M4 (application) + Cortex-M0+
//! (radio coprocessor). The M0+ runs ST's proprietary 802.15.4 firmware.
//! Communication between cores uses IPCC (Inter-Processor Communication
//! Controller) mailbox.
//!
//! This backend uses `embassy-stm32-wpan` which wraps the IPCC interface
//! and provides a Rust-friendly 802.15.4 MAC API.
//!
//! # Architecture
//! ```text
//! ┌──────────────────────┐    ┌──────────────────────┐
//! │  Cortex-M4 (App)     │    │  Cortex-M0+ (Radio)  │
//! │  zigbee-rs + Rust    │◄──►│  ST 802.15.4 FW      │
//! │  NWK / APS / ZCL     │IPCC│  MAC / PHY           │
//! └──────────────────────┘    └──────────────────────┘
//! ```
//!
//! # Dependencies
//! - `embassy-stm32` with wpan feature
//! - STM32WB55 coprocessor firmware flashed via STM32CubeProgrammer
//!
//! # Hardware
//! - STM32WB55 Nucleo board (~$25)
//! - P-NUCLEO-WB55 (USB Dongle variant)

use crate::pib::{PibAttribute, PibValue};
use crate::primitives::*;
use crate::{MacCapabilities, MacDriver, MacError};
use zigbee_types::*;

/// STM32WB55 802.15.4 MAC driver.
///
/// Talks to the M0+ coprocessor via IPCC mailbox. The coprocessor handles
/// all PHY/MAC timing, CSMA-CA, ACK, and address filtering in hardware.
pub struct Stm32wbMac {
    seq_number: u8,
    short_address: ShortAddress,
    pan_id: PanId,
    channel: u8,
}

impl Stm32wbMac {
    pub fn new() -> Self {
        Self {
            seq_number: 0,
            short_address: ShortAddress(0xFFFF),
            pan_id: PanId(0xFFFF),
            channel: 11,
        }
    }
}

// TODO: Implement MacDriver for Stm32wbMac
//
// Implementation notes:
// - embassy-stm32-wpan provides Mac802154 struct with:
//   - mlme_set_req(), mlme_get_req()
//   - mlme_scan_req(), mlme_associate_req()
//   - mcps_data_req()
// - These are already close to IEEE 802.15.4 primitive names!
// - Main work: adapt embassy-stm32-wpan types to our MacDriver trait types
// - The coprocessor firmware handles timing-critical MAC operations
// - We just need to bridge the IPCC message format
//
// Reference: embassy-rs/embassy/examples/stm32wb/src/bin/mac_ffd.rs
