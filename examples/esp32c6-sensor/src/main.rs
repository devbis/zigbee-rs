//! # ESP32-C6 Zigbee Temperature & Humidity Sensor
//!
//! A complete no_std firmware for the ESP32-C6, implementing a Zigbee 3.0
//! end device that reads an SHT31 I2C sensor and exposes measurements via
//! ZCL Temperature Measurement (0x0402) and Relative Humidity (0x0405) clusters.
//!
//! ## Hardware
//! - ESP32-C6 (RISC-V, built-in IEEE 802.15.4 radio)
//! - SHT31 I2C sensor: SDA → GPIO6, SCL → GPIO7
//!
//! ## Alternative sensors
//! - **BME280** (temp + humidity + pressure): use `bme280 = "0.5"` crate,
//!   add Pressure Measurement cluster (0x0403)
//! - **SHTC3** (Sensirion, lower power): use `shtcx = "1.0"` crate,
//!   drop-in replacement for the SHT3x reading logic
//! - **AHT20** (budget option): use `aht20 = "0.3"` crate,
//!   same I2C bus, different address (0x38)
//! - **Si7021**: use `si7021 = "0.3"` crate, compatible API

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::prelude::*;

use zigbee_aps::PROFILE_HOME_AUTOMATION;
use zigbee_nwk::DeviceType;
use zigbee_runtime::event_loop::{self, StackEvent, TickResult};
use zigbee_zcl::clusters::basic::BasicCluster;
use zigbee_zcl::clusters::humidity::HumidityCluster;
use zigbee_zcl::clusters::temperature::TemperatureCluster;

// ---------------------------------------------------------------------------
// ZCL cluster IDs used when registering endpoints
// ---------------------------------------------------------------------------
const CLUSTER_BASIC: u16 = 0x0000;
const CLUSTER_TEMPERATURE: u16 = 0x0402;
const CLUSTER_HUMIDITY: u16 = 0x0405;

// HA device type: Temperature Sensor (0x0302)
const DEVICE_TYPE_TEMP_SENSOR: u16 = 0x0302;

