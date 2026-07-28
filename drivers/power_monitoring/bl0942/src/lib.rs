//! Platform-agnostic BL0942 energy metering IC driver (UART).
//!
//! The BL0942 communicates via UART at 4800 baud, 8N1.
//! It sends a 24-byte packet containing voltage, current, power, energy, and frequency.
//!
//! # Packet Structure (from `app_monitoring.h`)
//!
//! ```text
//! Byte  | Field       | Bits  | Description
//! ------|-------------|-------|----------------------------------
//! 0     | head        | 8     | 0x55
//! 1-3   | i_rms       | 24    | Current RMS (ADC counts)
//! 4-6   | v_rms       | 24    | Voltage RMS (ADC counts)
//! 7-9   | i_fast_rms  | 24    | Fast current RMS
//! 10-12 | watt        | 24    | Active power (signed, ADC counts)
//! 13-15 | cf_cnt      | 24    | Energy pulse count
//! 16-17 | freq        | 16    | Frequency (microseconds period)
//! 18    | resv1       | 8     | Reserved
//! 19    | status      | 8     | Status register
//! 20    | resv2       | 8     | Reserved
//! 21    | resv3       | 8     | Reserved
//! 22    | crc         | 8     | Checksum
//! 23    | (padding)   | 8     | Total 23 bytes + padding = 24
//! ```
//!
//! # Checksum
//!
//! Sum all bytes except the last (CRC), add 0x58, then invert (bitwise NOT).
//!
//! # Calibration Constants
//!
//! Obtained from ESPHome's BL0942 driver:
//! - POWER_REF: 596.0
//! - VOLTAGE_REF: 15873.3594
//! - CURRENT_REF: 251213.4647
//! - ENERGY_REF: 3304.6113
//!
//! # Example
//!
//! ```ignore
//! let mut bl0942 = Bl0942::new(uart, CALIBRATION);
//! loop {
//!     if let Some(reading) = bl0942.try_read() {
//!         // reading.voltage_rms in centi-volts
//!         // reading.current_rms in centi-amps
//!         // reading.active_power in watts
//!         // reading.energy_wh in Wh
//!     }
//! }
//! ```

#![no_std]

/// UART abstraction for BL0942 communication.
///
/// Platform implements this trait to provide UART read/write.
pub trait Uart {
    /// Write bytes to UART. Returns number of bytes written.
    fn write(&mut self, data: &[u8]) -> Result<usize, UartError>;

    /// Read available bytes into buffer. Returns number of bytes read.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError>;

    /// Flush the UART receive buffer.
    fn flush_rx(&mut self) -> Result<(), UartError>;
}

/// UART error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UartError {
    /// UART hardware error.
    Hardware,
    /// No data available.
    NoData,
    /// Buffer overflow.
    Overflow,
}

/// Calibration constants for converting ADC counts to physical values.
///
/// Obtained from ESPHome's BL0942 driver and confirmed against the C firmware.
#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    /// POWER_REF: multiply raw power ADC by this factor to get watts.
    pub power_ref: f32,
    /// VOLTAGE_REF: divide raw voltage ADC by this to get centi-volts.
    pub voltage_ref: f32,
    /// CURRENT_REF: divide raw current ADC by this to get centi-amps.
    pub current_ref: f32,
    /// ENERGY_REF: divide raw energy ADC by this to get centi-Wh.
    pub energy_ref: f32,
}

impl Calibration {
    /// Default calibration from ESPHome's BL0942 driver.
    pub const DEFAULT: Self = Self {
        power_ref: 596.0,
        voltage_ref: 15_873.359,
        current_ref: 251_213.465,
        energy_ref: 3_304.611,
    };
}

/// One measurement reading from the BL0942.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// RMS voltage in centi-volts (e.g. 23000 = 230.00 V).
    pub voltage_rms: u16,
    /// RMS current in centi-amps (e.g. 150 = 1.50 A).
    pub current_rms: u16,
    /// Active power in watts (signed for bidirectional metering).
    pub active_power: i16,
    /// Energy pulse count (for accumulation).
    pub energy_pulses: u32,
    /// Line frequency in centi-Hz (e.g. 5000 = 50.00 Hz).
    pub frequency: u16,
}

/// Raw packet from BL0942 (24 bytes).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct RawPacket {
    head: u8,        // 0x55
    i_rms: [u8; 3],  // Current RMS (24-bit, little-endian)
    v_rms: [u8; 3],  // Voltage RMS (24-bit, little-endian)
    i_fast: [u8; 3], // Fast current RMS (24-bit)
    watt: [u8; 3],   // Active power (24-bit, signed)
    cf_cnt: [u8; 3], // Energy pulse count (24-bit)
    freq: [u8; 2],   // Frequency (16-bit, microseconds period)
    resv1: u8,
    status: u8,
    resv2: u8,
    resv3: u8,
    crc: u8,
}

