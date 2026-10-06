//! ZCL data type system (ZCL Rev 8, Chapter 2.6).
//!
//! Defines every standard data type ID and a value enum for storing typed
//! attribute values in a `no_std` context.

/// Standard ZCL data type identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ZclDataType {
    NoData = 0x00,
    Bool = 0x10,
    Bitmap8 = 0x18,
    Bitmap16 = 0x19,
    Bitmap32 = 0x1B,
    Bitmap64 = 0x1F,
    U8 = 0x20,
    U16 = 0x21,
    U24 = 0x22,
    U32 = 0x23,
    U48 = 0x25,
    U64 = 0x27,
    I8 = 0x28,
    I16 = 0x29,
    I32 = 0x2B,
    I64 = 0x2F,
    Enum8 = 0x30,
    Enum16 = 0x31,
    Float16 = 0x38,
    Float32 = 0x39,
    Float64 = 0x3A,
    OctetString = 0x41,
    CharString = 0x42,
    OctetString16 = 0x43,
    CharString16 = 0x44,
    Array = 0x48,
    Struct = 0x4C,
    Set = 0x50,
    Bag = 0x51,
    TimeOfDay = 0xE0,
    Date = 0xE1,
    UtcTime = 0xE2,
    ClusterId = 0xE8,
    AttributeId = 0xE9,
    BacNetOid = 0xEA,
    IeeeAddr = 0xF0,
    SecurityKey128 = 0xF1,
}

impl ZclDataType {
    /// Try to create a `ZclDataType` from its wire byte.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x00 => Some(Self::NoData),
            0x10 => Some(Self::Bool),
            0x18 => Some(Self::Bitmap8),
            0x19 => Some(Self::Bitmap16),
            0x1B => Some(Self::Bitmap32),
            0x1F => Some(Self::Bitmap64),
            0x20 => Some(Self::U8),
            0x21 => Some(Self::U16),
            0x22 => Some(Self::U24),
            0x23 => Some(Self::U32),
            0x25 => Some(Self::U48),
            0x27 => Some(Self::U64),
            0x28 => Some(Self::I8),
            0x29 => Some(Self::I16),
            0x2B => Some(Self::I32),
            0x2F => Some(Self::I64),
            0x30 => Some(Self::Enum8),
            0x31 => Some(Self::Enum16),
            0x38 => Some(Self::Float16),
            0x39 => Some(Self::Float32),
            0x3A => Some(Self::Float64),
            0x41 => Some(Self::OctetString),
            0x42 => Some(Self::CharString),
            0x43 => Some(Self::OctetString16),
            0x44 => Some(Self::CharString16),
            0x48 => Some(Self::Array),
            0x4C => Some(Self::Struct),
            0x50 => Some(Self::Set),
            0x51 => Some(Self::Bag),
            0xE0 => Some(Self::TimeOfDay),
            0xE1 => Some(Self::Date),
            0xE2 => Some(Self::UtcTime),
            0xE8 => Some(Self::ClusterId),
            0xE9 => Some(Self::AttributeId),
            0xEA => Some(Self::BacNetOid),
            0xF0 => Some(Self::IeeeAddr),
            0xF1 => Some(Self::SecurityKey128),
            _ => None,
        }
    }
}

/// Return the fixed wire size (in bytes) for a data type, or `None` for
/// variable-length types.
pub fn data_type_size(dt: ZclDataType) -> Option<usize> {
    // The discriminant is the ZCL type tag (ZCL r8 Table 2-11). Tags
    // 0x08..=0x37 (dataN, bool, bitmapN, uintN, intN, enumN) encode the
    // width in their low three bits; the remaining fixed-size tags are
    // listed explicitly. Every other tag is variable length.
    let t = dt as u8;
    Some(match t {
        0x00 => 0,
        0x08..=0x37 => (t & 7) as usize + 1,
        0x38..=0x3A => 2 << (t - 0x38), // semi, single, double
        0xE8 | 0xE9 => 2,               // cluster/attribute ID
        0xE0..=0xEA => 4,               // ToD, date, UTC, BACnet OID
        0xF0 | 0xF1 => 8 << (t - 0xF0), // EUI64, key128
        _ => return None,
    })
}

