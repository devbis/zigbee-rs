//! APS frame construction and parsing.
//!
//! Implements APS frame header encoding/decoding per Zigbee PRO R22 spec
//! Chapter 2.2.5. The APS frame sits inside the NWK payload.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ APS Frame Control (1 byte)                                  │
//! │  ├── Frame Type       (bits 0-1)                            │
//! │  ├── Delivery Mode    (bits 2-3)                            │
//! │  ├── Ack Format       (bit 4)                               │
//! │  ├── Security         (bit 5)                               │
//! │  ├── Ack Request      (bit 6)                               │
//! │  └── Extended Header  (bit 7)                               │
//! ├─────────────────────────────────────────────────────────────┤
//! │ Dst Endpoint (1 byte) — present if unicast/ack              │
//! │ Group Address (2 bytes LE) — present if group delivery      │
//! │ Cluster ID (2 bytes LE) — present if data/ack               │
//! │ Profile ID (2 bytes LE) — present if data/ack               │
//! │ Src Endpoint (1 byte) — present if data/ack/cmd             │
//! │ APS Counter (1 byte)                                        │
//! │ [Extended Header] — if ext_header bit set                   │
//! └─────────────────────────────────────────────────────────────┘
//! ```

// ── Frame types (2 bits) ────────────────────────────────────────

/// APS frame types (Zigbee spec Table 2-20)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApsFrameType {
    /// Data frame — carries application data
    Data = 0x00,
    /// APS command frame — carries APS commands (key transport, etc.)
    Command = 0x01,
    /// Acknowledgement frame
    Ack = 0x02,
    /// Inter-PAN frame (Zigbee 3.0 touchlink)
    InterPan = 0x03,
}

impl ApsFrameType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x00 => Some(Self::Data),
            0x01 => Some(Self::Command),
            0x02 => Some(Self::Ack),
            0x03 => Some(Self::InterPan),
            _ => None,
        }
    }
}

// ── Delivery modes (2 bits) ─────────────────────────────────────

/// APS delivery modes (Zigbee spec Table 2-21)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApsDeliveryMode {
    /// Normal unicast delivery
    Unicast = 0x00,
    /// Indirect delivery (via binding table on coordinator)
    Indirect = 0x01,
    /// Broadcast delivery
    Broadcast = 0x02,
    /// Group delivery (multicast)
    Group = 0x03,
}

impl ApsDeliveryMode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x00 => Some(Self::Unicast),
            0x01 => Some(Self::Indirect),
            0x02 => Some(Self::Broadcast),
            0x03 => Some(Self::Group),
            _ => None,
        }
    }
}

// ── APS Frame Control (1 byte) ──────────────────────────────────

/// APS frame control field (8 bits, Zigbee spec Figure 2-4)
///
/// Bit layout:
/// - Bits 0-1: Frame Type
/// - Bits 2-3: Delivery Mode
/// - Bit 4:    Ack Format (0 = data frame ack, 1 = APS command ack)
/// - Bit 5:    Security (APS-level encryption enabled)
/// - Bit 6:    Ack Request
/// - Bit 7:    Extended Header Present
#[derive(Debug, Clone, Copy, Default)]
pub struct ApsFrameControl {
    pub frame_type: u8,
    pub delivery_mode: u8,
    pub ack_format: bool,
    pub security: bool,
    pub ack_request: bool,
    pub extended_header: bool,
}

impl ApsFrameControl {
    /// Parse from a single byte.
    pub fn parse(raw: u8) -> Self {
        Self {
            frame_type: raw & 0x03,
            delivery_mode: (raw >> 2) & 0x03,
            ack_format: (raw >> 4) & 1 != 0,
            security: (raw >> 5) & 1 != 0,
            ack_request: (raw >> 6) & 1 != 0,
            extended_header: (raw >> 7) & 1 != 0,
        }
    }

    /// Serialize to a single byte.
    pub fn serialize(&self) -> u8 {
        let mut fc: u8 = 0;
        fc |= self.frame_type & 0x03;
        fc |= (self.delivery_mode & 0x03) << 2;
        if self.ack_format {
            fc |= 1 << 4;
        }
        if self.security {
            fc |= 1 << 5;
        }
        if self.ack_request {
            fc |= 1 << 6;
        }
        if self.extended_header {
            fc |= 1 << 7;
        }
        fc
    }
}

