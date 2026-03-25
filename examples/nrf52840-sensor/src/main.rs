//! # Zigbee-RS nRF52840 Weather Sensor
//!
//! Embassy-based firmware for the Nordic nRF52840 that exposes simulated
//! temperature, humidity, and pressure via Zigbee ZCL clusters:
//!
//! - Temperature Measurement (0x0402)
//! - Relative Humidity Measurement (0x0405)
//! - Pressure Measurement (0x0403)
//!
//! ## Hardware
//! - nRF52840-DK or any nRF52840 board
//!
//! ## Notes
//! Sensor values are simulated. To use a real sensor (e.g. BME280 over I2C),
//! add `bme280 = "0.5"` and replace the simulated readings with actual I2C
//! reads using `embassy_nrf::twim::Twim`.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_nrf::{self as _, bind_interrupts, peripherals, radio};
use embassy_time::{Duration, Timer};

use defmt::*;
use {defmt_rtt as _, panic_probe as _};

use zigbee_aps::PROFILE_HOME_AUTOMATION;
use zigbee_nwk::DeviceType;
use zigbee_runtime::event_loop::{self, StackEvent, TickResult};
use zigbee_runtime::ZigbeeDevice;
use zigbee_zcl::clusters::basic::BasicCluster;
use zigbee_zcl::clusters::humidity::HumidityCluster;
use zigbee_zcl::clusters::pressure::PressureCluster;
use zigbee_zcl::clusters::temperature::TemperatureCluster;

const CLUSTER_BASIC: u16 = 0x0000;
const CLUSTER_TEMPERATURE: u16 = 0x0402;
const CLUSTER_HUMIDITY: u16 = 0x0405;
const CLUSTER_PRESSURE: u16 = 0x0403;

const DEVICE_TYPE_TEMP_SENSOR: u16 = 0x0302;
const MEASURE_INTERVAL_SECS: u64 = 30;

bind_interrupts!(struct Irqs {
    RADIO => radio::InterruptHandler<peripherals::RADIO>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());

    info!("Zigbee-RS nRF52840 weather sensor starting…");

    // IEEE 802.15.4 MAC driver (nRF52840 on-chip radio)
    let radio = radio::ieee802154::Radio::new(p.RADIO, Irqs);
    let mac = zigbee_mac::nrf::NrfMac::new(radio);

    info!("IEEE 802.15.4 radio ready");

    // ZCL cluster instances
    let mut temp_cluster = TemperatureCluster::new(-4000, 8500);
    let mut humidity_cluster = HumidityCluster::new(0, 10000);
    let mut pressure_cluster = PressureCluster::new(300, 1100);

    let _basic = BasicCluster::new(
        b"Zigbee-RS",
        b"nRF52840 Weather",
        b"20250101",
        b"0.1.0",
    );

    // Build the Zigbee device
    let mut device = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::EndDevice)
        .manufacturer("Zigbee-RS")
        .model("nRF52840 Weather")
        .sw_build("0.1.0")
        .channels(zigbee_types::ChannelMask::ALL_2_4GHZ)
        .endpoint(1, PROFILE_HOME_AUTOMATION, DEVICE_TYPE_TEMP_SENSOR, |ep| {
            ep.cluster_server(CLUSTER_BASIC)
                .cluster_server(CLUSTER_TEMPERATURE)
                .cluster_server(CLUSTER_HUMIDITY)
                .cluster_server(CLUSTER_PRESSURE)
        })
        .build();

    info!("Zigbee device configured — joining network…");

    let mut joined = false;
    let mut tick: u32 = 0;

    loop {
        match event_loop::stack_tick(&mut device).await {
            TickResult::Event(StackEvent::Joined { short_address, channel, pan_id }) => {
                info!(
                    "Joined! addr=0x{:04X} ch={} pan=0x{:04X}",
                    short_address, channel, pan_id,
                );
                joined = true;
            }
            TickResult::Event(StackEvent::Left) => {
                info!("Left network — will rejoin");
                joined = false;
            }
            TickResult::Event(StackEvent::CommissioningComplete { success }) => {
                info!("Commissioning complete: {}", success);
            }
            TickResult::Event(_event) => {
                info!("Stack event received");
            }
            TickResult::RunAgain(delay_ms) => {
                Timer::after(Duration::from_millis(delay_ms as u64)).await;
                continue;
            }
            TickResult::Idle => {}
        }

        // Simulated sensor readings
        if joined {
            let temp_100: i16 = 2250 + ((tick % 50) as i16 - 25);
            let hum_100: u16 = 5500 + ((tick % 40) as u16).wrapping_sub(20);
            let press_10: i16 = 10130 + ((tick % 20) as i16 - 10);

            temp_cluster.set_temperature(temp_100);
            humidity_cluster.set_humidity(hum_100);
            pressure_cluster.set_pressure(press_10);

            info!(
                "T={}.{}°C  H={}.{}%  P={}.{}kPa",
                temp_100 / 100,
                (temp_100 % 100).unsigned_abs(),
                hum_100 / 100,
                hum_100 % 100,
                press_10 / 10,
                (press_10 % 10).unsigned_abs()
            );
        }

        tick = tick.wrapping_add(1);
        Timer::after(Duration::from_secs(MEASURE_INTERVAL_SECS)).await;
    }
}