/// Whether support for this data type is enabled in the current build.
#[cfg(all(feature = "float32", feature = "float64"))]
pub const fn is_data_type_enabled(dt: ZclDataType) -> bool {
    let _ = dt;
    true
}

/// Whether support for this data type is enabled in the current build.
#[cfg(all(feature = "float32", not(feature = "float64")))]
pub const fn is_data_type_enabled(dt: ZclDataType) -> bool {
    !matches!(dt, ZclDataType::Float64)
}

/// Whether support for this data type is enabled in the current build.
#[cfg(all(not(feature = "float32"), feature = "float64"))]
pub const fn is_data_type_enabled(dt: ZclDataType) -> bool {
    !matches!(dt, ZclDataType::Float32)
}

/// Whether support for this data type is enabled in the current build.
#[cfg(all(not(feature = "float32"), not(feature = "float64")))]
pub const fn is_data_type_enabled(dt: ZclDataType) -> bool {
    !matches!(dt, ZclDataType::Float32 | ZclDataType::Float64)
}

/// Maximum size (bytes) of an inline string/octet-string value.
pub const MAX_STRING_LEN: usize = 32;

/// Typed ZCL value matching the data type system.
#[derive(Debug, Clone, PartialEq)]
pub enum ZclValue {
    NoData,
    Bool(bool),
    Bitmap8(u8),
    Bitmap16(u16),
    Bitmap32(u32),
    Bitmap64(u64),
    U8(u8),
    U16(u16),
    U24(u32),
    U32(u32),
    U48(u64),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    Enum8(u8),
    Enum16(u16),
    /// Semi-precision float, kept as raw IEEE 754 binary16 bits.
    Float16(u16),
    Float32(f32),
    Float64(f64),
    /// Octet/char string stored inline (length-prefixed on the wire).
    OctetString(heapless::Vec<u8, MAX_STRING_LEN>),
    CharString(heapless::Vec<u8, MAX_STRING_LEN>),
    UtcTime(u32),
    IeeeAddr(u64),
    SecurityKey128([u8; 16]),
}

impl ZclValue {
    /// Return the ZCL data type tag for this value.
    pub fn data_type(&self) -> ZclDataType {
        match self {
            Self::NoData => ZclDataType::NoData,
            Self::Bool(_) => ZclDataType::Bool,
            Self::Bitmap8(_) => ZclDataType::Bitmap8,
            Self::Bitmap16(_) => ZclDataType::Bitmap16,
            Self::Bitmap32(_) => ZclDataType::Bitmap32,
            Self::Bitmap64(_) => ZclDataType::Bitmap64,
            Self::U8(_) => ZclDataType::U8,
            Self::U16(_) => ZclDataType::U16,
            Self::U24(_) => ZclDataType::U24,
            Self::U32(_) => ZclDataType::U32,
            Self::U48(_) => ZclDataType::U48,
            Self::U64(_) => ZclDataType::U64,
            Self::I8(_) => ZclDataType::I8,
            Self::I16(_) => ZclDataType::I16,
            Self::I32(_) => ZclDataType::I32,
            Self::I64(_) => ZclDataType::I64,
            Self::Enum8(_) => ZclDataType::Enum8,
            Self::Enum16(_) => ZclDataType::Enum16,
            Self::Float16(_) => ZclDataType::Float16,
            Self::Float32(_) => ZclDataType::Float32,
            Self::Float64(_) => ZclDataType::Float64,
            Self::OctetString(_) => ZclDataType::OctetString,
            Self::CharString(_) => ZclDataType::CharString,
            Self::UtcTime(_) => ZclDataType::UtcTime,
            Self::IeeeAddr(_) => ZclDataType::IeeeAddr,
            Self::SecurityKey128(_) => ZclDataType::SecurityKey128,
        }
    }

