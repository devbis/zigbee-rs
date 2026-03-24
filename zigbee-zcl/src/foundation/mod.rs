//! ZCL foundation (global) commands — the command set shared by all clusters.

pub mod default_response;
pub mod discover;
pub mod read_attributes;
pub mod reporting;
pub mod write_attributes;

/// Foundation command identifiers (ZCL Rev 8, Table 2-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FoundationCommandId {
    ReadAttributes = 0x00,
    ReadAttributesResponse = 0x01,
    WriteAttributes = 0x02,
    WriteAttributesUndivided = 0x03,
    WriteAttributesResponse = 0x04,
    WriteAttributesNoResponse = 0x05,
    ConfigureReporting = 0x06,
    ConfigureReportingResponse = 0x07,
    ReadReportingConfig = 0x08,
    ReadReportingConfigResponse = 0x09,
    ReportAttributes = 0x0A,
    DefaultResponse = 0x0B,
    DiscoverAttributes = 0x0C,
    DiscoverAttributesResponse = 0x0D,
    DiscoverCommandsReceived = 0x11,
    DiscoverCommandsReceivedResponse = 0x12,
    DiscoverCommandsGenerated = 0x13,
    DiscoverCommandsGeneratedResponse = 0x14,
    DiscoverAttributesExtended = 0x15,
    DiscoverAttributesExtendedResponse = 0x16,
}

impl FoundationCommandId {
    /// Try to map a raw byte to a foundation command.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x00 => Some(Self::ReadAttributes),
            0x01 => Some(Self::ReadAttributesResponse),
            0x02 => Some(Self::WriteAttributes),
            0x03 => Some(Self::WriteAttributesUndivided),
            0x04 => Some(Self::WriteAttributesResponse),
            0x05 => Some(Self::WriteAttributesNoResponse),
            0x06 => Some(Self::ConfigureReporting),
            0x07 => Some(Self::ConfigureReportingResponse),
            0x08 => Some(Self::ReadReportingConfig),
            0x09 => Some(Self::ReadReportingConfigResponse),
            0x0A => Some(Self::ReportAttributes),
            0x0B => Some(Self::DefaultResponse),
            0x0C => Some(Self::DiscoverAttributes),
            0x0D => Some(Self::DiscoverAttributesResponse),
            0x11 => Some(Self::DiscoverCommandsReceived),
            0x12 => Some(Self::DiscoverCommandsReceivedResponse),
            0x13 => Some(Self::DiscoverCommandsGenerated),
            0x14 => Some(Self::DiscoverCommandsGeneratedResponse),
            0x15 => Some(Self::DiscoverAttributesExtended),
            0x16 => Some(Self::DiscoverAttributesExtendedResponse),
            _ => None,
        }
    }
}
