//! Nordic environmental-sensor adapters for the shared SED lifecycle.

use core::convert::Infallible;

use sensor_sed_app::{EnvironmentReading, EnvironmentSource};

/// ZCL Relative Humidity Measurement `MeasuredValue` meaning "the
/// measurement is invalid" (ZCL r8 §4.7.2.2.1.1).
pub const HUMIDITY_UNKNOWN: u16 = 0xFFFF;

/// The nRF52 on-chip `TEMP` peripheral.
///
/// The die has no humidity sensor, so the humidity value is reported as
/// [`HUMIDITY_UNKNOWN`] rather than an invented number a coordinator would
/// store as a real measurement. The synthetic 50–59.9 % ramp of the former
/// no-external-sensor firmware is available only through the explicit
/// `demo-humidity` feature for radio/reporting bring-up.
pub struct OnChipTemperature<'d> {
    temp: embassy_nrf::temp::Temp<'d>,
    #[cfg(feature = "demo-humidity")]
    humidity_tick: u32,
}

impl<'d> OnChipTemperature<'d> {
    pub const fn new(temp: embassy_nrf::temp::Temp<'d>) -> Self {
        Self {
            temp,
            #[cfg(feature = "demo-humidity")]
            humidity_tick: 0,
        }
    }

    #[cfg(feature = "demo-humidity")]
    fn humidity_centi_percent(&mut self) -> u16 {
        self.humidity_tick = self.humidity_tick.wrapping_add(1);
        5000u16 + ((self.humidity_tick % 100) as u16).wrapping_mul(10)
    }

    #[cfg(not(feature = "demo-humidity"))]
    fn humidity_centi_percent(&mut self) -> u16 {
        HUMIDITY_UNKNOWN
    }
}

impl EnvironmentSource for OnChipTemperature<'_> {
    type Error = Infallible;

    async fn sample(&mut self) -> Result<EnvironmentReading, Self::Error> {
        let raw_temp = self.temp.read().await;
        let temperature_centi_celsius = (raw_temp.to_bits() * 100 / 4) as i16;
        Ok(EnvironmentReading {
            temperature_centi_celsius,
            humidity_centi_percent: self.humidity_centi_percent(),
            pressure_tenth_kpa: None,
        })
    }
}