    /// Number of bytes this value occupies on the wire (excluding the
    /// data-type tag).
    pub fn wire_len(&self) -> usize {
        match self {
            Self::OctetString(v) | Self::CharString(v) => 1 + v.len(),
            Self::SecurityKey128(_) => 16,
            _ => self.scalar_le().1,
        }
    }

    /// Little-endian bytes and width of a fixed-size scalar value.
    fn scalar_le(&self) -> ([u8; 8], usize) {
        let (v, n): (u64, usize) = match *self {
            Self::Bool(v) => (v as u64, 1),
            Self::U8(v) | Self::Enum8(v) | Self::Bitmap8(v) => (v as u64, 1),
            Self::I8(v) => (v as u8 as u64, 1),
            Self::U16(v) | Self::Enum16(v) | Self::Bitmap16(v) | Self::Float16(v) => (v as u64, 2),
            Self::I16(v) => (v as u16 as u64, 2),
            Self::U24(v) => (v as u64, 3),
            Self::U32(v) | Self::UtcTime(v) | Self::Bitmap32(v) => (v as u64, 4),
            Self::I32(v) => (v as u32 as u64, 4),
            Self::Float32(v) => (v.to_bits() as u64, 4),
            Self::U48(v) => (v, 6),
            Self::U64(v) | Self::IeeeAddr(v) | Self::Bitmap64(v) => (v, 8),
            Self::I64(v) => (v as u64, 8),
            Self::Float64(v) => (v.to_bits(), 8),
            Self::NoData | Self::OctetString(_) | Self::CharString(_) | Self::SecurityKey128(_) => {
                (0, 0)
            }
        };
        (v.to_le_bytes(), n)
    }

    /// Serialize this value to `buf` (little-endian, ZCL wire format).
    ///
    /// Returns `None` without writing anything if `buf` is shorter than
    /// [`wire_len`](Self::wire_len).
    pub fn try_serialize(&self, buf: &mut [u8]) -> Option<usize> {
        let (le, n);
        // (length-prefix flag, payload)
        let (p, src): (bool, &[u8]) = match self {
            Self::OctetString(v) | Self::CharString(v) => (true, v),
            Self::SecurityKey128(k) => (false, k),
            _ => {
                (le, n) = self.scalar_le();
                (false, &le[..n])
            }
        };
        let out = buf.get_mut(..p as usize + src.len())?;
        if p {
            out[0] = src.len() as u8;
        }
        out[p as usize..].copy_from_slice(src);
        Some(out.len())
    }

    /// Serialize this value to a buffer (little-endian, ZCL wire format).
    /// Returns the number of bytes written, or 0 (nothing written) when the
    /// buffer is too small. Never panics.
    pub fn serialize(&self, buf: &mut [u8]) -> usize {
        self.try_serialize(buf).unwrap_or(0)
    }

