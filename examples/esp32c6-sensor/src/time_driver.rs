//! Embassy time driver for ESP32-C6 using esp_hal::time::Instant.
//!
//! Wraps esp-hal's monotonic clock as the embassy time source.
//! Uses polling (not interrupt-driven) — the CPU runs continuously.
//!
//! For battery operation, an interrupt-driven SYSTIMER alarm driver
//! would be needed to enable WFI sleep. This requires proper integration
//! with esp-hal's RISC-V PLIC interrupt dispatcher.

use embassy_time_driver::Driver;
use portable_atomic::{AtomicU64, Ordering};

struct EspTimeDriver {
    alarm_at: AtomicU64,
}

impl Driver for EspTimeDriver {
    fn now(&self) -> u64 {
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros()
    }

    fn schedule_wake(&self, at: u64, _waker: &core::task::Waker) {
        self.alarm_at.store(at, Ordering::Release);
    }
}

embassy_time_driver::time_driver_impl!(static DRIVER: EspTimeDriver = EspTimeDriver {
    alarm_at: AtomicU64::new(u64::MAX),
});

pub fn init() {}
