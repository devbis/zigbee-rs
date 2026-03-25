//! # Zigbee-RS nRF52840 Weather Sensor
//!
//! Embassy-based firmware for the Nordic nRF52840 that reads temperature,
//! humidity, and pressure from a BME280 sensor over I2C and exposes the
//! values through Zigbee ZCL clusters:
//!
//! - Temperature Measurement (0x0402)
//! - Relative Humidity Measurement (0x0405)
//! - Pressure Measurement (0x0403)
//!
//! ## Hardware
//! - nRF52840-DK or any nRF52840 board (~$10–$40)
//! - BME280 I2C sensor: SDA → P0.26, SCL → P0.27
//!
//! ## Alternative sensors
//!
//! | Sensor | Crate   | I2C addr | Notes                        |
//! |--------|---------|----------|------------------------------|
//! | SHT31  | `sht3x` | 0x44     | Temp + humidity only         |
//! | SHT40  | `sht4x` | 0x44     | Higher accuracy successor    |
//! | SHTC3  | `shtcx` | 0x70     | Ultra-low-power, fast wakeup |
//! | BMP280 | `bme280`| 0x76     | Temp + pressure (no humidity)|
//!
//! For sensors without pressure, simply omit the pressure cluster.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_nrf::{self as _, bind_interrupts, peripherals, radio, twim};
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

// ---------------------------------------------------------------------------
// ZCL cluster IDs used when registering endpoints
// ---------------------------------------------------------------------------
const CLUSTER_BASIC: u16 = 0x0000;
const CLUSTER_TEMPERATURE: u16 = 0x0402;
const CLUSTER_HUMIDITY: u16 = 0x0405;
const CLUSTER_PRESSURE: u16 = 0x0403;

// HA device type: Temperature Sensor (0x0302)
const DEVICE_TYPE_TEMP_SENSOR: u16 = 0x0302;

/// Measurement interval in seconds.
const MEASURE_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Interrupt bindings
// ---------------------------------------------------------------------------
bind_interrupts!(struct Irqs {
    SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0 => twim::InterruptHandler<peripherals::TWISPI0>;
    RADIO => radio::InterruptHandler<peripherals::RADIO>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // -----------------------------------------------------------------------
    // 1. Peripheral initialisation
    // -----------------------------------------------------------------------
    let p = embassy_nrf::init(Default::default());

    info!("Zigbee-RS nRF52840 weather sensor starting…");

    // -----------------------------------------------------------------------
    // 2. I2C bus for the BME280 sensor
    //    Wiring: SDA → P0.26, SCL → P0.27, VCC → 3.3 V, GND → GND
    // -----------------------------------------------------------------------
    let i2c_config = twim::Config::default();
    let i2c = twim::Twim::new(p.TWISPI0, Irqs, p.P0_26, p.P0_27, i2c_config);

    let mut bme = bme280::Bme280::new(i2c, bme280::Address::Primary); // 0x76
    bme.init().unwrap();

    // -----------------------------------------------------------------------
    // 3. IEEE 802.15.4 MAC driver (nRF52840 on-chip radio)
    // -----------------------------------------------------------------------
    let radio = radio::ieee802154::Radio::new(p.RADIO, Irqs);
    let mac = zigbee_mac::nrf::NrfMac::new(radio);

    info!("IEEE 802.15.4 radio ready");

    // -----------------------------------------------------------------------
    // 4. ZCL cluster instances
    // -----------------------------------------------------------------------
    let mut temp_cluster = TemperatureCluster::new(-4000, 8500);
    let mut humidity_cluster = HumidityCluster::new(0, 10000);
    let mut pressure_cluster = PressureCluster::new(300, 1100);

    let _basic = BasicCluster::new(
        b"Zigbee-RS",           // manufacturer name
        b"nRF52840 Weather",    // model identifier
        b"20250101",            // date code
        b"0.1.0",               // software build
    );

    // -----------------------------------------------------------------------
    // 5. Build the Zigbee device using the runtime builder
    //
    // Endpoint 1 — HA Temperature Sensor with server-side clusters:
    //   • Basic          (0x0000) — mandatory on every endpoint
    //   • Temperature    (0x0402) — measured value in 0.01 °C
    //   • Rel. Humidity  (0x0405) — measured value in 0.01 %
    //   • Pressure       (0x0403) — measured value in 0.1 kPa
    // -----------------------------------------------------------------------
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

    // -----------------------------------------------------------------------
    // 6. Main loop — sensor reading + Zigbee stack processing
    // -----------------------------------------------------------------------
    let mut joined = false;

    loop {
        // --- 6a. Tick the Zigbee stack (processes radio events, NWK, APS) --
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

        // --- 6b. Read the BME280 and update ZCL attributes ----------------
        if joined {
            match bme.measure() {
                Ok(measurements) => {
                    let temp_100 = (measurements.temperature * 100.0) as i16;
                    let hum_100 = (measurements.humidity * 100.0) as u16;
                    let press_10 = (measurements.pressure * 10.0) as i16;

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
                Err(_e) => {
                    warn!("BME280 read failed — will retry");
                }
            }
        }

        // --- 6c. Sleep until next reading ----------------------------------
        Timer::after(Duration::from_secs(MEASURE_INTERVAL_SECS)).await;
    }
}