    /// Deserialize a ZCL value of a known type from a buffer.
    /// Returns `(value, bytes_consumed)`.
    pub fn deserialize(dt: ZclDataType, data: &[u8]) -> Option<(Self, usize)> {
        if !is_data_type_enabled(dt) {
            return None;
        }
        if matches!(dt, ZclDataType::OctetString | ZclDataType::CharString) {
            let len = *data.first()? as usize;
            let v = heapless::Vec::from_slice(data.get(1..1 + len)?).ok()?;
            let value = if dt == ZclDataType::OctetString {
                Self::OctetString(v)
            } else {
                Self::CharString(v)
            };
            return Some((value, 1 + len));
        }
        // Fixed-size types: read `n` little-endian bytes.
        let n = data_type_size(dt)?;
        let b = data.get(..n)?;
        if dt == ZclDataType::SecurityKey128 {
            let mut key = [0u8; 16];
            key.copy_from_slice(b);
            return Some((Self::SecurityKey128(key), 16));
        }
        let mut le = [0u8; 8];
        le[..n].copy_from_slice(b);
        let u = u64::from_le_bytes(le);
        let value = match dt {
            ZclDataType::NoData => Self::NoData,
            ZclDataType::Bool => Self::Bool(u != 0),
            ZclDataType::U8 => Self::U8(u as u8),
            ZclDataType::Enum8 => Self::Enum8(u as u8),
            ZclDataType::Bitmap8 => Self::Bitmap8(u as u8),
            ZclDataType::I8 => Self::I8(u as i8),
            ZclDataType::U16 => Self::U16(u as u16),
            ZclDataType::Enum16 => Self::Enum16(u as u16),
            ZclDataType::Bitmap16 => Self::Bitmap16(u as u16),
            ZclDataType::Float16 => Self::Float16(u as u16),
            ZclDataType::I16 => Self::I16(u as i16),
            ZclDataType::U24 => Self::U24(u as u32),
            ZclDataType::U32 => Self::U32(u as u32),
            ZclDataType::Bitmap32 => Self::Bitmap32(u as u32),
            ZclDataType::UtcTime => Self::UtcTime(u as u32),
            ZclDataType::I32 => Self::I32(u as i32),
            ZclDataType::U48 => Self::U48(u),
            ZclDataType::U64 => Self::U64(u),
            ZclDataType::Bitmap64 => Self::Bitmap64(u),
            ZclDataType::IeeeAddr => Self::IeeeAddr(u),
            ZclDataType::I64 => Self::I64(u as i64),
            #[cfg(feature = "float32")]
            ZclDataType::Float32 => Self::Float32(f32::from_bits(u as u32)),
            #[cfg(feature = "float64")]
            ZclDataType::Float64 => Self::Float64(f64::from_bits(u)),
            // Other fixed-size types (ClusterId, AttributeId, TimeOfDay,
            // Date, BACnet OID) have no inline representation.
            _ => return None,
        };
        Some((value, n))
    }
}

impl ZclValue {
    /// Integer value (sign-extended to 64 bits for signed types) and
    /// signedness, for reportable-change comparison.
    fn int_bits(&self) -> Option<(u64, bool)> {
        Some(match *self {
            Self::U8(v) => (v as u64, false),
            Self::U16(v) => (v as u64, false),
            Self::U24(v) | Self::U32(v) | Self::UtcTime(v) => (v as u64, false),
            Self::U48(v) | Self::U64(v) => (v, false),
            Self::I8(v) => (v as i64 as u64, true),
            Self::I16(v) => (v as i64 as u64, true),
            Self::I32(v) => (v as i64 as u64, true),
            Self::I64(v) => (v as u64, true),
            _ => return None,
        })
    }