// ── APS Extended Header ─────────────────────────────────────────

/// APS extended header sub-frame (Zigbee spec 2.2.5.1.8)
///
/// Present when the Extended Header bit is set in the frame control.
/// Used for fragmentation.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApsExtendedHeader {
    /// Extended frame control
    ///   Bits 0-1: Fragmentation (0=none, 1=first fragment, 2=subsequent)
    pub fragmentation: u8,
    /// Block number (fragment index), present when fragmentation != 0
    pub block_number: u8,
    /// Ack bitfield (for fragment acks), present for first fragment
    pub ack_bitfield: Option<u8>,
}

/// Fragmentation sub-field values
pub const FRAG_NONE: u8 = 0x00;
pub const FRAG_FIRST: u8 = 0x01;
pub const FRAG_SUBSEQUENT: u8 = 0x02;

// ── APS Header ──────────────────────────────────────────────────

/// Complete APS header (Zigbee spec 2.2.5.1).
///
/// Field presence depends on frame type and delivery mode:
/// - `dst_endpoint`: present for Unicast/Broadcast data & ack frames
/// - `group_address`: present for Group delivery mode
/// - `cluster_id`: present for Data and Ack frames
/// - `profile_id`: present for Data and Ack frames
/// - `src_endpoint`: present for Data, Ack, and Command frames
/// - `aps_counter`: always present
#[derive(Debug, Clone, Default)]
pub struct ApsHeader {
    pub frame_control: ApsFrameControl,
    /// Destination endpoint (0x00 = ZDO, 0x01-0xF0 = app, 0xFF = broadcast)
    pub dst_endpoint: Option<u8>,
    /// Group address (for group delivery mode)
    pub group_address: Option<u16>,
    /// Cluster identifier
    pub cluster_id: Option<u16>,
    /// Profile identifier
    pub profile_id: Option<u16>,
    /// Source endpoint
    pub src_endpoint: Option<u8>,
    /// APS counter (sequence number)
    pub aps_counter: u8,
    /// Extended header (fragmentation)
    pub extended_header: Option<ApsExtendedHeader>,
}

impl ApsHeader {
    /// Parse an APS header from raw bytes. Returns (header, bytes_consumed).
    pub fn parse(data: &[u8]) -> Option<(Self, usize)> {
        if data.is_empty() {
            return None;
        }

        let fc = ApsFrameControl::parse(data[0]);
        let mut offset = 1;

        let frame_type = ApsFrameType::from_u8(fc.frame_type)?;
        let delivery_mode = ApsDeliveryMode::from_u8(fc.delivery_mode)?;

        // Destination endpoint: present for unicast/broadcast data and ack frames
        let dst_endpoint = match frame_type {
            ApsFrameType::Data | ApsFrameType::Ack => match delivery_mode {
                ApsDeliveryMode::Unicast | ApsDeliveryMode::Broadcast => {
                    if data.len() <= offset {
                        return None;
                    }
                    let ep = data[offset];
                    offset += 1;
                    Some(ep)
                }
                _ => None,
            },
            _ => None,
        };

        // Group address: present for group delivery mode
        let group_address = if delivery_mode == ApsDeliveryMode::Group {
            if data.len() < offset + 2 {
                return None;
            }
            let g = u16::from_le_bytes([data[offset], data[offset + 1]]);
            offset += 2;
            Some(g)
        } else {
            None
        };

        // Cluster ID: present for data and ack frames
        let cluster_id = match frame_type {
            ApsFrameType::Data | ApsFrameType::Ack => {
                if data.len() < offset + 2 {
                    return None;
                }
                let c = u16::from_le_bytes([data[offset], data[offset + 1]]);
                offset += 2;
                Some(c)
            }
            _ => None,
        };

        // Profile ID: present for data and ack frames
        let profile_id = match frame_type {
            ApsFrameType::Data | ApsFrameType::Ack => {
                if data.len() < offset + 2 {
                    return None;
                }
                let p = u16::from_le_bytes([data[offset], data[offset + 1]]);
                offset += 2;
                Some(p)
            }
            _ => None,
        };

        // Source endpoint: present for all except InterPan
        // For group delivery in data frames, src_endpoint is also present
        let src_endpoint = match frame_type {
            ApsFrameType::Data | ApsFrameType::Ack | ApsFrameType::Command => {
                if data.len() <= offset {
                    return None;
                }
                let ep = data[offset];
                offset += 1;
                Some(ep)
            }
            ApsFrameType::InterPan => None,
        };

        // APS counter: always present
        if data.len() <= offset {
            return None;
        }
        let aps_counter = data[offset];
        offset += 1;

        // Extended header (fragmentation)
        let extended_header = if fc.extended_header {
            if data.len() <= offset {
                return None;
            }
            let ext_fc = data[offset];
            offset += 1;
            let fragmentation = ext_fc & 0x03;
            let (block_number, ack_bitfield) = if fragmentation != FRAG_NONE {
                if data.len() <= offset {
                    return None;
                }
                let bn = data[offset];
                offset += 1;
                let abf = if fragmentation == FRAG_FIRST && data.len() > offset {
                    let a = data[offset];
                    offset += 1;
                    Some(a)
                } else {
                    None
                };
                (bn, abf)
            } else {
                (0, None)
            };
            Some(ApsExtendedHeader {
                fragmentation,
                block_number,
                ack_bitfield,
            })
        } else {
            None
        };

        Some((
            Self {
                frame_control: fc,
                dst_endpoint,
                group_address,
                cluster_id,
                profile_id,
                src_endpoint,
                aps_counter,
                extended_header,
            },
            offset,
        ))
    }

