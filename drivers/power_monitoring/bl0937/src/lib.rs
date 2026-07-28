//! Platform-agnostic BL0937 single-phase energy metering IC driver.
//!
//! The BL0937 outputs pulse frequencies proportional to:
//! - **CF pin**: active power (energy metering)
//! - **CF1 pin**: current RMS (SEL=0) or voltage RMS (SEL=1)
//!
//! The MCU measures pulse frequencies to derive power, voltage, and current.
//! SEL must be toggled to alternate between voltage and current measurement
//! on CF1.
//!
//! # Example
//!
//! ```ignore
//! let mut bl0937 = Bl0937::new(cf_counter, cf1_counter, sel_pin, CALIBRATION);
//! loop {
//!     let reading = bl0937.read();
//!     // reading.voltage_rms in centi-volts (230.00V = 23000)
//!     // reading.current_rms in centi-amps (1.50A = 150)
//!     // reading.active_power in watts
//! }
//! ```

#![no_std]

/// Counts pulses on a GPIO pin. Platform implements this.
///
/// The counter should be reset to zero on each `read_and_reset` call.
/// Elapsed time is the duration since the last reset, in microseconds.
pub trait PulseCounter {
    /// Return (pulse_count, elapsed_us) since the last reset.
    fn read_and_reset(&mut self) -> (u32, u32);
}

/// Output pin to control the BL0937 SEL line.
///
/// SEL=0 → CF1 outputs current RMS pulses
/// SEL=1 → CF1 outputs voltage RMS pulses
pub trait SelPin {
    fn set_high(&mut self);
    fn set_low(&mut self);
}

/// BL0937 reference voltage (Vref = 1.218V typical).
pub const VREF: f32 = 1.218;

/// Pulse width of CF/CF1 output (38 µs typical).
pub const PULSE_WIDTH_US: u32 = 38;

/// Round an `f32` to the nearest integer (no_std compatible).
#[inline]
fn round_f32(x: f32) -> f32 {
    if x >= 0.0 {
        (x + 0.5) as i32 as f32
    } else {
        (x - 0.5) as i32 as f32
    }
}

/// Round an `f64` to the nearest integer (no_std compatible).
#[inline]
fn round_f64(x: f64) -> f64 {
    if x >= 0.0 {
        (x + 0.5) as i64 as f64
    } else {
        (x - 0.5) as i64 as f64
    }
}

/// Datasheet frequency formulas:
///
/// - F_CF  = 1721506 * V(V) * V(I) / Vref²
/// - F_CFU = 15397   * V(V) / Vref
/// - F_CFI = 94638   * V(I) / Vref
///
/// Where V(V) is the voltage-channel input and V(I) is the current-channel
/// input (both in volts, not the measured mains values).
///
/// Calibration factors convert measured pulse frequencies to real-world
/// values by accounting for the external resistor divider network and
/// shunt resistor tolerances.

/// Calibration factors for converting pulse frequencies to physical values.
///
/// Obtain these via single-point calibration: apply a known load
/// (voltage U0, current I0, power P0), measure the CF/CF1 pulse
/// frequencies, and compute:
///
/// ```text
/// voltage_factor = U0 / F_CFU_measured
/// current_factor = I0 / F_CFI_measured
/// power_factor   = P0 / F_CF_measured
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    /// Multiply CF pulse frequency (Hz) by this to get watts.
    pub power_factor: f32,
    /// Multiply CF1 frequency (Hz, SEL=1) by this to get volts.
    pub voltage_factor: f32,
    /// Multiply CF1 frequency (Hz, SEL=0) by this to get amps.
    pub current_factor: f32,
}

impl Calibration {
    /// Theoretical calibration from datasheet constants (no external
    /// component tolerance compensation). Values are only correct if
    /// the resistor network exactly matches the reference design.
    pub const fn theoretical() -> Self {
        // From V(V)*V(I)/Vref² * 1721506 = F_CF
        // → P = F_CF * Vref² / (V(V)*V(I) * 1721506)
        // But the "factor" is simpler: P(watts) = freq * power_factor
        // where power_factor is determined by calibration.
        //
        // Theoretical values are not useful without knowing the actual
        // resistor divider ratios. Use calibration() instead.
        Self {
            power_factor: 1.0,
            voltage_factor: 1.0,
            current_factor: 1.0,
        }
    }
}

