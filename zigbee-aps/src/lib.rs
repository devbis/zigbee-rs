//! Zigbee PRO R22 Application Support Sub-layer (APS).
//!
//! This crate implements the APS layer of the Zigbee stack, providing:
//! - APS frame construction and parsing
//! - APS Data Entity (APSDE-DATA) service
//! - APS Management Entity (APSME) — binding, group, key management
//! - APS Information Base (AIB)
//! - APS-level security (link key encryption)
//!
//! # Architecture
//! ```text
//! ┌──────────────────────────────────────┐
//! │  ZDO / ZCL / Application             │
//! └──────────────┬───────────────────────┘
//!                │ APSDE-DATA / APSME-*
//! ┌──────────────┴───────────────────────┐
//! │  APS Layer (this crate)              │
//! │  ├── apsde: data service             │
//! │  ├── apsme: management entity        │
//! │  ├── aib: APS information base       │
//! │  ├── frames: APS frame codec         │
//! │  ├── binding: binding table          │
//! │  ├── group: group table              │
//! │  └── security: APS encryption        │
//! └──────────────┬───────────────────────┘
//!                │ NLDE-DATA / NLME-*
//! ┌──────────────┴───────────────────────┐
//! │  NWK Layer (zigbee-nwk)              │
//! └──────────────────────────────────────┘
//! ```

#![no_std]
#![allow(async_fn_in_trait)]

pub mod aib;
pub mod apsde;
pub mod apsme;
pub mod binding;
pub mod frames;
pub mod group;
pub mod security;

use zigbee_mac::MacDriver;
use zigbee_nwk::NwkLayer;

// ── Well-known endpoints ────────────────────────────────────────

/// ZDO endpoint (Zigbee Device Object)
pub const ZDO_ENDPOINT: u8 = 0x00;

/// Minimum application endpoint
pub const MIN_APP_ENDPOINT: u8 = 0x01;

/// Maximum application endpoint
pub const MAX_APP_ENDPOINT: u8 = 0xF0;

/// Broadcast endpoint — delivers to all active endpoints on a device
pub const BROADCAST_ENDPOINT: u8 = 0xFF;

// ── Well-known profile IDs ──────────────────────────────────────

/// Zigbee Device Profile (ZDP)
pub const PROFILE_ZDP: u16 = 0x0000;

/// Home Automation profile
pub const PROFILE_HOME_AUTOMATION: u16 = 0x0104;

/// Smart Energy profile
pub const PROFILE_SMART_ENERGY: u16 = 0x0109;

/// Zigbee Light Link (ZLL) profile
pub const PROFILE_ZLL: u16 = 0xC05E;

/// Wildcard profile — matches any profile
pub const PROFILE_WILDCARD: u16 = 0xFFFF;

// ── APS Status Codes (Zigbee spec Table 2-27) ──────────────────

/// APS layer status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApsStatus {
    /// Request executed successfully
    Success = 0x00,
    /// A transmit request failed since the ASDU is too large and fragmentation
    /// is not supported
    AsduTooLong = 0xA0,
    /// A received fragmented frame could not be defragmented
    DefragDeferred = 0xA1,
    /// A received fragmented frame could not be defragmented because the device
    /// does not support fragmentation
    DefragUnsupported = 0xA2,
    /// A parameter value was out of range
    IllegalRequest = 0xA3,
    /// An APSME-UNBIND.request failed because the requested binding table
    /// entry was not found
    InvalidBinding = 0xA4,
    /// An APSME-GET/SET request was issued with an unknown attribute identifier
    InvalidParameter = 0xA5,
    /// An APSDE-DATA.request requesting acknowledged transmission failed due
    /// to no acknowledgement being received
    NoAck = 0xA6,
    /// An APSDE-DATA.request with a destination addressing mode set to 0x00
    /// failed due to there being no devices bound to this device
    NoBoundDevice = 0xA7,
    /// An APSDE-DATA.request with a destination addressing mode set to 0x03
    /// failed because no matching group table entry could be found
    NoShortAddress = 0xA8,
    /// An APSME-BIND.request or APSME-ADD-GROUP.request issued when the
    /// binding/group table is full
    TableFull = 0xA9,
    /// An ASDU was received that was secured using a link key but a link key
    /// was not found in the key table
    UnsecuredKey = 0xAA,
    /// An APSME-GET.request or APSME-SET.request has been issued with an
    /// unsupported attribute identifier
    UnsupportedAttribute = 0xAB,
    /// An unsecured frame was received
    SecurityFail = 0xAD,
    /// Decryption or authentication of the APS frame failed
    DecryptionError = 0xAE,
    /// Not enough buffers for the requested operation
    InsufficientSpace = 0xAF,
    /// No matching entry in binding table
    NotFound = 0xB0,
}