    /// Check if the absolute difference between `self` and `other` reaches a
    /// reportable-change threshold (ZCL r8 §2.5.7.1.7).
    ///
    /// Used by the reporting engine for reportable_change comparison.
    /// Returns `true` if `self != other` and `|self - other| >= |threshold|`.
    /// The reportable change is a magnitude: a negative signed threshold is
    /// interpreted by its absolute value. For non-numeric or mismatched types,
    /// returns `true` if the values differ.
    pub fn exceeds_threshold(&self, other: &ZclValue, threshold: &ZclValue) -> bool {
        let d = core::mem::discriminant(self);
        let same_type =
            d == core::mem::discriminant(other) && d == core::mem::discriminant(threshold);
        if same_type
            && let (Some((a, signed)), Some((b, _)), Some((t, _))) =
                (self.int_bits(), other.int_bits(), threshold.int_bits())
        {
            let (diff, t) = if signed {
                ((a as i64).abs_diff(b as i64), (t as i64).unsigned_abs())
            } else {
                (a.abs_diff(b), t)
            };
            return diff != 0 && diff >= t;
        }
        match (self, other, threshold) {
            #[cfg(feature = "float32")]
            (ZclValue::Float16(a), ZclValue::Float16(b), ZclValue::Float16(t)) => {
                float32_exceeds_threshold(f16_to_f32(*a), f16_to_f32(*b), f16_to_f32(*t))
            }
            #[cfg(feature = "float32")]
            (ZclValue::Float32(a), ZclValue::Float32(b), ZclValue::Float32(t)) => {
                float32_exceeds_threshold(*a, *b, *t)
            }
            #[cfg(not(feature = "float32"))]
            (ZclValue::Float32(a), ZclValue::Float32(b), _) => a.to_bits() != b.to_bits(),
            #[cfg(not(feature = "float32"))]
            (ZclValue::Float32(_), _, _) | (_, ZclValue::Float32(_), _) => true,
            #[cfg(feature = "float64")]
            (ZclValue::Float64(a), ZclValue::Float64(b), ZclValue::Float64(t)) => {
                float64_exceeds_threshold(*a, *b, *t)
            }
            #[cfg(not(feature = "float64"))]
            (ZclValue::Float64(a), ZclValue::Float64(b), _) => a.to_bits() != b.to_bits(),
            #[cfg(not(feature = "float64"))]
            (ZclValue::Float64(_), _, _) | (_, ZclValue::Float64(_), _) => true,
            // For non-numeric or mismatched types, any difference triggers
            _ => self != other,
        }
    }
}

/// Convert IEEE 754 binary16 bits to `f32` (exact).
#[cfg(feature = "float32")]
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h as u32) & 0x8000) << 16;
    let exp = ((h >> 10) & 0x1F) as u32;
    let man = (h & 0x3FF) as u32;
    let bits = match exp {
        0 if man == 0 => sign,
        0 => {
            // Subnormal: man * 2^-24.
            let v = man as f32 * (1.0 / 16_777_216.0);
            return if sign != 0 { -v } else { v };
        }
        0x1F => sign | 0x7F80_0000 | (man << 13),
        _ => sign | ((exp + 112) << 23) | (man << 13),
    };
    f32::from_bits(bits)
}

#[cfg(feature = "float32")]
fn float32_exceeds_threshold(current: f32, previous: f32, threshold: f32) -> bool {
    if !current.is_finite() || !previous.is_finite() {
        return if current.is_nan() && previous.is_nan() {
            false
        } else {
            current != previous
        };
    }
    if current == previous || !threshold.is_finite() || threshold < 0.0 {
        return false;
    }

    let difference = (current - previous).abs();
    let rounding_tolerance = f32::EPSILON * current.abs().max(previous.abs()).max(threshold) * 2.0;
    difference >= threshold || threshold - difference <= rounding_tolerance
}

#[cfg(feature = "float64")]
fn float64_exceeds_threshold(current: f64, previous: f64, threshold: f64) -> bool {
    if !current.is_finite() || !previous.is_finite() {
        return if current.is_nan() && previous.is_nan() {
            false
        } else {
            current != previous
        };
    }
    if current == previous || !threshold.is_finite() || threshold < 0.0 {
        return false;
    }

    let difference = (current - previous).abs();
    let rounding_tolerance = f64::EPSILON * current.abs().max(previous.abs()).max(threshold) * 2.0;
    difference >= threshold || threshold - difference <= rounding_tolerance
}

/// Convenience free function: serialize a value into `buf`.
/// Returns the number of bytes written.
pub fn serialize_value(val: &ZclValue, buf: &mut [u8]) -> usize {
    val.serialize(buf)
}

/// Convenience free function: parse a value of a known type from `data`.
/// Returns `(value, bytes_consumed)`.
pub fn parse_value(data_type: ZclDataType, data: &[u8]) -> Option<(ZclValue, usize)> {
    ZclValue::deserialize(data_type, data)
}

