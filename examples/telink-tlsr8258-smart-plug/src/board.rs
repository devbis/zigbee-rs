//! TS011F board initialization and hardware abstraction.

use tlsr8258_hal::gpio::{self, Pin, Port};
use zigbee_bl0937::{PulseCounter, SelPin};

pub struct CfCounter;

impl PulseCounter for CfCounter {
    fn read_and_reset(&mut self) -> (u32, u32) {
        // TODO: implement with GPIO IRQ edge counting
        // For now, return 0 — actual implementation needs IRQ handler
        (0, 0)
    }
}

pub struct Cf1Counter;

impl PulseCounter for Cf1Counter {
    fn read_and_reset(&mut self) -> (u32, u32) {
        // TODO: implement with GPIO IRQ edge counting
        (0, 0)
    }
}

pub struct SelPinOutput;

impl SelPin for SelPinOutput {
    fn set_high(&mut self) {
        #[cfg(target_arch = "tc32")]
        gpio::write(Pin::new(Port::B, 7), true);
    }

    fn set_low(&mut self) {
        #[cfg(target_arch = "tc32")]
        gpio::write(Pin::new(Port::B, 7), false);
    }
}

pub fn configure() {
    #[cfg(target_arch = "tc32")]
    {
        tlsr8258_ts011f::configure();
    }
}
