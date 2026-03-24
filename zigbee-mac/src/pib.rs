//! MAC PAN Information Base (PIB) attributes and values.
//!
//! PIB attributes are the configuration interface between the Zigbee NWK layer
//! and the MAC. The NWK layer uses MLME-GET/SET to read and write these.

use zigbee_types::{IeeeAddress, PanId, ShortAddress};

/// MAC PIB attribute identifiers (IEEE 802.15.4 Table 8-82)
///
/// Only attributes actually used by Zigbee PRO R22 are included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PibAttribute {
    // ── Addressing (critical — set during join) ─────────────
    /// Own 16-bit short address. Default: 0xFFFF (unassigned)
    MacShortAddress = 0x53,
    /// PAN ID of the network we're in. Default: 0xFFFF (not associated)
    MacPanId = 0x50,
    /// Own 64-bit IEEE address (read from hardware, usually read-only)
    MacExtendedAddress = 0x6F,
    /// Short address of our parent coordinator/router
    MacCoordShortAddress = 0x4B,
    /// Extended address of our parent coordinator/router
    MacCoordExtendedAddress = 0x4A,

    // ── Network configuration ───────────────────────────────
    /// True if this device is the PAN coordinator
    MacAssociatedPanCoord = 0x56,
    /// RX enabled during idle (true for ZC/ZR, false for sleepy ZED)
    MacRxOnWhenIdle = 0x52,
    /// True = accepting association requests (join permit open)
    MacAssociationPermit = 0x41,

    // ── Beacon (always 15/15 for Zigbee non-beacon mode) ────
    /// Beacon order. ALWAYS 15 for Zigbee PRO (non-beacon mode)
    MacBeaconOrder = 0x47,
    /// Superframe order. ALWAYS 15 for Zigbee PRO
    MacSuperframeOrder = 0x54,
    /// Beacon payload bytes (NWK beacon content for ZC/ZR)
    MacBeaconPayload = 0x45,
    /// Length of beacon payload
    MacBeaconPayloadLength = 0x46,

    // ── TX/RX tuning ────────────────────────────────────────
    /// Auto data-request after beacon with pending bit (ZED)
    MacAutoRequest = 0x42,
    /// Max CSMA-CA backoffs (default 4)
    MacMaxCsmaBackoffs = 0x4E,
    /// Min backoff exponent (default 3 for 2.4 GHz)
    MacMinBe = 0x4F,
    /// Max backoff exponent (default 5)
    MacMaxBe = 0x57,
    /// Max frame retries after ACK failure (default 3)
    MacMaxFrameRetries = 0x59,
    /// Max wait for indirect TX frame (symbols)
    MacMaxFrameTotalWaitTime = 0x58,
    /// Response wait time for association etc
    MacResponseWaitTime = 0x5A,

    // ── Sequence numbers ────────────────────────────────────
    /// Data/command frame sequence number
    MacDsn = 0x4C,
    /// Beacon sequence number
    MacBsn = 0x49,

    // ── Indirect TX (ZC/ZR) ─────────────────────────────────
    /// How long coordinator stores indirect frames (symbols)
    MacTransactionPersistenceTime = 0x55,

    // ── Debug / special ─────────────────────────────────────
    /// Promiscuous mode (sniffer use)
    MacPromiscuousMode = 0x51,

    // ── PHY attributes (accessed via MAC GET/SET) ───────────
    /// Current channel (11-26 for 2.4 GHz)
    PhyCurrentChannel = 0x00,
    /// Supported channels bitmask (0x07FFF800 for 2.4 GHz)
    PhyChannelsSupported = 0x01,
    /// TX power in dBm
    PhyTransmitPower = 0x02,
    /// CCA mode
    PhyCcaMode = 0x03,
    /// Channel page (always 0 for 2.4 GHz Zigbee)
    PhyCurrentPage = 0x04,
}

/// Value container for PIB GET/SET operations
#[derive(Debug, Clone)]
pub enum PibValue {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    I8(i8),
    ShortAddress(ShortAddress),
    PanId(PanId),
    ExtendedAddress(IeeeAddress),
    /// Variable-length beacon payload (max 52 bytes for Zigbee)
    Payload(PibPayload),
}

/// Fixed-capacity beacon payload buffer
#[derive(Debug, Clone)]
pub struct PibPayload {
    buf: [u8; 52],
    len: usize,
}

impl PibPayload {
    pub fn new() -> Self {
        Self {
            buf: [0u8; 52],
            len: 0,
        }
    }

    pub fn from_slice(data: &[u8]) -> Option<Self> {
        if data.len() > 52 {
            return None;
        }
        let mut p = Self::new();
        p.buf[..data.len()].copy_from_slice(data);
        p.len = data.len();
        Some(p)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Default for PibPayload {
    fn default() -> Self {
        Self::new()
    }
}

// ── Convenience conversions ─────────────────────────────────────

impl PibValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_u8(&self) -> Option<u8> {
        match self {
            Self::U8(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_u16(&self) -> Option<u16> {
        match self {
            Self::U16(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::U32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_short_address(&self) -> Option<ShortAddress> {
        match self {
            Self::ShortAddress(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_pan_id(&self) -> Option<PanId> {
        match self {
            Self::PanId(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_extended_address(&self) -> Option<IeeeAddress> {
        match self {
            Self::ExtendedAddress(v) => Some(*v),
            _ => None,
        }
    }
}

// ── PHY constants ───────────────────────────────────────────────

/// Base superframe duration in symbols (960)
pub const A_BASE_SUPERFRAME_DURATION: u32 = 960;

/// Symbol rate at 2.4 GHz in symbols/second
pub const SYMBOL_RATE_2_4GHZ: u32 = 62_500;

/// Calculate scan duration per channel in symbols
pub fn scan_duration_symbols(exponent: u8) -> u32 {
    A_BASE_SUPERFRAME_DURATION * ((1u32 << exponent) + 1)
}

/// Calculate scan duration per channel in microseconds
pub fn scan_duration_us(exponent: u8) -> u64 {
    let symbols = scan_duration_symbols(exponent) as u64;
    symbols * 1_000_000 / SYMBOL_RATE_2_4GHZ as u64
}
