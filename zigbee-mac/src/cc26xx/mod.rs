//! TI CC2652 MAC backend (Tier 2 — requires RF core driver).
//!
//! The CC2652 uses a dedicated RF core (Cortex-M0) that runs TI's
//! proprietary radio firmware. The application core communicates with
//! the RF core via a command/mailbox interface (RFC doorbell).
//!
//! # Architecture
//! ```text
//! ┌──────────────────────────────┐
//! │  Cortex-M4F (Application)    │
//! │  Rust: MacDriver trait impl  │
//! │  Frame construction/parsing  │
//! └────────────┬─────────────────┘
//!              │ RFC doorbell (shared RAM commands)
//! ┌────────────┴─────────────────┐
//! │  Cortex-M0 (RF Core)         │
//! │  TI RF firmware: PHY timing, │
//! │  CSMA-CA, auto-ACK           │
//! └──────────────────────────────┘
//! ```
//!
//! # Dependencies
//! - `cc13x2_26x2_pac` (SVD-generated register access)
//! - RF core patch blobs from TI SimpleLink SDK
//!
//! # Hardware
//! - TI CC2652R LaunchPad (~$30)
//! - TI CC2652P (with PA, +20 dBm)
//! - Many commercial Zigbee coordinators use CC2652

use crate::pib::{PibAttribute, PibValue};
use crate::primitives::*;
use crate::{MacCapabilities, MacDriver, MacError};
use zigbee_types::*;

/// TI CC2652 MAC driver via RF core command interface.
pub struct Cc26xxMac {
    seq_number: u8,
    short_address: ShortAddress,
    pan_id: PanId,
    channel: u8,
}

impl Cc26xxMac {
    pub fn new() -> Self {
        Self {
            seq_number: 0,
            short_address: ShortAddress(0xFFFF),
            pan_id: PanId(0xFFFF),
            channel: 11,
        }
    }
}

// TODO: Implement MacDriver for Cc26xxMac
//
// RF core command interface:
// - CMD_IEEE_RX: start RX on a channel
// - CMD_IEEE_TX: transmit a frame
// - CMD_IEEE_ED_SCAN: energy detection
// - CMD_IEEE_CCA_REQ: clear channel assessment
// - CMD_IEEE_ABORT: stop current operation
//
// The RF core handles:
// - CSMA-CA (hardware)
// - Auto-ACK (hardware)
// - Frame filtering by PAN ID / short address
// - CRC computation
//
// We need to:
// 1. Load RF core patches (binary blobs from TI SDK)
// 2. Set up command structures in shared RAM
// 3. Post commands via RFC doorbell register
// 4. Handle command-done interrupts
//
// Reference: TI CC26xx Technical Reference Manual, Chapter 23 (RF Core)
