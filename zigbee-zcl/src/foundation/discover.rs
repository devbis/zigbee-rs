//! Discover Attributes (0x0C/0x0D) and Discover Commands Received/Generated
//! (0x11/0x12/0x13/0x14).

use crate::AttributeId;
use crate::data_types::ZclDataType;

/// Maximum attributes returned in a single discover response.
pub const MAX_DISCOVER: usize = 16;

/// Discover Attributes request.
#[derive(Debug, Clone)]
pub struct DiscoverAttributesRequest {
    /// Start attribute identifier.
    pub start_id: AttributeId,
    /// Maximum number of attribute IDs to return.
    pub max_results: u8,
}

/// A single entry in the Discover Attributes Response.
#[derive(Debug, Clone)]
pub struct DiscoverAttributeInfo {
    pub id: AttributeId,
    pub data_type: ZclDataType,
}

/// Discover Attributes Response.
#[derive(Debug, Clone)]
pub struct DiscoverAttributesResponse {
    /// `true` when the entire attribute list has been returned.
    pub complete: bool,
    pub attributes: heapless::Vec<DiscoverAttributeInfo, MAX_DISCOVER>,
}

impl DiscoverAttributesRequest {
    /// Parse from ZCL payload (2 bytes start_id + 1 byte max).
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 3 {
            return None;
        }
        Some(Self {
            start_id: AttributeId(u16::from_le_bytes([data[0], data[1]])),
            max_results: data[2],
        })
    }

    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        if buf.len() < 3 {
            return 0;
        }
        let b = self.start_id.0.to_le_bytes();
        buf[0] = b[0];
        buf[1] = b[1];
        buf[2] = self.max_results;
        3
    }
}

impl DiscoverAttributesResponse {
    /// Serialize the response to ZCL payload bytes.
    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        buf[0] = if self.complete { 1 } else { 0 };
        let mut pos = 1;
        for info in &self.attributes {
            // Need 2 (id) + 1 (type) = 3 bytes
            if pos + 3 > buf.len() {
                break;
            }
            let b = info.id.0.to_le_bytes();
            buf[pos] = b[0];
            buf[pos + 1] = b[1];
            pos += 2;
            buf[pos] = info.data_type as u8;
            pos += 1;
        }
        pos
    }
}

/// Process a discover request using a type-erased attribute store.
pub fn process_discover_dyn(
    store: &dyn crate::clusters::AttributeStoreAccess,
    request: &DiscoverAttributesRequest,
) -> DiscoverAttributesResponse {
    let ids = store.all_ids();
    let mut attributes = heapless::Vec::new();
    let max = request.max_results as usize;
    let mut count = 0;
    let mut complete = true;

    for id in &ids {
        if id.0 >= request.start_id.0 {
            if count >= max {
                complete = false;
                break;
            }
            if let Some(def) = store.find(*id) {
                let _ = attributes.push(DiscoverAttributeInfo {
                    id: *id,
                    data_type: def.data_type,
                });
                count += 1;
            }
        }
    }

    DiscoverAttributesResponse {
        complete,
        attributes,
    }
}

// ── Discover Commands Received (0x11/0x12) & Generated (0x13/0x14) ──

/// Maximum command IDs returned in a single discover-commands response.
pub const MAX_DISCOVER_COMMANDS: usize = 32;

/// Discover Commands Received/Generated request (0x11 / 0x13) — same wire format.
#[derive(Debug, Clone)]
pub struct DiscoverCommandsRequest {
    /// First command identifier to return.
    pub start_command_id: u8,
    /// Maximum number of command IDs to return.
    pub max_results: u8,
}

/// Discover Commands Received/Generated response (0x12 / 0x14) — same wire format.
#[derive(Debug, Clone)]
pub struct DiscoverCommandsResponse {
    /// `true` when the entire command list has been returned.
    pub complete: bool,
    /// The matching command identifiers.
    pub command_ids: heapless::Vec<u8, MAX_DISCOVER_COMMANDS>,
}