/// Whether a data type is "analog" (supports reportable change thresholds).
pub fn is_analog_type(dt: ZclDataType) -> bool {
    matches!(
        dt,
        ZclDataType::U8
            | ZclDataType::U16
            | ZclDataType::U24
            | ZclDataType::U32
            | ZclDataType::U48
            | ZclDataType::U64
            | ZclDataType::I8
            | ZclDataType::I16
            | ZclDataType::I32
            | ZclDataType::I64
            | ZclDataType::Float16
            | ZclDataType::Float32
            | ZclDataType::Float64
            | ZclDataType::UtcTime
            | ZclDataType::TimeOfDay
            | ZclDataType::Date
    )
}

/// Whether a data type is "discrete" (does not support reportable change).
pub fn is_discrete_type(dt: ZclDataType) -> bool {
    !is_analog_type(dt)
}

#[cfg(test)]
mod tests {
    #[cfg(any(not(feature = "float32"), not(feature = "float64")))]
    use super::is_data_type_enabled;
    use super::{ZclDataType, ZclValue};

    #[test]
    fn data_type_size_matches_zcl_table() {
        use ZclDataType::*;
        let fixed = [
            (NoData, 0),
            (Bool, 1),
            (Bitmap8, 1),
            (Bitmap16, 2),
            (Bitmap32, 4),
            (Bitmap64, 8),
            (U8, 1),
            (U16, 2),
            (U24, 3),
            (U32, 4),
            (U48, 6),
            (U64, 8),
            (I8, 1),
            (I16, 2),
            (I32, 4),
            (I64, 8),
            (Enum8, 1),
            (Enum16, 2),
            (Float16, 2),
            (Float32, 4),
            (Float64, 8),
            (TimeOfDay, 4),
            (Date, 4),
            (UtcTime, 4),
            (ClusterId, 2),
            (AttributeId, 2),
            (BacNetOid, 4),
            (IeeeAddr, 8),
            (SecurityKey128, 16),
        ];
        for (dt, n) in fixed {
            assert_eq!(super::data_type_size(dt), Some(n), "{dt:?}");
        }
        for dt in [
            OctetString,
            CharString,
            OctetString16,
            CharString16,
            Array,
            Struct,
            Set,
            Bag,
        ] {
            assert_eq!(super::data_type_size(dt), None, "{dt:?}");
        }
    }

    #[test]
    fn serialize_never_panics_on_short_buffers() {
        let mut long = heapless::Vec::new();
        long.extend_from_slice(&[b'x'; super::MAX_STRING_LEN])
            .unwrap();
        let values = [
            ZclValue::NoData,
            ZclValue::Bool(true),
            ZclValue::U8(1),
            ZclValue::I16(-2),
            ZclValue::U24(0x123456),
            ZclValue::I32(-3),
            ZclValue::U48(0x1234_5678_9ABC),
            ZclValue::I64(-4),
            ZclValue::Float16(0x3C00),
            ZclValue::Float32(1.5),
            ZclValue::Float64(2.5),
            ZclValue::IeeeAddr(5),
            ZclValue::SecurityKey128([7; 16]),
            ZclValue::CharString(long.clone()),
            ZclValue::OctetString(long),
        ];
        for v in &values {
            let n = v.wire_len();
            for len in 0..n {
                let mut buf = [0xAAu8; 64];
                assert_eq!(v.try_serialize(&mut buf[..len]), None);
                assert_eq!(v.serialize(&mut buf[..len]), 0);
                assert!(buf.iter().all(|&b| b == 0xAA), "partial write for {v:?}");
            }
            let mut buf = [0u8; 64];
            assert_eq!(v.try_serialize(&mut buf[..n]), Some(n));
            if super::is_data_type_enabled(v.data_type()) && *v != ZclValue::NoData {
                let (back, used) = ZclValue::deserialize(v.data_type(), &buf[..n]).unwrap();
                assert_eq!(used, n);
                assert_eq!(&back, v);
            }
        }
    }

