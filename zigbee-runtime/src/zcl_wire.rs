//! Bounded ZCL wire helpers owned by the runtime dispatcher.
//!
//! The runtime queues every local ZCL response into a fixed
//! [`crate::PENDING_ZCL_DATA_CAP`]-byte slot. Two properties therefore matter
//! more here than in a general-purpose serializer:
//!
//! * a response must be made of *whole* records — a frame truncated in the
//!   middle of a record is malformed on the wire; and
//! * building it must never index past a buffer, whatever the record count or
//!   string lengths the cluster produced.
//!
//! The serializers below size each record exactly before writing it and stop
//! at the first record that would not fit, so the queued frame is always
//! well-formed and the write is always in bounds. ZCL r8 §2.5.2.3 (Read
//! Attributes Response) lets a server return the records that fit; the client
//! reads the remainder with a follow-up request.
//!
//! [`parse_configure_reporting_record`] is the single Configure Reporting
//! record parser. The dispatcher runs it over the whole payload *before*
//! changing any state so a malformed tail can no longer leave a partially
//! applied command behind.

use zigbee_zcl::ZclStatus;
use zigbee_zcl::data_types::{self, ZclDataType, ZclValue};
use zigbee_zcl::foundation::read_attributes::ReadAttributesResponse;
use zigbee_zcl::foundation::reporting::{
    AttributeReport, ReadReportingConfigResponse, ReportDirection, ReportingConfig,
};

/// Exact ZCL wire length of `value` (as written by [`ZclValue::serialize`]).
#[inline]
pub(crate) fn zcl_value_wire_len(value: &ZclValue) -> usize {
    value.wire_len()
}

/// Write `value` into `buf[pos..]` only when it fits; returns the new position.
fn put_value(buf: &mut [u8], pos: usize, value: &ZclValue) -> Option<usize> {
    let len = zcl_value_wire_len(value);
    let end = pos.checked_add(len)?;
    if end > buf.len() {
        return None;
    }
    let written = value.serialize(&mut buf[pos..end]);
    debug_assert_eq!(written, len);
    Some(end)
}

/// Serialize a Read Attributes Response, keeping only whole records that fit.
///
/// Returns `(bytes_written, records_written)`.
pub(crate) fn serialize_read_attributes_response(
    response: &ReadAttributesResponse,
    buf: &mut [u8],
) -> (usize, usize) {
    let mut pos = 0usize;
    let mut written = 0usize;
    for rec in &response.records {
        let value = if rec.status == ZclStatus::Success {
            rec.value.as_ref()
        } else {
            None
        };
        let needed = 3 + value.map_or(0, |v| 1 + zcl_value_wire_len(v));
        if pos + needed > buf.len() {
            break;
        }
        let id = rec.id.0.to_le_bytes();
        buf[pos] = id[0];
        buf[pos + 1] = id[1];
        buf[pos + 2] = rec.status as u8;
        let mut next = pos + 3;
        if let Some(value) = value {
            buf[next] = rec.data_type as u8;
            next = match put_value(buf, next + 1, value) {
                Some(end) => end,
                None => break,
            };
        }
        pos = next;
        written += 1;
    }
    (pos, written)
}

/// Serialize a Read Reporting Configuration Response, keeping only whole
/// records that fit. Returns `(bytes_written, records_written)`.
pub(crate) fn serialize_read_reporting_response(
    response: &ReadReportingConfigResponse,
    buf: &mut [u8],
) -> (usize, usize) {
    let mut pos = 0usize;
    let mut written = 0usize;
    for rec in &response.records {
        let success = rec.status == ZclStatus::Success;
        let send_config = match (success, rec.direction) {
            (true, ReportDirection::Send) => rec.config.as_ref(),
            _ => None,
        };
        let timeout = match (success, rec.direction) {
            (true, ReportDirection::Receive) => rec.timeout,
            _ => None,
        };
        let needed =
            4 + send_config.map_or(0, |cfg| {
                5 + cfg.reportable_change.as_ref().map_or(0, zcl_value_wire_len)
            }) + timeout.map_or(0, |_| 2);
        if pos + needed > buf.len() {
            break;
        }
        let id = rec.attribute_id.0.to_le_bytes();
        buf[pos] = rec.status as u8;
        buf[pos + 1] = rec.direction as u8;
        buf[pos + 2] = id[0];
        buf[pos + 3] = id[1];
        let mut next = pos + 4;
        if let Some(cfg) = send_config {
            buf[next] = cfg.data_type as u8;
            buf[next + 1..next + 3].copy_from_slice(&cfg.min_interval.to_le_bytes());
            buf[next + 3..next + 5].copy_from_slice(&cfg.max_interval.to_le_bytes());
            next += 5;
            if let Some(change) = cfg.reportable_change.as_ref() {
                next = match put_value(buf, next, change) {
                    Some(end) => end,
                    None => break,
                };
            }
        }
        if let Some(timeout) = timeout {
            buf[next..next + 2].copy_from_slice(&timeout.to_le_bytes());
            next += 2;
        }
        pos = next;
        written += 1;
    }
    (pos, written)
}

