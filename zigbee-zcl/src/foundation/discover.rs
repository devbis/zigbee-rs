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
        let mut complete = self.complete;
        let mut pos = 1;
        for info in &self.attributes {
            // Need 2 (id) + 1 (type) = 3 bytes
            let Some(rec) = buf.get_mut(pos..pos + 3) else {
                // Truncated: the requester must continue discovery.
                complete = false;
                break;
            };
            rec[..2].copy_from_slice(&info.id.0.to_le_bytes());
            rec[2] = info.data_type as u8;
            pos += 3;
        }
        buf[0] = complete as u8;
        pos
    }
}

/// Process a discover request using a type-erased attribute store.
pub fn process_discover_dyn(
    store: &dyn crate::clusters::AttributeStoreAccess,
    request: &DiscoverAttributesRequest,
) -> DiscoverAttributesResponse {
    // Same selection as the extended variant, without the access flags.
    let ext = process_discover_extended_dyn(store, request);
    DiscoverAttributesResponse {
        complete: ext.complete,
        attributes: ext
            .attributes
            .iter()
            .map(|a| DiscoverAttributeInfo {
                id: a.id,
                data_type: a.data_type,
            })
            .collect(),
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
        let n = self.command_ids.len().min(buf.len() - 1);
        buf[0] = u8::from(self.complete && n == self.command_ids.len());
        buf[1..1 + n].copy_from_slice(&self.command_ids[..n]);
        1 + n
    }
}

/// Return the command IDs of `all_commands` that are `>= start_id`, in
/// ascending order, up to `max_results` (ZCL r8 §2.5.19/§2.5.21).
pub fn process_discover_commands(
    all_commands: &[u8],
    start_id: u8,
    max_results: u8,
) -> DiscoverCommandsResponse {
    let mut command_ids: heapless::Vec<u8, MAX_DISCOVER_COMMANDS> = heapless::Vec::new();
    let mut floor = start_id as u16;
    let complete = loop {
        // Smallest command ID >= floor (0x100 = none left).
        let mut next = 0x100u16;
        for &c in all_commands {
            if c as u16 >= floor && (c as u16) < next {
                next = c as u16;
            }
        }
        let Ok(id) = u8::try_from(next) else {
            break true;
        };
        if command_ids.len() >= max_results as usize || command_ids.push(id).is_err() {
            break false;
        }
        floor = id as u16 + 1;
    };

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
        let mut complete = self.complete;
        let mut pos = 1;
        for info in &self.attributes {
            let Some(rec) = buf.get_mut(pos..pos + 4) else {
                complete = false;
                break;
            };
            rec[..2].copy_from_slice(&info.id.0.to_le_bytes());
            rec[2] = info.data_type as u8;
            rec[3] = info.access_control;
            pos += 4;
        }
        buf[0] = complete as u8;
        pos
    }
}