// ── APS address modes ───────────────────────────────────────────

/// APS addressing modes (Zigbee spec Table 2-3)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApsAddressMode {
    /// Indirect (via binding table)
    Indirect = 0x00,
    /// Group addressing (16-bit group address)
    Group = 0x01,
    /// Direct short (16-bit NWK address + endpoint)
    Short = 0x02,
    /// Direct extended (64-bit IEEE address + endpoint)
    Extended = 0x03,
}

// ── APS address ─────────────────────────────────────────────────

/// Destination/source address used in APS primitives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApsAddress {
    /// 16-bit NWK short address
    Short(zigbee_types::ShortAddress),
    /// 64-bit IEEE extended address
    Extended(zigbee_types::IeeeAddress),
    /// 16-bit group address
    Group(u16),
}

// ── TX Options ──────────────────────────────────────────────────

/// APSDE-DATA.request TX options bitfield.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApsTxOptions {
    /// Use APS-level security (link key encryption)
    pub security_enabled: bool,
    /// Use NWK key (standard NWK encryption)
    pub use_nwk_key: bool,
    /// Request APS acknowledgement
    pub ack_request: bool,
    /// Enable fragmentation
    pub fragmentation_permitted: bool,
    /// Include extended nonce in APS security frame
    pub include_extended_nonce: bool,
}

// ── The APS Layer ───────────────────────────────────────────────

/// The APS layer — owns the NWK layer and all APS state.
///
/// Generic over `M: MacDriver` (the hardware abstraction).
pub struct ApsLayer<M: MacDriver> {
    /// Underlying NWK layer
    nwk: NwkLayer<M>,
    /// APS Information Base
    aib: aib::Aib,
    /// Binding table
    binding_table: binding::BindingTable,
    /// Group table
    group_table: group::GroupTable,
    /// APS security material
    security: security::ApsSecurity,
    /// APS frame counter (outgoing)
    aps_counter: u8,
}

impl<M: MacDriver> ApsLayer<M> {
    /// Create a new APS layer wrapping the given NWK layer.
    pub fn new(nwk: NwkLayer<M>) -> Self {
        Self {
            nwk,
            aib: aib::Aib::new(),
            binding_table: binding::BindingTable::new(),
            group_table: group::GroupTable::new(),
            security: security::ApsSecurity::new(),
            aps_counter: 0,
        }
    }

    /// Get the next APS counter value (wrapping).
    pub fn next_aps_counter(&mut self) -> u8 {
        let c = self.aps_counter;
        self.aps_counter = self.aps_counter.wrapping_add(1);
        c
    }

    /// Reference to the underlying NWK layer.
    pub fn nwk(&self) -> &NwkLayer<M> {
        &self.nwk
    }

    /// Mutable reference to the underlying NWK layer.
    pub fn nwk_mut(&mut self) -> &mut NwkLayer<M> {
        &mut self.nwk
    }

    /// Reference to the APS Information Base.
    pub fn aib(&self) -> &aib::Aib {
        &self.aib
    }

    /// Mutable reference to the APS Information Base.
    pub fn aib_mut(&mut self) -> &mut aib::Aib {
        &mut self.aib
    }

    /// Reference to the binding table.
    pub fn binding_table(&self) -> &binding::BindingTable {
        &self.binding_table
    }

    /// Mutable reference to the binding table.
    pub fn binding_table_mut(&mut self) -> &mut binding::BindingTable {
        &mut self.binding_table
    }

    /// Reference to the group table.
    pub fn group_table(&self) -> &group::GroupTable {
        &self.group_table
    }

    /// Mutable reference to the group table.
    pub fn group_table_mut(&mut self) -> &mut group::GroupTable {
        &mut self.group_table
    }

    /// Reference to APS security state.
    pub fn security(&self) -> &security::ApsSecurity {
        &self.security
    }

    /// Mutable reference to APS security state.
    pub fn security_mut(&mut self) -> &mut security::ApsSecurity {
        &mut self.security
    }
}