impl RawPacket {
    const HEAD: u8 = 0x55;
    const SIZE: usize = 23; // Without padding

    /// Validate the packet header and checksum.
    fn is_valid(&self) -> bool {
        if self.head != Self::HEAD {
            return false;
        }
        let data = unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                Self::SIZE,
            )
        };
        checksum(data) == self.crc
    }

    /// Read 24-bit value from a 3-byte array (little-endian).
    fn read_u24(bytes: &[u8; 3]) -> u32 {
        u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
    }

    /// Read signed 24-bit value from a 3-byte array (little-endian).
    fn read_i24(bytes: &[u8; 3]) -> i32 {
        let raw = Self::read_u24(bytes);
        if raw & 0x800000 != 0 {
            // Sign extend
            (raw | 0xFF000000) as i32
        } else {
            raw as i32
        }
    }
}

/// Calculate checksum: sum all bytes except the last (CRC), add 0x58, then bitwise NOT.
///
/// Matches the C implementation: `for i in 0..(length-1) { crc += data[i]; }`.
fn checksum(data: &[u8]) -> u8 {
    let mut crc8: u8 = 0;
    for i in 0..(data.len().saturating_sub(1)) {
        crc8 = crc8.wrapping_add(data[i]);
    }
    crc8 = crc8.wrapping_add(0x58);
    !crc8
}

/// BL0942 energy metering IC driver.
///
/// Generic over `UART` platform abstraction.
pub struct Bl0942<U: Uart> {
    uart: U,
    calibration: Calibration,
    /// Buffer for incoming UART data.
    rx_buf: [u8; 32],
    /// Number of valid bytes in rx_buf.
    rx_len: usize,
    /// Last valid reading.
    last_reading: Option<Reading>,
}

impl<U: Uart> Bl0942<U> {
    /// Create a new BL0942 driver.
    pub fn new(uart: U, calibration: Calibration) -> Self {
        Self {
            uart,
            calibration,
            rx_buf: [0u8; 32],
            rx_len: 0,
            last_reading: None,
        }
    }

    /// Create a new BL0942 driver with default calibration.
    pub fn with_default_calibration(uart: U) -> Self {
        Self::new(uart, Calibration::DEFAULT)
    }

    /// Try to read a complete packet from UART.
    ///
    /// Returns `Some(reading)` if a valid packet was received and parsed,
    /// `None` if no complete packet is available yet.
    pub fn try_read(&mut self) -> Option<Reading> {
        // Read available bytes into buffer
        let mut tmp = [0u8; 32];
        match self.uart.read(&mut tmp) {
            Ok(n) if n > 0 => {
                // Append to rx_buf
                let space = self.rx_buf.len() - self.rx_len;
                let to_copy = n.min(space);
                self.rx_buf[self.rx_len..self.rx_len + to_copy]
                    .copy_from_slice(&tmp[..to_copy]);
                self.rx_len += to_copy;
            }
            _ => return None,
        }

        // Search for a complete packet
        loop {
            // Find header byte
            let start = match self.rx_buf[..self.rx_len]
                .iter()
                .position(|&b| b == RawPacket::HEAD)
            {
                Some(pos) => pos,
                None => {
                    self.rx_len = 0;
                    return None;
                }
            };

            // Check if we have enough bytes
            if self.rx_len - start < RawPacket::SIZE {
                // Need more data
                // Shift buffer to remove consumed bytes
                if start > 0 {
                    self.rx_buf.copy_within(start..self.rx_len, 0);
                    self.rx_len -= start;
                }
                return None;
            }

            // Try to parse packet at this position
            let packet = unsafe {
                core::ptr::read(self.rx_buf[start..].as_ptr() as *const RawPacket)
            };

            if packet.is_valid() {
                // Shift buffer past this packet
                let consumed = start + RawPacket::SIZE;
                if consumed < self.rx_len {
                    self.rx_buf.copy_within(consumed..self.rx_len, 0);
                    self.rx_len -= consumed;
                } else {
                    self.rx_len = 0;
                }

                // Parse the packet
                let reading = self.parse_packet(&packet);
                self.last_reading = Some(reading);
                return Some(reading);
            }

            // Invalid packet, skip header and try again
            self.rx_buf.copy_within((start + 1)..self.rx_len, 0);
            self.rx_len -= start + 1;
        }
    }