    /// Serialize the APS header into a buffer. Returns bytes written.
    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        let mut offset = 0;

        buf[offset] = self.frame_control.serialize();
        offset += 1;

        if let Some(ep) = self.dst_endpoint {
            buf[offset] = ep;
            offset += 1;
        }

        if let Some(g) = self.group_address {
            let bytes = g.to_le_bytes();
            buf[offset] = bytes[0];
            buf[offset + 1] = bytes[1];
            offset += 2;
        }

        if let Some(c) = self.cluster_id {
            let bytes = c.to_le_bytes();
            buf[offset] = bytes[0];
            buf[offset + 1] = bytes[1];
            offset += 2;
        }

        if let Some(p) = self.profile_id {
            let bytes = p.to_le_bytes();
            buf[offset] = bytes[0];
            buf[offset + 1] = bytes[1];
            offset += 2;
        }

        if let Some(ep) = self.src_endpoint {
            buf[offset] = ep;
            offset += 1;
        }

        buf[offset] = self.aps_counter;
        offset += 1;

        if let Some(ref ext) = self.extended_header {
            buf[offset] = ext.fragmentation & 0x03;
            offset += 1;
            if ext.fragmentation != FRAG_NONE {
                buf[offset] = ext.block_number;
                offset += 1;
                if let Some(abf) = ext.ack_bitfield {
                    buf[offset] = abf;
                    offset += 1;
                }
            }
        }

        offset
    }
}

// ── APS Command IDs (Zigbee spec Table 4-9) ────────────────────

/// APS command identifiers (carried in APS command frames).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApsCommandId {
    TransportKey = 0x05,
    UpdateDevice = 0x06,
    RemoveDevice = 0x07,
    RequestKey = 0x08,
    SwitchKey = 0x09,
    /// Tunnel command (for transporting frames across the network)
    Tunnel = 0x0E,
    VerifyKey = 0x0F,
    ConfirmKey = 0x10,
}

impl ApsCommandId {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x05 => Some(Self::TransportKey),
            0x06 => Some(Self::UpdateDevice),
            0x07 => Some(Self::RemoveDevice),
            0x08 => Some(Self::RequestKey),
            0x09 => Some(Self::SwitchKey),
            0x0E => Some(Self::Tunnel),
            0x0F => Some(Self::VerifyKey),
            0x10 => Some(Self::ConfirmKey),
            _ => None,
        }
    }
}
