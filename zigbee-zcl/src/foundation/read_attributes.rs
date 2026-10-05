//! Read Attributes command (0x00) and Read Attributes Response (0x01).

use crate::data_types::{ZclDataType, ZclValue};
use crate::{AttributeId, ZclStatus};

/// Maximum number of attributes in a single read request / response.
pub const MAX_READ_ATTRS: usize = 16;

/// Read Attributes request — a list of attribute IDs to read.
#[derive(Debug, Clone)]
pub struct ReadAttributesRequest {
    pub attributes: heapless::Vec<AttributeId, MAX_READ_ATTRS>,
}

/// A single record in the Read Attributes Response.
#[derive(Debug, Clone)]
pub struct ReadAttributeRecord {
    pub id: AttributeId,
    pub status: ZclStatus,
    /// Present only when `status == Success`.
    pub data_type: ZclDataType,
    pub value: Option<ZclValue>,
}

/// Read Attributes Response.
#[derive(Debug, Clone)]
pub struct ReadAttributesResponse {
    pub records: heapless::Vec<ReadAttributeRecord, MAX_READ_ATTRS>,
}

impl ReadAttributesRequest {
    /// Parse from ZCL payload bytes (list of little-endian u16 attribute IDs).
    /// Requests with more than [`MAX_READ_ATTRS`] IDs are truncated.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if !data.len().is_multiple_of(2) {
            return None;
        }
        // Requests naming more attributes than we can track are answered for
        // the first MAX_READ_ATTRS IDs instead of being dropped entirely; the
        // response could not carry more records in one frame anyway.
        let attributes = data
            .chunks_exact(2)
            .take(MAX_READ_ATTRS)
            .map(|c| AttributeId(u16::from_le_bytes([c[0], c[1]])))
            .collect();
        Some(Self { attributes })
    }

    /// Serialize to ZCL payload bytes. Returns bytes written.
    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        for attr in &self.attributes {
            if pos + 2 > buf.len() {
                break;
            }
            let b = attr.0.to_le_bytes();
            buf[pos] = b[0];
            buf[pos + 1] = b[1];
            pos += 2;
        }
        pos
    }
}

impl ReadAttributeRecord {
    /// Serialize one complete record into `buf`, or return `None` without
    /// a partial record when it does not fit.
    fn serialize_into(&self, buf: &mut [u8]) -> Option<usize> {
        let hdr = buf.get_mut(..3)?;
        hdr[..2].copy_from_slice(&self.id.0.to_le_bytes());
        hdr[2] = self.status as u8;
        if self.status != ZclStatus::Success {
            return Some(3);
        }
        let Some(v) = &self.value else {
            // A successful record without a value cannot be encoded; report
            // it as a failure rather than emitting a header without a value.
            buf[2] = ZclStatus::Failure as u8;
            return Some(3);
        };
        let n = v.try_serialize(buf.get_mut(4..)?)?;
        buf[3] = self.data_type as u8;
        Some(4 + n)
    }
}

impl ReadAttributesResponse {
    /// Serialize the response to ZCL payload bytes. Returns bytes written.
    ///
    /// Only complete records are emitted: once a record (header plus value)
    /// does not fit, serialization stops so the payload stays well-formed and
    /// carries as many records as fit (ZCL r8 §2.5.2.3). Never panics.
    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        for rec in &self.records {
            match rec.serialize_into(buf.get_mut(pos..).unwrap_or_default()) {
                Some(n) => pos += n,
                None => break,
            }
        }
        pos
    }

    /// Parse from ZCL payload bytes.
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut records = heapless::Vec::new();
        let mut i = 0;
        while i + 2 < data.len() {
            let id = AttributeId(u16::from_le_bytes([data[i], data[i + 1]]));
            i += 2;
            if i >= data.len() {
                break;
            }
            let status = ZclStatus::from_u8(data[i]);
            i += 1;
            let (data_type, value) = if status == ZclStatus::Success && i < data.len() {
                let dt = ZclDataType::from_u8(data[i])?;
                i += 1;
                let (val, consumed) = ZclValue::deserialize(dt, &data[i..])?;
                i += consumed;
                (dt, Some(val))
            } else {
                (ZclDataType::NoData, None)
            };
            records
                .push(ReadAttributeRecord {
                    id,
                    status,
                    data_type,
                    value,
                })
                .ok()?;
        }
        Some(Self { records })
    }
}