    /// Parse a raw packet into a Reading.
    fn parse_packet(&self, packet: &RawPacket) -> Reading {
        let i_rms_raw = RawPacket::read_u24(&packet.i_rms);
        let v_rms_raw = RawPacket::read_u24(&packet.v_rms);
        let watt_raw = RawPacket::read_i24(&packet.watt);
        let cf_raw = RawPacket::read_u24(&packet.cf_cnt);
        let freq_raw = u16::from_le_bytes(packet.freq);

        // Convert to physical values (matching C firmware formulas)
        let voltage = (v_rms_raw as f32 / self.calibration.voltage_ref * 100.0) as u16;
        let current = (i_rms_raw as f32 / self.calibration.current_ref * 100.0) as u16;
        let power = (watt_raw as f32 / self.calibration.power_ref) as i16;
        let energy = (cf_raw as f32 / self.calibration.energy_ref * 100.0) as u32;

        // Frequency: freq_raw is in microseconds period
        // freq_Hz = 1_000_000 / freq_raw
        // freq_centiHz = freq_Hz * 100 = 100_000_000 / freq_raw
        let freq = if freq_raw > 0 {
            (100_000_000u32 / freq_raw as u32) as u16
        } else {
            0
        };

        Reading {
            voltage_rms: voltage,
            current_rms: current,
            active_power: power,
            energy_pulses: energy,
            frequency: freq,
        }
    }

    /// Get the last valid reading without re-reading UART.
    pub fn last_reading(&self) -> Option<Reading> {
        self.last_reading
    }

    /// Release the underlying UART peripheral.
    pub fn release(self) -> U {
        self.uart
    }
}

