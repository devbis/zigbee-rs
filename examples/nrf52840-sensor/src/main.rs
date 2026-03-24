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
//! ## Alternative sensors
//!
//! The BME280 can be swapped for other I2C sensors with minimal changes:
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
use embassy_nrf::{self as _, bind_interrupts, peripherals, twim};
use embassy_time::{Duration, Timer};

use defmt::*;
use {defmt_rtt as _, panic_probe as _};

// ---------------------------------------------------------------------------
// Interrupt bindings — routes the TWIM0 interrupt to the Embassy driver.
// ---------------------------------------------------------------------------
bind_interrupts!(struct Irqs {
    SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0 => twim::InterruptHandler<peripherals::TWISPI0>;
});

/// Measurement interval in seconds.
const MEASURE_INTERVAL_SECS: u64 = 30;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // -----------------------------------------------------------------------
    // 1. Peripheral initialisation
    // -----------------------------------------------------------------------
    let p = embassy_nrf::init(Default::default());

    // -----------------------------------------------------------------------
    // 2. I2C bus for the BME280 sensor
    //    Wiring: SDA → P0.26, SCL → P0.27, VCC → 3.3 V, GND → GND
    //
    //    For SHT31 / SHT40 / SHTC3, use the same pins but swap the driver
    //    crate and I2C address (see module-level docs).
    // -----------------------------------------------------------------------
    let i2c_config = twim::Config::default(); // 100 kHz — raise to 400 kHz if needed
    let i2c = twim::Twim::new(p.TWISPI0, Irqs, p.P0_26, p.P0_27, i2c_config);

    let mut bme = bme280::Bme280::new(i2c, bme280::Address::Primary); // 0x76
    bme.init().unwrap();

    // -----------------------------------------------------------------------
    // 3. ZCL cluster instances
    //
    //    Temperature: range -40.00 … +85.00 °C  (encoded as hundredths)
    //    Humidity:    range   0.00 … 100.00 %RH  (encoded as hundredths)
    //    Pressure:    range  300.0 … 1100.0 hPa   (encoded as tenths of kPa)
    // -----------------------------------------------------------------------
    let mut temp_cluster =
        zigbee_zcl::clusters::temperature::TemperatureCluster::new(-4000, 8500);
    let mut humidity_cluster =
        zigbee_zcl::clusters::humidity::HumidityCluster::new(0, 10000);
    let mut pressure_cluster =
        zigbee_zcl::clusters::pressure::PressureCluster::new(300, 1100);

    let _basic = zigbee_zcl::clusters::basic::BasicCluster::new(
        "Zigbee-RS",         // manufacturer name
        "nRF52840 Weather",  // model identifier
        1,                   // hardware version
    );

    info!("Zigbee-RS nRF52840 sensor starting...");

    // -----------------------------------------------------------------------
    // 4. Main loop — measure, update clusters, sleep
    // -----------------------------------------------------------------------
    loop {
        // Read sensor data.  If the bus or sensor is temporarily unavailable
        // we log the error and retry on the next cycle.
        match bme.measure() {
            Ok(measurements) => {
                // ZCL Temperature Measurement uses i16 in units of 0.01 °C.
                let temp_100 = (measurements.temperature * 100.0) as i16;
                // ZCL Relative Humidity uses u16 in units of 0.01 %RH.
                let hum_100 = (measurements.humidity * 100.0) as u16;
                // ZCL Pressure Measurement uses i16 in units of 0.1 kPa
                // (i.e. 1 hPa precision).
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
                warn!("BME280 read failed — will retry in {} s", MEASURE_INTERVAL_SECS);
            }
        }

        // TODO: call into the Zigbee stack tick / event processing here once
        // zigbee-runtime provides an async `poll()` or `process_events()`.

        Timer::after(Duration::from_secs(MEASURE_INTERVAL_SECS)).await;
    }
}
