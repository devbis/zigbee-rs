//! Board support for the Tuya TS011F Zigbee smart plug.
//!
//! Pin assignments from `board_ts011f.h`:
//! - Relay: PD6
//! - Button: PD3
//! - LED green: PD5, LED blue: PD4, LED red: PD7
//! - BL0937 CF: PB5, CF1: PB6, SEL: PB7

#![no_std]

pub mod leds;
pub mod storage;

use tlsr8258_hal::gpio::{self, Pin, Port};
use zigbee_bl0937::SelPin;

/// BL0937 CF pin (PB5) — pulse counter for active power.
pub struct CfPin(Pin);

impl CfPin {
    pub fn new() -> Self {
        Self(Pin::new(Port::B, 5))
    }

    #[cfg(target_arch = "tc32")]
    pub fn configure_input(&self) {
        gpio::set_function_gpio(self.0);
        gpio::set_input_enable(self.0, true).ok();
        gpio::set_output_enable(self.0, false);
    }
}

impl Default for CfPin {
    fn default() -> Self {
        Self::new()
    }
}

/// BL0937 CF1 pin (PB6) — pulse counter for voltage/current.
pub struct Cf1Pin(Pin);

impl Cf1Pin {
    pub fn new() -> Self {
        Self(Pin::new(Port::B, 6))
    }

    #[cfg(target_arch = "tc32")]
    pub fn configure_input(&self) {
        gpio::set_function_gpio(self.0);
        gpio::set_input_enable(self.0, true).ok();
        gpio::set_output_enable(self.0, false);
    }
}

impl Default for Cf1Pin {
    fn default() -> Self {
        Self::new()
    }
}

/// BL0937 SEL pin (PB7) — controls voltage/current mode on CF1.
pub struct SelOutputPin(Pin);

impl SelOutputPin {
    pub fn new() -> Self {
        Self(Pin::new(Port::B, 7))
    }

    #[cfg(target_arch = "tc32")]
    pub fn configure_output(&self) {
        gpio::set_function_gpio(self.0);
        gpio::write(self.0, false);
        gpio::set_output_enable(self.0, true);
        gpio::set_input_enable(self.0, false).ok();
    }
}

impl Default for SelOutputPin {
    fn default() -> Self {
        Self::new()
    }
}

impl SelPin for SelOutputPin {
    fn set_high(&mut self) {
        #[cfg(target_arch = "tc32")]
        gpio::write(self.0, true);
    }

    fn set_low(&mut self) {
        #[cfg(target_arch = "tc32")]
        gpio::write(self.0, false);
    }
}

/// Relay output pin (PD6).
pub struct RelayPin(Pin);

impl RelayPin {
    pub fn new() -> Self {
        Self(Pin::new(Port::D, 6))
    }

    #[cfg(target_arch = "tc32")]
    pub fn configure_output(&self) {
        gpio::set_function_gpio(self.0);
        gpio::write(self.0, false);
        gpio::set_output_enable(self.0, true);
        gpio::set_input_enable(self.0, false).ok();
    }

    pub fn set_on(&self, on: bool) {
        #[cfg(target_arch = "tc32")]
        gpio::write(self.0, on);
    }
}

impl Default for RelayPin {
    fn default() -> Self {
        Self::new()
    }
}

/// Button input pin (PD3).
pub struct ButtonPin(Pin);

impl ButtonPin {
    pub fn new() -> Self {
        Self(Pin::new(Port::D, 3))
    }

    #[cfg(target_arch = "tc32")]
    pub fn configure_input(&self) {
        gpio::set_function_gpio(self.0);
        gpio::set_input_enable(self.0, true).ok();
        gpio::set_output_enable(self.0, false);
    }

    pub fn is_pressed(&self) -> bool {
        #[cfg(target_arch = "tc32")]
        {
            gpio::read(self.0)
        }
        #[cfg(not(target_arch = "tc32"))]
        {
            false
        }
    }
}

impl Default for ButtonPin {
    fn default() -> Self {
        Self::new()
    }
}

/// Configure all TS011F board pins.
pub fn configure() {
    #[cfg(target_arch = "tc32")]
    {
        CfPin::new().configure_input();
        Cf1Pin::new().configure_input();
        SelOutputPin::new().configure_output();
        RelayPin::new().configure_output();
        ButtonPin::new().configure_input();
        leds::configure_status_leds().ok();
    }
}
