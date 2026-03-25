//! # ESP32-C6 Zigbee Temperature & Humidity Sensor
//!
//! A complete no_std firmware for the ESP32-C6, implementing a Zigbee 3.0
//! end device that exposes temperature and humidity measurements via
//! ZCL Temperature Measurement (0x0402) and Relative Humidity (0x0405) clusters.
//!
//! ## Hardware
//! - ESP32-C6 (RISC-V, built-in IEEE 802.15.4 radio)
//!
//! ## Notes
//! Temperature/humidity values are simulated. To use a real sensor (e.g.
//! SHT31 over I2C), add an embedded-hal 1.0 compatible driver and replace
//! the simulated readings in the main loop.

#![no_std]
#![no_main]

use esp_backtrace as _;

use zigbee_aps::PROFILE_HOME_AUTOMATION;
use zigbee_nwk::DeviceType;
use zigbee_runtime::ZigbeeDevice;
use zigbee_zcl::clusters::basic::BasicCluster;
use zigbee_zcl::clusters::humidity::HumidityCluster;
use zigbee_zcl::clusters::temperature::TemperatureCluster;

const CLUSTER_BASIC: u16 = 0x0000;
const CLUSTER_TEMPERATURE: u16 = 0x0402;
const CLUSTER_HUMIDITY: u16 = 0x0405;
const DEVICE_TYPE_TEMP_SENSOR: u16 = 0x0302;
const READING_INTERVAL_SECS: u32 = 30;

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    esp_println::println!("[init] ESP32-C6 Zigbee sensor starting");

    // IEEE 802.15.4 MAC driver (ESP32-C6 on-chip radio)
    let ieee802154 = esp_radio::ieee802154::Ieee802154::new(peripherals.IEEE802154);
    let config = esp_radio::ieee802154::Config::default();
    let mac = zigbee_mac::esp::EspMac::new(ieee802154, config);

    esp_println::println!("[init] IEEE 802.15.4 radio ready");

    // ZCL cluster instances
    let mut temp_cluster = TemperatureCluster::new(-4000, 12500);
    let mut humidity_cluster = HumidityCluster::new(0, 10000);

    let _basic = BasicCluster::new(
        b"Zigbee-RS",
        b"ESP32-C6 Sensor",
        b"20250101",
        b"0.1.0",
    );

    // Build the Zigbee device
    let mut _device = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::EndDevice)
        .manufacturer("Zigbee-RS")
        .model("ESP32-C6 Sensor")
        .sw_build("0.1.0")
        .channels(zigbee_types::ChannelMask::ALL_2_4GHZ)
        .endpoint(1, PROFILE_HOME_AUTOMATION, DEVICE_TYPE_TEMP_SENSOR, |ep| {
            ep.cluster_server(CLUSTER_BASIC)
                .cluster_server(CLUSTER_TEMPERATURE)
                .cluster_server(CLUSTER_HUMIDITY)
        })
        .build();

    esp_println::println!("[init] Zigbee device configured — joining network…");

    let mut tick: u32 = 0;
    let delay = esp_hal::delay::Delay::new();

    loop {
        // Simulated sensor readings
        let temp_100: i16 = 2250 + ((tick % 50) as i16 - 25);
        let hum_100: u16 = 5500 + ((tick % 40) as u16).wrapping_sub(20);

        temp_cluster.set_temperature(temp_100);
        humidity_cluster.set_humidity(hum_100);

        esp_println::println!(
            "[sensor] T={}.{:02}°C  H={}.{:02}%",
            temp_100 / 100,
            (temp_100 % 100).unsigned_abs(),
            hum_100 / 100,
            hum_100 % 100,
        );

        tick = tick.wrapping_add(1);
        delay.delay_millis(READING_INTERVAL_SECS * 1000);
    }
}
