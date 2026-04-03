//! Embassy time driver for ESP32-C6 using esp_hal::time::Instant.
//!
//! Wraps esp-hal's proven monotonic clock as the embassy time source.
//! This avoids direct SYSTIMER register access which varies by chip revision.

use embassy_time_driver::Driver;
use portable_atomic::{AtomicU64, Ordering};

/// esp-hal's Instant uses the SYSTIMER internally and always works.
fn now_micros() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}

struct EspTimeDriver {
    alarm_at: AtomicU64,
}

impl Driver for EspTimeDriver {
    fn now(&self) -> u64 {
        now_micros()
    }

    fn schedule_wake(&self, at: u64, _waker: &core::task::Waker) {
        self.alarm_at.store(at, Ordering::Release);
        // block_on polls now() in a spin loop, so no alarm interrupt needed
    }
}

embassy_time_driver::time_driver_impl!(static DRIVER: EspTimeDriver = EspTimeDriver {
    alarm_at: AtomicU64::new(u64::MAX),
});

/// Initialize the time driver. No-op since esp-hal's Instant is always available.
pub fn init() {
    // esp_hal::time::Instant works immediately after esp_hal::init()
}