impl DiscoverCommandsRequest {
    /// Parse from ZCL payload (1 byte start_id + 1 byte max).
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 2 {
            return None;
        }
        Some(Self {
            start_command_id: data[0],
            max_results: data[1],
        })
    }

    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        if buf.len() < 2 {
            return 0;
        }
        buf[0] = self.start_command_id;
        buf[1] = self.max_results;
        2
    }
}

impl DiscoverCommandsResponse {
    /// Serialize the response: 1 byte complete flag + N command-ID bytes.
    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        buf[0] = u8::from(self.complete);
        let mut pos = 1;
        for &id in &self.command_ids {
            if pos >= buf.len() {
                break;
            }
            buf[pos] = id;
            pos += 1;
        }
        pos
    }
}

/// Filter `all_commands` starting from `start_id`, returning up to `max_results`.
pub fn process_discover_commands(
    all_commands: &[u8],
    start_id: u8,
    max_results: u8,
) -> DiscoverCommandsResponse {
    let max = max_results as usize;
    let mut command_ids: heapless::Vec<u8, MAX_DISCOVER_COMMANDS> = heapless::Vec::new();
    let mut complete = true;

    for &id in all_commands {
        if id >= start_id {
            if command_ids.len() >= max {
                complete = false;
                break;
            }
            let _ = command_ids.push(id);
        }
    }

    DiscoverCommandsResponse {
        complete,
        command_ids,
    }
}

// ── Discover Attributes Extended (0x15/0x16) ──

/// A single entry in the Discover Attributes Extended Response.
/// Includes access control flags per ZCL spec §2.5.14.
#[derive(Debug, Clone)]
pub struct DiscoverAttributeExtendedInfo {
    pub id: AttributeId,
    pub data_type: ZclDataType,
    /// Bit 0: readable, Bit 1: writable, Bit 2: reportable
    pub access_control: u8,
}

/// Discover Attributes Extended Response.
#[derive(Debug, Clone)]
pub struct DiscoverAttributesExtendedResponse {
    pub complete: bool,
    pub attributes: heapless::Vec<DiscoverAttributeExtendedInfo, MAX_DISCOVER>,
}

impl DiscoverAttributesExtendedResponse {
    /// Serialize: 1 byte complete + N*(2 id + 1 type + 1 access) = 4 bytes per entry.
    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        buf[0] = if self.complete { 1 } else { 0 };
        let mut pos = 1;
        for info in &self.attributes {
            if pos + 4 > buf.len() {
                break;
            }
            let b = info.id.0.to_le_bytes();
            buf[pos] = b[0];
            buf[pos + 1] = b[1];
            buf[pos + 2] = info.data_type as u8;
            buf[pos + 3] = info.access_control;
            pos += 4;
        }
        pos
    }
}

/// Process a Discover Attributes Extended request.
/// Returns attribute info with access control flags.
pub fn process_discover_extended_dyn(
    store: &dyn crate::clusters::AttributeStoreAccess,
    request: &DiscoverAttributesRequest,
) -> DiscoverAttributesExtendedResponse {
    let ids = store.all_ids();
    let mut attributes = heapless::Vec::new();
    let max = request.max_results as usize;
    let mut count = 0;
    let mut complete = true;

    for id in &ids {
        if id.0 >= request.start_id.0 {
            if count >= max {
                complete = false;
                break;
            }
            if let Some(def) = store.find(*id) {
                // Bit 0: readable, Bit 1: writable, Bit 2: reportable
                let mut access_control: u8 = 0;
                if def.access.is_readable() {
                    access_control |= 0x01;
                }
                if def.access.is_writable() {
                    access_control |= 0x02;
                }
                if def.access.is_reportable() {
                    access_control |= 0x04;
                }
                let _ = attributes.push(DiscoverAttributeExtendedInfo {
                    id: *id,
                    data_type: def.data_type,
                    access_control,
                });
                count += 1;
            }
        }
    }

    DiscoverAttributesExtendedResponse {
        complete,
        attributes,
    }
}