/// One measurement reading from the BL0937.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// RMS voltage in centi-volts (e.g. 23000 = 230.00 V).
    pub voltage_rms: u16,
    /// RMS current in centi-amps (e.g. 150 = 1.50 A).
    pub current_rms: u16,
    /// Active power in watts (signed for bidirectional metering).
    pub active_power: i16,
    /// Raw CF pulse count since last read (for energy accumulation).
    pub energy_pulses: u32,
}

/// BL0937 energy metering driver.
///
/// Generic over:
/// - `CF`: pulse counter for the CF (active power) pin
/// - `CF1`: pulse counter for the CF1 (voltage/current RMS) pin
/// - `SEL`: output pin for the SEL (mode select) line
pub struct Bl0937<CF, CF1, SEL> {
    cf: CF,
    cf1: CF1,
    sel: SEL,
    calibration: Calibration,
    /// false = CF1 outputs current (SEL low), true = CF1 outputs voltage (SEL high)
    sel_state: bool,
    /// Toggle counter: switch SEL every `toggle_interval` reads.
    toggle_counter: u32,
    /// How many `read()` calls between SEL toggles. 0 = never toggle.
    toggle_interval: u32,
    /// Last stable voltage reading (retained between SEL switches).
    last_voltage: u16,
    /// Last stable current reading (retained between SEL switches).
    last_current: u16,
    /// Last power reading.
    last_power: i16,
    /// Accumulated energy pulses (CF).
    total_energy_pulses: u64,
}

impl<CF, CF1, SEL> Bl0937<CF, CF1, SEL>
where
    CF: PulseCounter,
    CF1: PulseCounter,
    SEL: SelPin,
{
    /// Create a new BL0937 driver.
    ///
    /// `toggle_interval` controls how many `read()` calls between SEL
    /// toggles. A typical value is 5 (switching every 5 seconds at 1 Hz
    /// read rate). Pass 0 to disable SEL toggling (CF1 will always
    /// measure whatever mode SEL was last set to).
    pub fn new(cf: CF, cf1: CF1, sel: SEL, calibration: Calibration, toggle_interval: u32) -> Self {
        Self {
            cf,
            cf1,
            sel,
            calibration,
            sel_state: false,
            toggle_counter: 0,
            toggle_interval,
            last_voltage: 0,
            last_current: 0,
            last_power: 0,
            total_energy_pulses: 0,
        }
    }

    /// Perform a measurement cycle. Call this periodically (e.g. once per
    /// second). Returns a reading with the latest available values.
    ///
    /// Voltage and current may come from alternating SEL states, so each
    /// `read()` returns the most recent value for each parameter
    /// independently.
    pub fn read(&mut self) -> Reading {
        // Read CF (active power + energy pulses)
        let (cf_pulses, cf_us) = self.cf.read_and_reset();
        let cf_freq = pulse_frequency_hz(cf_pulses, cf_us);
        self.last_power = round_f32(cf_freq * self.calibration.power_factor) as i16;
        self.total_energy_pulses += cf_pulses as u64;

        // Toggle SEL if interval elapsed
        let cf1_pulses;
        let cf1_us;
        if self.toggle_interval > 0 {
            self.toggle_counter += 1;
            if self.toggle_counter >= self.toggle_interval {
                self.toggle_counter = 0;
                // Toggle SEL and read CF1 before toggling back
                self.toggle_sel();
                // Small settle time: read CF1 in the new mode
                (cf1_pulses, cf1_us) = self.cf1.read_and_reset();
            } else {
                // Keep reading CF1 in current mode (non-blocking check)
                (cf1_pulses, cf1_us) = self.cf1.read_and_reset();
            }
        } else {
            (cf1_pulses, cf1_us) = self.cf1.read_and_reset();
        }

        // Convert CF1 frequency based on current SEL state
        let cf1_freq = pulse_frequency_hz(cf1_pulses, cf1_us);
        if self.sel_state {
            // SEL=1 → voltage
            self.last_voltage = round_f32(cf1_freq * self.calibration.voltage_factor) as u16;
        } else {
            // SEL=0 → current
            self.last_current = centi_amps_from_hz(cf1_freq, self.calibration.current_factor);
        }

        Reading {
            voltage_rms: self.last_voltage,
            current_rms: self.last_current,
            active_power: self.last_power,
            energy_pulses: cf_pulses,
        }
    }

    /// Toggle the SEL pin to switch between voltage and current measurement.
    fn toggle_sel(&mut self) {
        self.sel_state = !self.sel_state;
        if self.sel_state {
            self.sel.set_high();
        } else {
            self.sel.set_low();
        }
    }

    /// Set SEL to a specific state and wait for the BL0937 to switch.
    pub fn set_sel(&mut self, voltage_mode: bool) {
        self.sel_state = voltage_mode;
        if voltage_mode {
            self.sel.set_high();
        } else {
            self.sel.set_low();
        }
    }

    /// Total accumulated energy pulses (CF) since driver creation.
    pub fn total_energy_pulses(&self) -> u64 {
        self.total_energy_pulses
    }

    /// Reset the total energy pulse counter.
    pub fn reset_energy_pulses(&mut self) {
        self.total_energy_pulses = 0;
    }

    /// Release the underlying peripherals.
    pub fn release(self) -> (CF, CF1, SEL) {
        (self.cf, self.cf1, self.sel)
    }
}