// Sensor reading interval
const READING_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Main entry point — Embassy async executor on the ESP32-C6
// ---------------------------------------------------------------------------
#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
    // -----------------------------------------------------------------------
    // 1. Peripheral initialisation
    // -----------------------------------------------------------------------
    let peripherals = esp_hal::init(esp_hal::Config::default());

    esp_println::println!("[init] ESP32-C6 Zigbee sensor starting");

    // Embassy time driver — uses SYSTIMER on ESP32-C6
    let timer0 = esp_hal::timer::systimer::SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    // -----------------------------------------------------------------------
    // 2. I2C bus for the SHT31 sensor
    //    SDA → GPIO6, SCL → GPIO7 (adjust for your board layout)
    // -----------------------------------------------------------------------
    let i2c = esp_hal::i2c::master::I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(100.kHz()),
    )
    .with_sda(peripherals.GPIO6)
    .with_scl(peripherals.GPIO7);

    // sht3x crate uses blocking embedded-hal I2C — fine for short transfers.
    // Address::Low → 0x44 (ADDR pin to GND). Use Address::High for 0x45.
    let mut sht = sht3x::Sht3x::new(i2c, sht3x::Address::Low);

    esp_println::println!("[init] SHT31 on I2C0 (GPIO6/GPIO7) @ 0x44");

    // -----------------------------------------------------------------------
    // 3. IEEE 802.15.4 MAC driver (ESP32-C6 on-chip radio)
    //
    // The `zigbee-mac` crate's "esp32c6" feature exposes a driver that wraps
    // the `esp-radio` IEEE 802.15.4 peripheral through the `MacDriver` trait.
    // -----------------------------------------------------------------------
    let radio = esp_ieee802154::Ieee802154::new(peripherals.IEEE802154, peripherals.RADIO_CLK);
    let mac = zigbee_mac::Esp32c6Mac::new(radio);

    esp_println::println!("[init] IEEE 802.15.4 radio ready");

    // -----------------------------------------------------------------------
    // 4. ZCL cluster instances
    //
    // Temperature: range −40.00 °C … +125.00 °C (in hundredths of °C)
    // Humidity:    range   0.00 % … 100.00 %   (in hundredths of %)
    // -----------------------------------------------------------------------
    let mut temp_cluster = TemperatureCluster::new(-4000, 12500);
    let mut humidity_cluster = HumidityCluster::new(0, 10000);

    let _basic = BasicCluster::new(
        b"Zigbee-RS",        // manufacturer name
        b"ESP32-C6 Sensor",  // model identifier
        b"20250101",         // date code
        b"0.1.0",            // software build
    );

    // -----------------------------------------------------------------------
    // 5. Build the Zigbee device using the runtime builder
    //
    // Endpoint 1 — HA Temperature Sensor with server-side clusters:
    //   • Basic          (0x0000) — mandatory on every endpoint
    //   • Temperature    (0x0402) — measured value in 0.01 °C
    //   • Rel. Humidity  (0x0405) — measured value in 0.01 %
    // -----------------------------------------------------------------------
    let mut device = zigbee_runtime::builder(mac)
        .device_type(DeviceType::EndDevice)
        .manufacturer("Zigbee-RS")
        .model("ESP32-C6 Sensor")
        .sw_build("0.1.0")
        .channels(zigbee_types::ChannelMask::default()) // all channels
        .endpoint(1, PROFILE_HOME_AUTOMATION, DEVICE_TYPE_TEMP_SENSOR, |ep| {
            ep.cluster_server(CLUSTER_BASIC)
                .cluster_server(CLUSTER_TEMPERATURE)
                .cluster_server(CLUSTER_HUMIDITY)
        })
        .build();

    esp_println::println!("[init] Zigbee device configured — joining network…");

    // -----------------------------------------------------------------------
    // 6. Main loop — sensor reading + Zigbee stack processing
    // -----------------------------------------------------------------------
    let mut joined = false;

    loop {
        // --- 6a. Tick the Zigbee stack (processes radio events, NWK, APS) --
        match event_loop::stack_tick(&mut device).await {
            TickResult::Event(StackEvent::Joined { short_address, channel, pan_id }) => {
                esp_println::println!(
                    "[zigbee] Joined! addr=0x{:04X} ch={} pan=0x{:04X}",
                    short_address.0,
                    channel as u8,
                    pan_id.0,
                );
                joined = true;
            }
            TickResult::Event(StackEvent::Left) => {
                esp_println::println!("[zigbee] Left network — will rejoin");
                joined = false;
            }
            TickResult::Event(StackEvent::CommissioningComplete { success }) => {
                esp_println::println!("[zigbee] Commissioning complete: {}", success);
            }
            TickResult::Event(event) => {
                esp_println::println!("[zigbee] Event: {:?}", event);
            }
            TickResult::RunAgain(delay_ms) => {
                // Stack wants to be called again soon (e.g. retry)
                Timer::after(Duration::from_millis(delay_ms as u64)).await;
                continue;
            }
            TickResult::Idle => {}
        }

        // --- 6b. Read the SHT31 and update ZCL attributes -----------------
        if joined {
            // Blocking I2C read — typically < 20 ms for a single-shot measurement.
            // The sht3x crate's `measure` uses a clock-stretching single-shot command.
            match sht.measure(sht3x::Repeatability::High, &mut esp_hal::delay::Delay) {
                Ok(measurement) => {
                    // SHT31 returns temperature in °C and humidity in %RH as f32.
                    // ZCL expects signed hundredths for temperature, unsigned for humidity.
                    let temp_100 = (measurement.temperature * 100.0) as i16;
                    let hum_100 = (measurement.humidity * 100.0) as u16;

                    temp_cluster.set_temperature(temp_100);
                    humidity_cluster.set_humidity(hum_100);

                    esp_println::println!(
                        "[sensor] T={}.{:02}°C  H={}.{:02}%",
                        temp_100 / 100,
                        (temp_100 % 100).unsigned_abs(),
                        hum_100 / 100,
                        hum_100 % 100,
                    );
                }
                Err(_e) => {
                    esp_println::println!("[sensor] SHT31 read error");
                }
            }
        }

        // --- 6c. Sleep until next reading ----------------------------------
        Timer::after(Duration::from_secs(READING_INTERVAL_SECS)).await;
    }
}
