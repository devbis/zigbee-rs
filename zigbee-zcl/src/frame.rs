//! ZCL frame parsing and serialization.
//!
//! A ZCL frame consists of a header (frame control, optional manufacturer code,
//! sequence number, command ID) followed by a variable-length payload.

use crate::{ClusterDirection, CommandId};

/// ZCL frame type encoded in bits 0–1 of frame control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZclFrameType {
    /// Global (foundation) command.
    Global = 0x00,
    /// Cluster-specific command.
    ClusterSpecific = 0x01,
}

impl ZclFrameType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val & 0x03 {
            0x00 => Some(Self::Global),
            0x01 => Some(Self::ClusterSpecific),
            _ => None,
        }
    }
}

/// Parsed ZCL frame header.
#[derive(Debug, Clone)]
pub struct ZclFrameHeader {
    /// Raw frame control byte.
    pub frame_control: u8,
    /// Optional manufacturer code (present when bit 2 of frame_control is set).
    pub manufacturer_code: Option<u16>,
    /// Transaction sequence number.
    pub seq_number: u8,
    /// Command identifier.
    pub command_id: CommandId,
}

impl ZclFrameHeader {
    // Frame control bit positions.
    const FC_FRAME_TYPE_MASK: u8 = 0x03;
    const FC_MANUFACTURER_SPECIFIC: u8 = 1 << 2;
    const FC_DIRECTION: u8 = 1 << 3;
    const FC_DISABLE_DEFAULT_RESPONSE: u8 = 1 << 4;

    /// Frame type (global vs. cluster-specific).
    pub fn frame_type(&self) -> ZclFrameType {
        ZclFrameType::from_u8(self.frame_control & Self::FC_FRAME_TYPE_MASK)
            .unwrap_or(ZclFrameType::Global)
    }

    /// Whether the manufacturer code field is present.
    pub fn is_manufacturer_specific(&self) -> bool {
        self.frame_control & Self::FC_MANUFACTURER_SPECIFIC != 0
    }

    /// Command direction.
    pub fn direction(&self) -> ClusterDirection {
        if self.frame_control & Self::FC_DIRECTION != 0 {
            ClusterDirection::ServerToClient
        } else {
            ClusterDirection::ClientToServer
        }
    }

    /// Whether the default response is disabled.
    pub fn disable_default_response(&self) -> bool {
        self.frame_control & Self::FC_DISABLE_DEFAULT_RESPONSE != 0
    }

    /// Build a new frame-control byte from its components.
    pub fn build_frame_control(
        frame_type: ZclFrameType,
        manufacturer_specific: bool,
        direction: ClusterDirection,
        disable_default_response: bool,
    ) -> u8 {
        let mut fc = frame_type as u8;
        if manufacturer_specific {
            fc |= Self::FC_MANUFACTURER_SPECIFIC;
        }
        if matches!(direction, ClusterDirection::ServerToClient) {
            fc |= Self::FC_DIRECTION;
        }
        if disable_default_response {
            fc |= Self::FC_DISABLE_DEFAULT_RESPONSE;
        }
        fc
    }
}

/// Maximum ZCL payload size (conservative for Zigbee frames).
pub const MAX_ZCL_PAYLOAD: usize = 128;

/// A parsed ZCL frame.
#[derive(Debug, Clone)]
pub struct ZclFrame {
    pub header: ZclFrameHeader,
    /// Payload bytes (excluding the header).
    pub payload: heapless::Vec<u8, MAX_ZCL_PAYLOAD>,
}

/// Errors that may occur during ZCL frame parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZclFrameError {
    /// Buffer too short to contain a valid ZCL header.
    TooShort,
    /// Payload exceeds maximum buffer size.
    PayloadTooLarge,
    /// Invalid frame type bits.
    InvalidFrameType,
}

impl ZclFrame {
    /// Parse a ZCL frame from a raw byte slice.
    pub fn parse(data: &[u8]) -> Result<Self, ZclFrameError> {
        if data.len() < 3 {
            return Err(ZclFrameError::TooShort);
        }

        let frame_control = data[0];
        let manufacturer_specific = frame_control & ZclFrameHeader::FC_MANUFACTURER_SPECIFIC != 0;

        let min_header = if manufacturer_specific { 5 } else { 3 };
        if data.len() < min_header {
            return Err(ZclFrameError::TooShort);
        }

        let (manufacturer_code, seq_idx) = if manufacturer_specific {
            let mfr = u16::from_le_bytes([data[1], data[2]]);
            (Some(mfr), 3)
        } else {
            (None, 1)
        };

        let seq_number = data[seq_idx];
        let command_id = CommandId(data[seq_idx + 1]);

        let payload_start = seq_idx + 2;
        let payload_data = &data[payload_start..];

        let mut payload = heapless::Vec::new();
        for &b in payload_data {
            payload
                .push(b)
                .map_err(|_| ZclFrameError::PayloadTooLarge)?;
        }

        Ok(Self {
            header: ZclFrameHeader {
                frame_control,
                manufacturer_code,
                seq_number,
                command_id,
            },
            payload,
        })
    }

    /// Serialize this frame into `buf`, returning the number of bytes written.
    pub fn serialize(&self, buf: &mut [u8]) -> Result<usize, ZclFrameError> {
        let header_len = if self.header.manufacturer_code.is_some() {
            5
        } else {
            3
        };
        let total = header_len + self.payload.len();
        if buf.len() < total {
            return Err(ZclFrameError::TooShort);
        }

        buf[0] = self.header.frame_control;
        let mut idx = 1;

        if let Some(mfr) = self.header.manufacturer_code {
            let bytes = mfr.to_le_bytes();
            buf[idx] = bytes[0];
            buf[idx + 1] = bytes[1];
            idx += 2;
        }

        buf[idx] = self.header.seq_number;
        buf[idx + 1] = self.header.command_id.0;
        idx += 2;

        buf[idx..idx + self.payload.len()].copy_from_slice(&self.payload);
        idx += self.payload.len();

        Ok(idx)
    }

    /// Convenience constructor for a global-command frame.
    pub fn new_global(
        seq: u8,
        command_id: CommandId,
        direction: ClusterDirection,
        disable_default_response: bool,
    ) -> Self {
        Self {
            header: ZclFrameHeader {
                frame_control: ZclFrameHeader::build_frame_control(
                    ZclFrameType::Global,
                    false,
                    direction,
                    disable_default_response,
                ),
                manufacturer_code: None,
                seq_number: seq,
                command_id,
            },
            payload: heapless::Vec::new(),
        }
    }

    /// Convenience constructor for a cluster-specific frame.
    pub fn new_cluster_specific(
        seq: u8,
        command_id: CommandId,
        direction: ClusterDirection,
        disable_default_response: bool,
    ) -> Self {
        Self {
            header: ZclFrameHeader {
                frame_control: ZclFrameHeader::build_frame_control(
                    ZclFrameType::ClusterSpecific,
                    false,
                    direction,
                    disable_default_response,
                ),
                manufacturer_code: None,
                seq_number: seq,
                command_id,
            },
            payload: heapless::Vec::new(),
        }
    }
}