/// Process a Discover Attributes Extended request.
/// Returns attribute info with access control flags.
pub fn process_discover_extended_dyn(
    store: &dyn crate::clusters::AttributeStoreAccess,
    request: &DiscoverAttributesRequest,
) -> DiscoverAttributesExtendedResponse {
    // Attributes are reported in ascending ID order starting at the start
    // ID (ZCL r8 §2.5.13/§2.5.23); a selection pass per entry keeps this
    // allocation-free. `complete` is false whenever any attribute remains.
    let all = store.all_ids();
    let mut attributes = heapless::Vec::new();
    let mut floor = request.start_id.0 as u32;
    let complete = loop {
        // Smallest ID >= floor (0x1_0000 = none left).
        let mut next = 0x1_0000u32;
        for a in &all {
            let v = a.0 as u32;
            if v >= floor && v < next {
                next = v;
            }
        }
        let Ok(id) = u16::try_from(next) else {
            break true;
        };
        if attributes.len() >= request.max_results as usize {
            break false;
        }
        if let Some(def) = store.find(AttributeId(id)) {
            // Bit 0: readable, Bit 1: writable, Bit 2: reportable
            // (indexed by `AttributeAccess` declaration order).
            const AC: [u8; 4] = [0b101, 0b010, 0b111, 0b101];
            let access_control = AC[def.access as usize];
            let info = DiscoverAttributeExtendedInfo {
                id: def.id,
                data_type: def.data_type,
                access_control,
            };
            if attributes.push(info).is_err() {
                break false;
            }
        }
        floor = id as u32 + 1;
    };

    DiscoverAttributesExtendedResponse {
        complete,
        attributes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::{AttributeAccess, AttributeDefinition, AttributeStore};
    use crate::data_types::ZclValue;

    fn store(ids: &[u16]) -> AttributeStore<24> {
        let mut s = AttributeStore::new();
        for &id in ids {
            s.register(
                AttributeDefinition {
                    id: AttributeId(id),
                    data_type: ZclDataType::U8,
                    access: AttributeAccess::ReadOnly,
                    name: "a",
                },
                ZclValue::U8(0),
            )
            .unwrap();
        }
        s
    }

    fn ids(r: &DiscoverAttributesResponse) -> heapless::Vec<u16, MAX_DISCOVER> {
        r.attributes.iter().map(|a| a.id.0).collect()
    }

    #[test]
    fn attributes_are_returned_ascending_from_start_id() {
        let s = store(&[0x0010, 0x0002, 0xFFFD, 0x0000, 0x0005]);
        let r = process_discover_dyn(
            &s,
            &DiscoverAttributesRequest {
                start_id: AttributeId(1),
                max_results: 10,
            },
        );
        assert_eq!(&ids(&r)[..], &[0x0002, 0x0005, 0x0010, 0xFFFD]);
        assert!(r.complete);
    }

    #[test]
    fn truncation_clears_the_complete_flag() {
        let s = store(&[3, 1, 2]);
        let req = |start, max| DiscoverAttributesRequest {
            start_id: AttributeId(start),
            max_results: max,
        };
        let r = process_discover_dyn(&s, &req(0, 2));
        assert_eq!(&ids(&r)[..], &[1, 2]);
        assert!(!r.complete);
        // Exactly the remaining attributes: complete.
        let r = process_discover_dyn(&s, &req(3, 1));
        assert_eq!(&ids(&r)[..], &[3]);
        assert!(r.complete);
        // More attributes than MAX_DISCOVER.
        let many: heapless::Vec<u16, 24> = (0..20u16).rev().collect();
        let s = store(&many);
        let r = process_discover_dyn(&s, &req(0, 0xFF));
        assert_eq!(r.attributes.len(), MAX_DISCOVER);
        assert_eq!(r.attributes[15].id, AttributeId(15));
        assert!(!r.complete);
        let r = process_discover_extended_dyn(&s, &req(0, 0xFF));
        assert_eq!(r.attributes.len(), MAX_DISCOVER);
        assert!(!r.complete);
        // Buffer truncation also clears the flag in the serialized frame.
        let r = process_discover_dyn(&store(&[1, 2]), &req(0, 10));
        assert!(r.complete);
        let mut buf = [0u8; 5];
        assert_eq!(r.serialize(&mut buf), 4);
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn commands_are_sorted_and_flag_reflects_truncation() {
        let r = process_discover_commands(&[5, 0, 3, 1], 1, 10);
        assert_eq!(&r.command_ids[..], &[1, 3, 5]);
        assert!(r.complete);
        let r = process_discover_commands(&[5, 0, 3, 1], 0, 2);
        assert_eq!(&r.command_ids[..], &[0, 1]);
        assert!(!r.complete);
        let mut buf = [0u8; 2];
        let r = process_discover_commands(&[1, 2], 0, 10);
        assert_eq!(r.serialize(&mut buf), 2);
        assert_eq!(buf, [0, 1]);
    }

    #[test]
    fn access_control_bits_match_access_predicates_and_edge_ids() {
        for access in [
            AttributeAccess::ReadOnly,
            AttributeAccess::WriteOnly,
            AttributeAccess::ReadWrite,
            AttributeAccess::Reportable,
        ] {
            let mut s: AttributeStore<2> = AttributeStore::new();
            for id in [0xFFFF, 0] {
                s.register(
                    AttributeDefinition {
                        id: AttributeId(id),
                        data_type: ZclDataType::U8,
                        access,
                        name: "a",
                    },
                    ZclValue::U8(0),
                )
                .unwrap();
            }
            let all = DiscoverAttributesRequest {
                start_id: AttributeId(0),
                max_results: 10,
            };
            let r = process_discover_extended_dyn(&s, &all);
            assert!(r.complete);
            assert_eq!(r.attributes.len(), 2);
            assert_eq!(
                (r.attributes[0].id, r.attributes[1].id),
                (AttributeId(0), AttributeId(0xFFFF))
            );
            let want = access.is_readable() as u8
                | (access.is_writable() as u8) << 1
                | (access.is_reportable() as u8) << 2;
            assert_eq!(r.attributes[0].access_control, want);
        }
        let r = process_discover_commands(&[0xFF, 0], 0xFF, 10);
        assert_eq!(&r.command_ids[..], &[0xFF]);
        assert!(r.complete);
    }
}