/// Parse one Configure Reporting attribute record starting at `*cursor`.
///
/// Returns `None` for any malformed record (unknown direction, truncated
/// fields, unknown data type, truncated reportable change). On success the
/// cursor is advanced past the record. Disabled (compiled-out) analog data
/// types are still *framed* correctly — their fixed-size reportable change
/// is skipped — so the record is reported as `INVALID_DATA_TYPE` by the
/// caller rather than turning the whole command into a malformed one.
pub(crate) fn parse_configure_reporting_record(
    payload: &[u8],
    cursor: &mut usize,
) -> Option<ReportingConfig> {
    let mut i = *cursor;
    let direction = match *payload.get(i)? {
        0x00 => ReportDirection::Send,
        0x01 => ReportDirection::Receive,
        _ => return None,
    };
    i += 1;
    let attribute_id =
        zigbee_zcl::AttributeId(u16::from_le_bytes([*payload.get(i)?, *payload.get(i + 1)?]));
    i += 2;
    let cfg = if direction == ReportDirection::Send {
        let data_type = ZclDataType::from_u8(*payload.get(i)?)?;
        let min_interval = u16::from_le_bytes([*payload.get(i + 1)?, *payload.get(i + 2)?]);
        let max_interval = u16::from_le_bytes([*payload.get(i + 3)?, *payload.get(i + 4)?]);
        i += 5;
        let reportable_change = if data_types::is_analog_type(data_type) {
            if data_types::is_data_type_enabled(data_type) {
                let (value, consumed) = ZclValue::deserialize(data_type, payload.get(i..)?)?;
                i += consumed;
                Some(value)
            } else {
                let size = data_types::data_type_size(data_type)?;
                if i + size > payload.len() {
                    return None;
                }
                i += size;
                None
            }
        } else {
            None
        };
        ReportingConfig {
            direction,
            attribute_id,
            data_type,
            min_interval,
            max_interval,
            reportable_change,
        }
    } else {
        let timeout = u16::from_le_bytes([*payload.get(i)?, *payload.get(i + 1)?]);
        i += 2;
        ReportingConfig {
            direction,
            attribute_id,
            data_type: ZclDataType::NoData,
            min_interval: 0,
            max_interval: timeout,
            reportable_change: None,
        }
    };
    *cursor = i;
    Some(cfg)
}

/// Validate a whole Configure Reporting payload without changing any state.
///
/// Returns the number of records when every record is well formed, `None`
/// for an empty or malformed payload.
pub(crate) fn count_configure_reporting_records(payload: &[u8]) -> Option<usize> {
    if payload.is_empty() {
        return None;
    }
    let mut cursor = 0usize;
    let mut records = 0usize;
    while cursor < payload.len() {
        parse_configure_reporting_record(payload, &mut cursor)?;
        records += 1;
    }
    Some(records)
}