/// Convert a pulse count and elapsed time to frequency in Hz.
///
/// Returns 0 if elapsed_us is 0 (no measurement window).
#[inline]
pub fn pulse_frequency_hz(pulses: u32, elapsed_us: u32) -> f32 {
    if elapsed_us == 0 || pulses == 0 {
        return 0.0;
    }
    pulses as f32 * 1_000_000.0 / elapsed_us as f32
}

/// Convert CF1 frequency to centi-amps using the current calibration factor.
#[inline]
pub fn centi_amps_from_hz(freq: f32, current_factor: f32) -> u16 {
    let amps = freq * current_factor;
    (round_f32(amps * 100.0)) as u16
}

/// Convert CF1 frequency to centi-volts using the voltage calibration factor.
#[inline]
pub fn centi_volts_from_hz(freq: f32, voltage_factor: f32) -> u16 {
    let volts = freq * voltage_factor;
    (round_f32(volts * 100.0)) as u16
}

/// Convert raw CF pulse count to energy in watt-hours.
///
/// Requires knowing the power calibration factor and the measurement
/// window duration. For continuous accumulation, prefer counting
/// pulses and converting periodically.
pub fn pulses_to_wh(pulses: u64, power_factor: f32, window_us: u64) -> u64 {
    if window_us == 0 || pulses == 0 {
        return 0;
    }
    let watts = pulses as f64 * power_factor as f64 * 1_000_000.0 / window_us as f64;
    // watt-hours = watts * seconds / 3600
    (round_f64(watts * window_us as f64 / 3_600_000_000.0)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_frequency_basic() {
        // 1000 pulses in 1,000,000 µs = 1000 Hz
        assert_eq!(pulse_frequency_hz(1000, 1_000_000), 1000.0);
    }

    #[test]
    fn pulse_frequency_zero_pulses() {
        assert_eq!(pulse_frequency_hz(0, 1_000_000), 0.0);
    }

    #[test]
    fn pulse_frequency_zero_time() {
        assert_eq!(pulse_frequency_hz(1000, 0), 0.0);
    }

    #[test]
    fn centi_amps_conversion() {
        // 1.50 A → 150 centi-amps
        let hz = 100.0; // some frequency
        let factor = 0.015; // calibration factor
        let result = centi_amps_from_hz(hz, factor);
        assert_eq!(result, 150);
    }

    #[test]
    fn centi_volts_conversion() {
        // 230.00 V → 23000 centi-volts
        let hz = 100.0;
        let factor = 2.3;
        let result = centi_volts_from_hz(hz, factor);
        assert_eq!(result, 23000);
    }

    #[test]
    fn pulses_to_wh_basic() {
        // 3600 pulses at 1 W for 1 hour = 1 Wh
        // power_factor = 1.0, window = 3,600,000,000 µs (1 hour)
        let result = pulses_to_wh(3600, 1.0, 3_600_000_000);
        assert_eq!(result, 1);
    }

    #[test]
    fn pulses_to_wh_zero() {
        assert_eq!(pulses_to_wh(0, 1.0, 3_600_000_000), 0);
        assert_eq!(pulses_to_wh(100, 1.0, 0), 0);
    }

    #[test]
    fn calibration_theoretical_identity() {
        let cal = Calibration::theoretical();
        assert_eq!(cal.power_factor, 1.0);
        assert_eq!(cal.voltage_factor, 1.0);
        assert_eq!(cal.current_factor, 1.0);
    }

    /// Mock pulse counter for testing.
    struct MockCounter {
        pulses: u32,
        elapsed_us: u32,
    }

    impl MockCounter {
        fn new(pulses: u32, elapsed_us: u32) -> Self {
            Self { pulses, elapsed_us }
        }
    }

    impl PulseCounter for MockCounter {
        fn read_and_reset(&mut self) -> (u32, u32) {
            let result = (self.pulses, self.elapsed_us);
            self.pulses = 0;
            self.elapsed_us = 0;
            result
        }
    }

    /// Mock SEL pin that records state changes.
    struct MockSelPin {
        state: bool,
    }

    impl MockSelPin {
        fn new() -> Self {
            Self { state: false }
        }
    }

    impl SelPin for MockSelPin {
        fn set_high(&mut self) {
            self.state = true;
        }
        fn set_low(&mut self) {
            self.state = false;
        }
    }

    #[test]
    fn driver_read_basic() {
        let cf = MockCounter::new(500, 1_000_000); // 500 Hz
        let cf1 = MockCounter::new(100, 1_000_000); // 100 Hz
        let sel = MockSelPin::new();

        let cal = Calibration {
            power_factor: 1.0,
            voltage_factor: 2.3,
            current_factor: 0.015,
        };

        let mut driver = Bl0937::new(cf, cf1, sel, cal, 0); // no toggling
        let reading = driver.read();

        assert_eq!(reading.active_power, 500);
        // SEL starts false (current mode), so CF1 = current
        assert_eq!(reading.current_rms, centi_amps_from_hz(100.0, 0.015));
        assert_eq!(reading.voltage_rms, 0); // never measured voltage
    }

    #[test]
    fn driver_sel_toggling() {
        let cf = MockCounter::new(500, 1_000_000);
        let cf1 = MockCounter::new(100, 1_000_000);
        let sel = MockSelPin::new();

        let cal = Calibration {
            power_factor: 1.0,
            voltage_factor: 2.3,
            current_factor: 0.015,
        };

        let mut driver = Bl0937::new(cf, cf1, sel, cal, 2); // toggle every 2 reads

        // First read: SEL=0 (current), CF1=100Hz → current
        let r1 = driver.read();
        assert_eq!(r1.current_rms, centi_amps_from_hz(100.0, 0.015));
        assert_eq!(r1.voltage_rms, 0);

        // Second read: toggle_counter reaches 2 → toggle to SEL=1
        // But CF1 mock returns 0 on second read (mock was reset)
        let r2 = driver.read();
        // SEL state is now true (voltage mode), but CF1 returns 0
        assert_eq!(r2.voltage_rms, 0);

        // Verify SEL was toggled by releasing and checking
        let (_cf, _cf1, sel_out) = driver.release();
        assert!(sel_out.state);
    }

    #[test]
    fn driver_energy_accumulation() {
        let cf = MockCounter::new(100, 1_000_000);
        let cf1 = MockCounter::new(0, 1_000_000);
        let sel = MockSelPin::new();
        let cal = Calibration::theoretical();

        let mut driver = Bl0937::new(cf, cf1, sel, cal, 0);

        let r1 = driver.read();
        assert_eq!(r1.energy_pulses, 100);
        assert_eq!(driver.total_energy_pulses(), 100);

        // Second read: mock returns 0 after read_and_reset
        let r2 = driver.read();
        assert_eq!(r2.energy_pulses, 0);
        assert_eq!(driver.total_energy_pulses(), 100);

        driver.reset_energy_pulses();
        assert_eq!(driver.total_energy_pulses(), 0);
    }

    #[test]
    fn driver_release() {
        let cf = MockCounter::new(0, 0);
        let cf1 = MockCounter::new(0, 0);
        let sel = MockSelPin::new();
        let cal = Calibration::theoretical();

        let driver = Bl0937::new(cf, cf1, sel, cal, 0);
        let (cf_out, cf1_out, sel_out) = driver.release();
        assert_eq!(cf_out.pulses, 0);
        assert_eq!(cf1_out.pulses, 0);
        assert!(!sel_out.state);
    }
}