    #[test]
    fn integer_thresholds_cover_all_widths_and_signed_magnitudes() {
        // U24 / U48 / I64 / UtcTime used to fall back to `!=`.
        assert!(!ZclValue::U24(100).exceeds_threshold(&ZclValue::U24(99), &ZclValue::U24(10)));
        assert!(ZclValue::U24(110).exceeds_threshold(&ZclValue::U24(100), &ZclValue::U24(10)));
        assert!(
            !ZclValue::U48(1_000_001)
                .exceeds_threshold(&ZclValue::U48(1_000_000), &ZclValue::U48(100))
        );
        assert!(
            ZclValue::U48(1_000_100)
                .exceeds_threshold(&ZclValue::U48(1_000_000), &ZclValue::U48(100))
        );
        assert!(!ZclValue::I64(-5).exceeds_threshold(&ZclValue::I64(-4), &ZclValue::I64(2)));
        assert!(
            ZclValue::I64(i64::MIN).exceeds_threshold(&ZclValue::I64(i64::MAX), &ZclValue::I64(2))
        );
        assert!(
            !ZclValue::UtcTime(5).exceeds_threshold(&ZclValue::UtcTime(4), &ZclValue::UtcTime(60))
        );
        // Negative signed thresholds are magnitudes, not huge unsigned casts.
        assert!(ZclValue::I8(-10).exceeds_threshold(&ZclValue::I8(0), &ZclValue::I8(-5)));
        assert!(!ZclValue::I8(-4).exceeds_threshold(&ZclValue::I8(0), &ZclValue::I8(-5)));
        assert!(ZclValue::I16(100).exceeds_threshold(&ZclValue::I16(-100), &ZclValue::I16(-200)));
        assert!(ZclValue::I32(-7).exceeds_threshold(&ZclValue::I32(0), &ZclValue::I32(-7)));
        assert!(ZclValue::I8(127).exceeds_threshold(&ZclValue::I8(-128), &ZclValue::I8(-128)));
        // A zero threshold reports any change, but never an unchanged value.
        assert!(!ZclValue::U16(5).exceeds_threshold(&ZclValue::U16(5), &ZclValue::U16(0)));
        assert!(ZclValue::U16(6).exceeds_threshold(&ZclValue::U16(5), &ZclValue::U16(0)));
    }

    #[test]
    fn float16_is_decoded_consistently_with_analog_classification() {
        assert!(super::is_analog_type(ZclDataType::Float16));
        let (v, n) = ZclValue::deserialize(ZclDataType::Float16, &[0x00, 0x3C]).unwrap();
        assert_eq!((v.clone(), n), (ZclValue::Float16(0x3C00), 2));
        assert_eq!(v.data_type(), ZclDataType::Float16);
        // 1.0 -> 2.0 crosses 0.5; 1.0 -> 1.0 does not.
        assert!(
            ZclValue::Float16(0x4000)
                .exceeds_threshold(&ZclValue::Float16(0x3C00), &ZclValue::Float16(0x3800))
        );
        assert!(
            !ZclValue::Float16(0x3C00)
                .exceeds_threshold(&ZclValue::Float16(0x3C00), &ZclValue::Float16(0x3800))
        );
    }

    #[cfg(feature = "float32")]
    #[test]
    fn f16_conversion_handles_special_values() {
        use super::f16_to_f32;
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
        assert_eq!(f16_to_f32(0x0001), 5.960_464_5e-8);
        assert!(f16_to_f32(0x7C00).is_infinite());
        assert!(f16_to_f32(0x7E00).is_nan());
        // 1.0 -> 1.25 is below a 0.5 threshold.
        assert!(
            !ZclValue::Float16(0x3D00)
                .exceeds_threshold(&ZclValue::Float16(0x3C00), &ZclValue::Float16(0x3800))
        );
    }