/// Pack whole Report Attributes records, starting at `records[start]`, into
/// `buf`.
///
/// Returns `(bytes_written, next_record)`. Records are never split: packing
/// stops before the first record that does not fit. When `records[start]`
/// alone cannot fit an empty `buf` it is unsendable in a single frame; the
/// result is then `(0, start + 1)` so the caller can skip it explicitly.
pub(crate) fn pack_report_records(
    records: &[AttributeReport],
    start: usize,
    buf: &mut [u8],
) -> (usize, usize) {
    let mut pos = 0usize;
    let mut next = start;
    while let Some(record) = records.get(next) {
        let end = pos
            .checked_add(3)
            .and_then(|p| p.checked_add(zcl_value_wire_len(&record.value)));
        let Some(end) = end.filter(|&end| end <= buf.len()) else {
            if pos == 0 {
                return (0, next + 1);
            }
            break;
        };
        buf[pos..pos + 2].copy_from_slice(&record.id.0.to_le_bytes());
        buf[pos + 2] = record.data_type as u8;
        // `end` already bounds the whole record, so this cannot fail; the
        // header bytes written above are not committed unless it succeeds.
        let Some(written_end) = put_value(buf, pos + 3, &record.value) else {
            break;
        };
        debug_assert_eq!(written_end, end);
        pos = written_end;
        next += 1;
    }
    (pos, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zigbee_zcl::AttributeId;
    use zigbee_zcl::foundation::read_attributes::ReadAttributeRecord;

    fn string(len: usize) -> ZclValue {
        let mut v = heapless::Vec::new();
        for n in 0..len {
            v.push(b'a' + (n % 26) as u8).unwrap();
        }
        ZclValue::CharString(v)
    }

    fn report(id: u16, value: ZclValue) -> AttributeReport {
        AttributeReport {
            id: AttributeId(id),
            data_type: ZclDataType::CharString,
            value,
        }
    }

    #[test]
    fn report_packing_keeps_whole_records_and_skips_unsendable_ones() {
        // 3 + 11 = 14 bytes, 3 + 31 = 34 bytes (never fits 30), 14 bytes.
        let records = [
            report(1, string(10)),
            report(2, string(30)),
            report(3, string(10)),
            report(4, string(10)),
        ];
        let mut buf = [0u8; 30];
        assert_eq!(pack_report_records(&records, 0, &mut buf), (14, 1));
        assert_eq!(&buf[..3], &[0x01, 0x00, ZclDataType::CharString as u8]);
        assert_eq!(buf[3], 10);
        // The oversized record is reported as unsendable, not truncated.
        assert_eq!(pack_report_records(&records, 1, &mut buf), (0, 2));
        assert_eq!(pack_report_records(&records, 2, &mut buf), (28, 4));
        assert_eq!(pack_report_records(&records, 4, &mut buf), (0, 4));
    }

    #[test]
    fn value_wire_len_matches_serializer_for_every_variant() {
        let values = [
            ZclValue::NoData,
            ZclValue::Bool(true),
            ZclValue::Bitmap8(1),
            ZclValue::Bitmap16(1),
            ZclValue::Bitmap32(1),
            ZclValue::Bitmap64(1),
            ZclValue::U8(1),
            ZclValue::U16(1),
            ZclValue::U24(1),
            ZclValue::U32(1),
            ZclValue::U48(1),
            ZclValue::U64(1),
            ZclValue::I8(1),
            ZclValue::I16(1),
            ZclValue::I32(1),
            ZclValue::I64(1),
            ZclValue::Enum8(1),
            ZclValue::Enum16(1),
            ZclValue::Float32(1.0),
            ZclValue::Float64(1.0),
            ZclValue::OctetString(heapless::Vec::new()),
            string(data_types::MAX_STRING_LEN),
            ZclValue::UtcTime(1),
            ZclValue::IeeeAddr(1),
            ZclValue::SecurityKey128([7; 16]),
        ];
        for value in values {
            let mut buf = [0u8; 64];
            assert_eq!(value.serialize(&mut buf), zcl_value_wire_len(&value));
        }
    }

    #[test]
    fn read_attributes_response_keeps_only_whole_records_and_never_panics() {
        let mut response = ReadAttributesResponse {
            records: heapless::Vec::new(),
        };
        while response.records.len() < response.records.capacity() {
            let n = response.records.len() as u16;
            response
                .records
                .push(ReadAttributeRecord {
                    id: AttributeId(n),
                    status: ZclStatus::Success,
                    data_type: ZclDataType::CharString,
                    value: Some(string(data_types::MAX_STRING_LEN)),
                })
                .unwrap();
        }
        // Each record is 2 + 1 + 1 + 33 = 37 bytes.
        for cap in [0usize, 3, 36, 37, 73, 74, 125, 253] {
            let mut buf = [0xEEu8; 253];
            let (len, records) = serialize_read_attributes_response(&response, &mut buf[..cap]);
            let expected = (cap / 37).min(response.records.len());
            assert_eq!(records, expected, "cap {cap}");
            assert_eq!(len, expected * 37, "cap {cap}");
            assert!(buf[cap..].iter().all(|&b| b == 0xEE));
        }
    }

    #[test]
    fn read_attributes_response_mixes_error_and_value_records() {
        let mut response = ReadAttributesResponse {
            records: heapless::Vec::new(),
        };
        response
            .records
            .push(ReadAttributeRecord {
                id: AttributeId(0x0001),
                status: ZclStatus::UnsupportedAttribute,
                data_type: ZclDataType::NoData,
                value: None,
            })
            .unwrap();
        response
            .records
            .push(ReadAttributeRecord {
                id: AttributeId(0x0002),
                status: ZclStatus::Success,
                data_type: ZclDataType::U16,
                value: Some(ZclValue::U16(0x1234)),
            })
            .unwrap();
        let mut buf = [0u8; 16];
        let (len, records) = serialize_read_attributes_response(&response, &mut buf);
        assert_eq!(records, 2);
        assert_eq!(
            &buf[..len],
            &[0x01, 0x00, 0x86, 0x02, 0x00, 0x00, 0x21, 0x34, 0x12]
        );
    }

    #[test]
    fn configure_record_parser_rejects_every_truncation() {
        // Send, attr 0x0000, int16, min 1, max 10, change 0x0005.
        let record = [0x00, 0x00, 0x00, 0x29, 0x01, 0x00, 0x0A, 0x00, 0x05, 0x00];
        assert_eq!(count_configure_reporting_records(&record), Some(1));
        for len in 0..record.len() {
            assert_eq!(count_configure_reporting_records(&record[..len]), None);
        }
        let mut two = [0u8; 20];
        two[..10].copy_from_slice(&record);
        two[10..].copy_from_slice(&record);
        assert_eq!(count_configure_reporting_records(&two), Some(2));
        for len in 11..20 {
            assert_eq!(count_configure_reporting_records(&two[..len]), None);
        }
        // Bad direction / unknown type.
        assert_eq!(count_configure_reporting_records(&[0x02, 0, 0]), None);
        assert_eq!(
            count_configure_reporting_records(&[0x00, 0, 0, 0xFE, 0, 0, 0, 0]),
            None
        );
        // Receive record: dir + attr + timeout.
        assert_eq!(
            count_configure_reporting_records(&[0x01, 0x00, 0x00, 0x10, 0x00]),
            Some(1)
        );
    }
}
