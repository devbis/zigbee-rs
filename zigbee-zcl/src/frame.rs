//! ZCL frame parsing and serialization.
//!
//! A ZCL frame consists of a header (frame control, optional manufacturer code,
//! sequence number, command ID) followed by a variable-length payload.

use crate::{ClusterDirection, CommandId, ZclStatus};

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
    ///
    /// Frames with a reserved frame type are rejected by [`ZclFrame::parse`];
    /// for a hand-built header carrying reserved bits this returns
    /// `ClusterSpecific` so the frame is never treated as a foundation
    /// command. Use [`try_frame_type`](Self::try_frame_type) to detect them.
    pub fn frame_type(&self) -> ZclFrameType {
        self.try_frame_type()
            .unwrap_or(ZclFrameType::ClusterSpecific)
    }

    /// Frame type, or `None` for the reserved values 0b10/0b11.
    pub fn try_frame_type(&self) -> Option<ZclFrameType> {
        ZclFrameType::from_u8(self.frame_control & Self::FC_FRAME_TYPE_MASK)
    }

    /// Status for a manufacturer-specific frame the receiver does not
    /// implement (ZCL r8 §2.5.12.4): `UNSUP_MANUF_GENERAL_COMMAND` for global
    /// frames, `UNSUP_MANUF_CLUSTER_COMMAND` for cluster-specific ones.
    /// Returns `None` for standard (non-manufacturer-specific) frames.
    ///
    /// Dispatchers without manufacturer extensions must answer such frames
    /// with this status instead of processing them as standard commands.
    pub fn manufacturer_specific_unsupported_status(&self) -> Option<ZclStatus> {
        if !self.is_manufacturer_specific() {
            return None;
        }
        Some(match self.frame_type() {
            ZclFrameType::Global => ZclStatus::UnsupManufacturerGeneralCommand,
            ZclFrameType::ClusterSpecific => ZclStatus::UnsupManufacturerClusterCommand,
        })
    }

    /// Whether a Default Response carrying `status` must be sent for this
    /// received command (ZCL r8 §2.5.12.2 / §2.4.1.1.4).
    ///
    /// The disable-default-response bit only suppresses *successful* Default
    /// Responses: an error status is always reported. A Default Response is
    /// never sent in reply to a Default Response.
    pub fn default_response_required(&self, status: ZclStatus) -> bool {
        let is_default_rsp = self.frame_type() == ZclFrameType::Global
            && self.command_id.0 == crate::foundation::FoundationCommandId::DefaultResponse as u8;
        !is_default_rsp && (status != ZclStatus::Success || !self.disable_default_response())
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
        if ZclFrameType::from_u8(frame_control).is_none() {
            return Err(ZclFrameError::InvalidFrameType);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_frame_types_are_rejected() {
        for fc in [0x02u8, 0x03, 0x12, 0x1B] {
            assert_eq!(
                ZclFrame::parse(&[fc, 1, 0x00]).err(),
                Some(ZclFrameError::InvalidFrameType)
            );
        }
        let f = ZclFrame::parse(&[0x00, 1, 0x00]).unwrap();
        assert_eq!(f.header.try_frame_type(), Some(ZclFrameType::Global));
        let hand_built = ZclFrameHeader {
            frame_control: 0x02,
            manufacturer_code: None,
            seq_number: 0,
            command_id: CommandId(0),
        };
        assert_eq!(hand_built.try_frame_type(), None);
        assert_eq!(hand_built.frame_type(), ZclFrameType::ClusterSpecific);
    }

    #[test]
    fn manufacturer_specific_frames_report_unsupported_status() {
        let g = ZclFrame::parse(&[0x04, 0x34, 0x12, 1, 0x00]).unwrap();
        assert_eq!(g.header.manufacturer_code, Some(0x1234));
        assert_eq!(
            g.header.manufacturer_specific_unsupported_status(),
            Some(ZclStatus::UnsupManufacturerGeneralCommand)
        );
        let c = ZclFrame::parse(&[0x05, 0x34, 0x12, 1, 0x00]).unwrap();
        assert_eq!(
            c.header.manufacturer_specific_unsupported_status(),
            Some(ZclStatus::UnsupManufacturerClusterCommand)
        );
        let std = ZclFrame::parse(&[0x01, 1, 0x00]).unwrap();
        assert_eq!(std.header.manufacturer_specific_unsupported_status(), None);
    }

    #[test]
    fn disable_default_response_only_suppresses_success() {
        let ddr = ZclFrame::parse(&[0x11, 1, 0x00]).unwrap().header;
        assert!(!ddr.default_response_required(ZclStatus::Success));
        assert!(ddr.default_response_required(ZclStatus::UnsupClusterCommand));
        let plain = ZclFrame::parse(&[0x01, 1, 0x00]).unwrap().header;
        assert!(plain.default_response_required(ZclStatus::Success));
        // Never answer a Default Response with a Default Response.
        let dr = ZclFrame::parse(&[0x00, 1, 0x0B, 0x00, 0x81])
            .unwrap()
            .header;
        assert!(!dr.default_response_required(ZclStatus::Failure));
    }
}
