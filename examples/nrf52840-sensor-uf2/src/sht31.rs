//! SHT31 I2C driver — minimal no_std, single-shot mode for battery devices.
//!
//! Implements the Sensirion SHT31 temperature/humidity sensor protocol.
//! Uses single-shot mode (clock stretching disabled) to minimize power.
//! Fully async — yields during I2C DMA and timer waits.

use embassy_nrf::peripherals;
use embassy_nrf::twim::Twim;
use embassy_time::{Duration, Timer};

use defmt::*;

/// Single-shot, high repeatability, no clock stretching
const CMD_MEASURE_HIGH: [u8; 2] = [0x24, 0x00];
/// Soft reset
const CMD_SOFT_RESET: [u8; 2] = [0x30, 0xA2];
/// Read status register
const CMD_READ_STATUS: [u8; 2] = [0xF3, 0x2D];

pub struct Sht31Data {
    /// Temperature in centidegrees (2350 = 23.50°C)
    pub temperature_centideg: i16,
    /// Humidity in centipercent (6500 = 65.00%)
    pub humidity_centipct: u16,
}

/// Initialize the SHT31 sensor. Soft-resets and verifies communication.
pub async fn init(i2c: &mut Twim<'_, peripherals::TWISPI0>, addr: u8) -> bool {
    // Soft reset
    if i2c.write(addr, &CMD_SOFT_RESET).await.is_err() {
        warn!("SHT31: soft reset failed");
        return false;
    }
    Timer::after(Duration::from_millis(2)).await;

    // Read status register to verify communication
    let mut status_buf = [0u8; 3]; // 2 data + 1 CRC
    if i2c
        .write_read(addr, &CMD_READ_STATUS, &mut status_buf)
        .await
        .is_err()
    {
        warn!("SHT31: status read failed — check wiring");
        return false;
    }

    // Verify CRC of status register
    if crc8(&status_buf[0..2]) != status_buf[2] {
        warn!("SHT31: status CRC mismatch");
        return false;
    }

    info!("SHT31: status=0x{:04X}", u16::from_be_bytes([status_buf[0], status_buf[1]]));
    true
}

/// Read temperature and humidity from SHT31 (single-shot, high repeatability).
///
/// Returns None on I2C error or CRC failure.
pub async fn read(i2c: &mut Twim<'_, peripherals::TWISPI0>, addr: u8) -> Option<Sht31Data> {
    // Trigger single-shot measurement (high repeatability, no clock stretching)
    if i2c.write(addr, &CMD_MEASURE_HIGH).await.is_err() {
        return None;
    }

    // High repeatability takes max 15.5 ms — wait 20 ms to be safe
    Timer::after(Duration::from_millis(20)).await;

    // Read 6 bytes: [temp_msb, temp_lsb, temp_crc, hum_msb, hum_lsb, hum_crc]
    let mut raw = [0u8; 6];
    // Retry up to 3 times (NACK means measurement not ready yet)
    let mut ok = false;
    for _ in 0..3 {
        if i2c.read(addr, &mut raw).await.is_ok() {
            ok = true;
            break;
        }
        Timer::after(Duration::from_millis(5)).await;
    }
    if !ok {
        warn!("SHT31: read timeout");
        return None;
    }

    // Verify CRCs
    if crc8(&raw[0..2]) != raw[2] {
        warn!("SHT31: temperature CRC error");
        return None;
    }
    if crc8(&raw[3..5]) != raw[5] {
        warn!("SHT31: humidity CRC error");
        return None;
    }

    let raw_temp = u16::from_be_bytes([raw[0], raw[1]]);
    let raw_hum = u16::from_be_bytes([raw[3], raw[4]]);

    // Temperature: -45 + 175 * (raw / 65535) °C → centidegrees
    // = (-4500 + 17500 * raw / 65535) = (-4500 + 17500 * raw / 65535)
    // Use i32 to avoid overflow: (17500 * raw) fits in i32 easily
    let temp_centideg = -4500i32 + (17500i32 * raw_temp as i32) / 65535;

    // Humidity: 100 * (raw / 65535) %RH → centipercent
    // = 10000 * raw / 65535
    let hum_centipct = (10000i32 * raw_hum as i32) / 65535;

    Some(Sht31Data {
        temperature_centideg: temp_centideg as i16,
        humidity_centipct: hum_centipct.clamp(0, 10000) as u16,
    })
}

/// CRC-8 for Sensirion sensors: polynomial 0x31, init 0xFF
fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0xFF;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x31;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}