    #[cfg(feature = "float32")]
    #[test]
    fn float32_thresholds_compare_finite_absolute_difference() {
        let previous = ZclValue::Float32(1_000.0);
        assert!(!ZclValue::Float32(1_049.9).exceeds_threshold(&previous, &ZclValue::Float32(50.0)));
        assert!(ZclValue::Float32(1_050.0).exceeds_threshold(&previous, &ZclValue::Float32(50.0)));

        let fractional_previous = ZclValue::Float32(1_000.0e-6);
        assert!(
            !ZclValue::Float32(1_049.9e-6)
                .exceeds_threshold(&fractional_previous, &ZclValue::Float32(50.0e-6))
        );
        assert!(
            ZclValue::Float32(1_050.0e-6)
                .exceeds_threshold(&fractional_previous, &ZclValue::Float32(50.0e-6))
        );

        assert!(
            !ZclValue::Float32(2_000.0).exceeds_threshold(&previous, &ZclValue::Float32(f32::NAN))
        );
        assert!(
            !ZclValue::Float32(2_000.0)
                .exceeds_threshold(&previous, &ZclValue::Float32(f32::INFINITY))
        );
    }

    #[cfg(feature = "float64")]
    #[test]
    fn float64_thresholds_compare_finite_absolute_difference() {
        let previous = ZclValue::Float64(-2.0);
        assert!(!ZclValue::Float64(2.99).exceeds_threshold(&previous, &ZclValue::Float64(5.0)));
        assert!(ZclValue::Float64(3.0).exceeds_threshold(&previous, &ZclValue::Float64(5.0)));
    }

    #[cfg(all(feature = "float32", feature = "float64"))]
    #[test]
    fn non_finite_float_values_do_not_repeat_without_a_transition() {
        assert!(
            !ZclValue::Float32(f32::NAN)
                .exceeds_threshold(&ZclValue::Float32(f32::NAN), &ZclValue::Float32(1.0),)
        );
        assert!(
            ZclValue::Float32(f32::NAN)
                .exceeds_threshold(&ZclValue::Float32(1.0), &ZclValue::Float32(1.0),)
        );
        assert!(
            !ZclValue::Float64(f64::INFINITY)
                .exceeds_threshold(&ZclValue::Float64(f64::INFINITY), &ZclValue::Float64(1.0),)
        );
        assert!(
            ZclValue::Float64(f64::NEG_INFINITY)
                .exceeds_threshold(&ZclValue::Float64(f64::INFINITY), &ZclValue::Float64(1.0),)
        );
    }

    #[cfg(all(feature = "float32", feature = "float64"))]
    #[test]
    fn mismatched_float_types_keep_change_detection_semantics() {
        assert!(
            ZclValue::Float32(1.0)
                .exceeds_threshold(&ZclValue::Float64(1.0), &ZclValue::Float32(1.0),)
        );
        assert!(
            !ZclValue::Float32(1.0)
                .exceeds_threshold(&ZclValue::Float32(1.0), &ZclValue::Float64(1.0),)
        );
        assert!(
            ZclValue::Float32(2.0)
                .exceeds_threshold(&ZclValue::Float32(1.0), &ZclValue::Float64(1.0),)
        );
    }

    #[cfg(not(feature = "float32"))]
    #[test]
    fn disabled_float32_rejects_wire_values_without_losing_public_variant() {
        assert!(!is_data_type_enabled(ZclDataType::Float32));
        assert!(ZclValue::deserialize(ZclDataType::Float32, &[0, 0, 0, 0]).is_none());
        assert!(
            ZclValue::Float32(2.0)
                .exceeds_threshold(&ZclValue::Float32(1.0), &ZclValue::Float32(0.5))
        );
    }

    #[cfg(not(feature = "float64"))]
    #[test]
    fn disabled_float64_rejects_wire_values_without_losing_public_variant() {
        assert!(!is_data_type_enabled(ZclDataType::Float64));
        assert!(ZclValue::deserialize(ZclDataType::Float64, &[0; 8]).is_none());
        assert!(
            !ZclValue::Float64(1.0)
                .exceeds_threshold(&ZclValue::Float64(1.0), &ZclValue::Float64(0.5))
        );
    }
}