/// Process a Read Attributes request using a type-erased attribute store.
pub fn process_read_dyn(
    store: &dyn crate::clusters::AttributeStoreAccess,
    request: &ReadAttributesRequest,
) -> ReadAttributesResponse {
    let mut records = heapless::Vec::new();
    for &attr_id in &request.attributes {
        let rec = match store.find(attr_id) {
            Some(def) => {
                if !def.access.is_readable() {
                    ReadAttributeRecord {
                        id: attr_id,
                        status: ZclStatus::WriteOnly,
                        data_type: ZclDataType::NoData,
                        value: None,
                    }
                } else if let Some(val) = store.get(attr_id) {
                    ReadAttributeRecord {
                        id: attr_id,
                        status: ZclStatus::Success,
                        data_type: def.data_type,
                        value: Some(val.clone()),
                    }
                } else {
                    ReadAttributeRecord {
                        id: attr_id,
                        status: ZclStatus::Failure,
                        data_type: ZclDataType::NoData,
                        value: None,
                    }
                }
            }
            None => ReadAttributeRecord {
                id: attr_id,
                status: ZclStatus::UnsupportedAttribute,
                data_type: ZclDataType::NoData,
                value: None,
            },
        };
        let _ = records.push(rec);
    }
    ReadAttributesResponse { records }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_string_response() -> ReadAttributesResponse {
        let s = ZclValue::CharString(heapless::Vec::from_slice(&[b'x'; 32]).unwrap());
        let mut records = heapless::Vec::new();
        for i in 0..4u16 {
            records
                .push(ReadAttributeRecord {
                    id: AttributeId(i),
                    status: ZclStatus::Success,
                    data_type: ZclDataType::CharString,
                    value: Some(s.clone()),
                })
                .unwrap();
        }
        records
            .push(ReadAttributeRecord {
                id: AttributeId(9),
                status: ZclStatus::UnsupportedAttribute,
                data_type: ZclDataType::NoData,
                value: None,
            })
            .unwrap();
        ReadAttributesResponse { records }
    }

    #[test]
    fn response_serializer_never_panics_and_emits_only_complete_records() {
        let resp = long_string_response();
        let mut full = [0u8; 256];
        let full_len = resp.serialize(&mut full);
        // 4 × (2 id + 1 status + 1 type + 1 len + 32) + 3 = 151
        assert_eq!(full_len, 151);
        for cap in 0..full.len() {
            let mut buf = [0xAAu8; 256];
            let n = resp.serialize(&mut buf[..cap]);
            assert!(n <= cap);
            // Each emitted record is complete: the output parses and the
            // record count matches the number of 37-byte records that fit.
            let parsed = ReadAttributesResponse::parse(&buf[..n]).unwrap();
            let expected = (cap / 37).min(4) + usize::from(cap >= 151);
            assert_eq!(parsed.records.len(), expected, "cap {cap}");
            assert_eq!(&buf[..n], &full[..n]);
        }
    }

    #[test]
    fn success_record_without_value_is_encoded_as_failure() {
        let mut records = heapless::Vec::new();
        records
            .push(ReadAttributeRecord {
                id: AttributeId(1),
                status: ZclStatus::Success,
                data_type: ZclDataType::U8,
                value: None,
            })
            .unwrap();
        let mut buf = [0u8; 8];
        let n = ReadAttributesResponse { records }.serialize(&mut buf);
        assert_eq!(&buf[..n], &[0x01, 0x00, ZclStatus::Failure as u8]);
    }

    #[test]
    fn request_with_more_than_max_ids_is_answered_for_the_first_max() {
        let mut payload = [0u8; 2 * (MAX_READ_ATTRS + 4)];
        for (i, c) in payload.chunks_exact_mut(2).enumerate() {
            c.copy_from_slice(&(i as u16).to_le_bytes());
        }
        let req = ReadAttributesRequest::parse(&payload).unwrap();
        assert_eq!(req.attributes.len(), MAX_READ_ATTRS);
        assert_eq!(req.attributes[15], AttributeId(15));
        assert!(ReadAttributesRequest::parse(&payload[..3]).is_none());
    }
}
