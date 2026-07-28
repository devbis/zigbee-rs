//! TLSR8258 smart plug with BL0937 energy monitoring entry point.

#![no_std]
#![no_main]

mod app;
mod board;
mod energy;

use tlsr8258_rt as _;

#[panic_handler]
fn panic_handler(_info: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("nop");
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn irq_handler() {
    // TODO: dispatch GPIO IRQ for BL0937 pulse counting
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _rust_entry() -> ! {
    tlsr8258_hal::clocks::init();
    tlsr8258_hal::timer::init();
    board::configure();
    app::run();
}