/// Convert centi-Wh to Wh.
#[inline]
pub fn centi_wh_to_wh(centi_wh: u32) -> u64 {
    centi_wh as u64 / 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_basic() {
        // Checksum sums all bytes except the last (CRC), adds 0x58, then NOT
        // For [0x55, 0x00, 0x00, 0x00]: sum first 3 = 0x55, 0x55+0x58=0xAD, !0xAD=0x52
        let mut data = [0u8; 4];
        data[0] = 0x55; // head
        let crc = checksum(&data);
        assert_eq!(crc, 0x52);
    }

    #[test]
    fn checksum_known_value() {
        // Hand-computed: sum of first (len-1) bytes + 0x58, then NOT
        let data = [0x55, 0x01, 0x02, 0x03];
        // Sum first 3 bytes: 0x55 + 0x01 + 0x02 = 0x58
        // 0x58 + 0x58 = 0xB0
        // !0xB0 = 0x4F
        assert_eq!(checksum(&data), 0x4F);
    }

    #[test]
    fn read_u24_little_endian() {
        let bytes = [0x10, 0x20, 0x30];
        assert_eq!(RawPacket::read_u24(&bytes), 0x302010);
    }

    #[test]
    fn read_u24_zero() {
        let bytes = [0x00, 0x00, 0x00];
        assert_eq!(RawPacket::read_u24(&bytes), 0);
    }

    #[test]
    fn read_u24_max() {
        let bytes = [0xFF, 0xFF, 0xFF];
        assert_eq!(RawPacket::read_u24(&bytes), 0xFFFFFF);
    }

    #[test]
    fn read_i24_positive() {
        let bytes = [0x10, 0x20, 0x00];
        assert_eq!(RawPacket::read_i24(&bytes), 0x2010);
    }

    #[test]
    fn read_i24_negative() {
        let bytes = [0x00, 0x00, 0x80];
        assert_eq!(RawPacket::read_i24(&bytes), -8388608);
    }

    #[test]
    fn read_i24_minus_one() {
        let bytes = [0xFF, 0xFF, 0xFF];
        assert_eq!(RawPacket::read_i24(&bytes), -1);
    }

    #[test]
    fn calibration_default_values() {
        let cal = Calibration::DEFAULT;
        assert!((cal.power_ref - 596.0).abs() < 0.01);
        assert!((cal.voltage_ref - 15_873.359).abs() < 0.01);
        assert!((cal.current_ref - 251_213.465).abs() < 0.01);
        assert!((cal.energy_ref - 3_304.611).abs() < 0.01);
    }

    #[test]
    fn centi_wh_conversion() {
        assert_eq!(centi_wh_to_wh(100), 1);
        assert_eq!(centi_wh_to_wh(99), 0);
        assert_eq!(centi_wh_to_wh(150), 1);
        assert_eq!(centi_wh_to_wh(0), 0);
    }

    /// Mock UART for testing.
    struct MockUart {
        rx_data: heapless::Vec<u8, 64>,
        tx_data: heapless::Vec<u8, 64>,
    }

    impl MockUart {
        fn new() -> Self {
            Self {
                rx_data: heapless::Vec::new(),
                tx_data: heapless::Vec::new(),
            }
        }

        fn feed_packet(&mut self, packet: &RawPacket) {
            let bytes =
                unsafe { core::slice::from_raw_parts(packet as *const RawPacket as *const u8, 23) };
            self.rx_data.extend_from_slice(bytes).ok();
        }
    }

    impl Uart for MockUart {
        fn write(&mut self, data: &[u8]) -> Result<usize, UartError> {
            self.tx_data.extend_from_slice(data).map_err(|_| UartError::Overflow)?;
            Ok(data.len())
        }

        fn read(&mut self, buf: &mut [u8]) -> Result<usize, UartError> {
            if self.rx_data.is_empty() {
                return Err(UartError::NoData);
            }
            let n = buf.len().min(self.rx_data.len());
            buf[..n].copy_from_slice(&self.rx_data[..n]);
            self.rx_data.drain(..n);
            Ok(n)
        }

        fn flush_rx(&mut self) -> Result<(), UartError> {
            self.rx_data.clear();
            Ok(())
        }
    }

    #[test]
    fn mock_uart_read_packet() {
        let mut uart = MockUart::new();

        // Build a valid packet manually as bytes
        let mut pkt_bytes = [0u8; 23];
        pkt_bytes[0] = 0x55; // head
        pkt_bytes[1] = 0x00; // i_rms[0]
        pkt_bytes[2] = 0x10; // i_rms[1] = 4096
        pkt_bytes[3] = 0x00; // i_rms[2]
        pkt_bytes[4] = 0x00; // v_rms[0]
        pkt_bytes[5] = 0x80; // v_rms[1]
        pkt_bytes[6] = 0x01; // v_rms[2] = 102400
        // i_fast (7-9) = 0
        pkt_bytes[10] = 0x10; // watt[0]
        pkt_bytes[11] = 0x02; // watt[1] = 528
        pkt_bytes[12] = 0x00; // watt[2]
        // cf_cnt (13-15) = 0
        pkt_bytes[16] = 0x40; // freq[0]
        pkt_bytes[17] = 0x0D; // freq[1] = 3400 µs
        // resv1 (18) = 0
        // status (19) = 0
        // resv2 (20) = 0
        // resv3 (21) = 0

        // Calculate checksum over first 22 bytes
        let mut sum: u8 = 0;
        for &b in &pkt_bytes[..22] {
            sum = sum.wrapping_add(b);
        }
        sum = sum.wrapping_add(0x58);
        pkt_bytes[22] = !sum; // CRC

        uart.rx_data.extend_from_slice(&pkt_bytes).ok();

        let mut bl0942 = Bl0942::new(uart, Calibration::DEFAULT);
        let reading = bl0942.try_read();
        assert!(reading.is_some(), "Expected valid reading, got None");

        let r = reading.unwrap();
        // Voltage: 98304 / 15873.359 * 100 ≈ 619
        assert!(r.voltage_rms > 610 && r.voltage_rms < 630, "voltage_rms={}", r.voltage_rms);
        // Current: 4096 / 251213.465 * 100 ≈ 1
        assert!(r.current_rms < 5);
        // Power: 528 / 596 ≈ 0.89 → 0 (truncated)
        assert!(r.active_power >= 0 && r.active_power <= 1);
        // Frequency: 100_000_000 / 3400 ≈ 29411
        assert!(r.frequency > 29000 && r.frequency < 30000);
    }

    #[test]
    fn mock_uart_no_data() {
        let uart = MockUart::new();
        let mut bl0942 = Bl0942::new(uart, Calibration::DEFAULT);
        assert_eq!(bl0942.try_read(), None);
    }

    #[test]
    fn mock_uart_invalid_checksum() {
        let mut uart = MockUart::new();

        let mut pkt_bytes = [0u8; 23];
        pkt_bytes[0] = 0x55;
        // All other bytes = 0

        // Calculate correct checksum
        let mut sum: u8 = 0;
        for &b in &pkt_bytes[..22] {
            sum = sum.wrapping_add(b);
        }
        sum = sum.wrapping_add(0x58);
        pkt_bytes[22] = !sum;
        pkt_bytes[22] = pkt_bytes[22].wrapping_add(1); // Corrupt it

        uart.rx_data.extend_from_slice(&pkt_bytes).ok();

        let mut bl0942 = Bl0942::new(uart, Calibration::DEFAULT);
        assert_eq!(bl0942.try_read(), None);
    }
}
