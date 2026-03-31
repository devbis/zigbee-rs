//! # Telink TLSR8258 Zigbee Temperature Sensor
//!
//! A `no_std` firmware for the Telink TLSR8258 (tc32 ISA),
//! implementing a Zigbee 3.0 end device with Temperature
//! Measurement (0x0402) and Relative Humidity (0x0405) clusters.
//!
//! ## Hardware
//! - Telink TLSR8258 module (tc32 core, 512KB Flash, 64KB SRAM)
//! - Built-in IEEE 802.15.4 + BLE radio
//! - Used in many Zigbee products (Sonoff SNZB, Tuya, IKEA devices)
//!
//! ## Note on tc32 ISA
//! The TLSR8258 uses Telink's proprietary tc32 instruction set.
//! There is no official Rust target for tc32. For `cargo check`,
//! we use `thumbv6m-none-eabi` as a compilation stand-in to verify
//! the Rust code compiles. Real builds require the Telink tc32 GCC
//! toolchain and would link via C firmware that calls into the Rust
//! static library.
//!
//! ## Building (cargo check only)
//! ```bash
//! cd examples/telink-tlsr8258-sensor
//! cargo check --release
//! ```

#![no_std]
#![no_main]

#[cfg(feature = "stubs")]
mod stubs;

use panic_halt as _;

use zigbee_aps::PROFILE_HOME_AUTOMATION;
use zigbee_mac::telink::TelinkMac;
use zigbee_nwk::DeviceType;
use zigbee_runtime::{UserAction, ZigbeeDevice};
use zigbee_zcl::clusters::humidity::HumidityCluster;
use zigbee_zcl::clusters::temperature::TemperatureCluster;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

const REPORT_INTERVAL_SECS: u64 = 30;

// ── Minimal GPIO (tc32 register-mapped) ────────────────────────

mod gpio {
    /// TLSR8258 GPIO group A data output register.
    const GPIO_PA_OUT: u32 = 0x00586 + 0x01;
    /// TLSR8258 GPIO group A data input register.
    const GPIO_PA_IN: u32 = 0x00586;
    /// TLSR8258 GPIO group A output enable register.
    const GPIO_PA_OEN: u32 = 0x00586 + 0x09;
    /// TLSR8258 GPIO group A input enable register.
    const GPIO_PA_IEN: u32 = 0x00586 + 0x0A;
    /// TLSR8258 GPIO group A pull-up enable register.
    const GPIO_PA_PULL: u32 = 0x00586 + 0x0D;

    pub fn configure_input_pullup(pin: u8) {
        unsafe {
            // Enable input
            let ien = GPIO_PA_IEN as *mut u8;
            core::ptr::write_volatile(ien, core::ptr::read_volatile(ien) | (1 << pin));
            // Enable pull-up (10k)
            let pull = GPIO_PA_PULL as *mut u8;
            core::ptr::write_volatile(pull, core::ptr::read_volatile(pull) | (1 << pin));
        }
    }

    pub fn set_output(pin: u8) {
        unsafe {
            // Enable output
            let oen = GPIO_PA_OEN as *mut u8;
            core::ptr::write_volatile(oen, core::ptr::read_volatile(oen) | (1 << pin));
        }
    }

    pub fn write(pin: u8, high: bool) {
        unsafe {
            let reg = GPIO_PA_OUT as *mut u8;
            let val = core::ptr::read_volatile(reg);
            if high {
                core::ptr::write_volatile(reg, val | (1 << pin));
            } else {
                core::ptr::write_volatile(reg, val & !(1 << pin));
            }
        }
    }

    pub fn read_input(pin: u8) -> bool {
        unsafe {
            let reg = GPIO_PA_IN as *const u8;
            let val = core::ptr::read_volatile(reg);
            (val >> pin) & 1 == 1
        }
    }
}

// ── Embassy time driver (reads TLSR8258 system timer) ──────────

mod time_driver {
    use embassy_time_driver::Driver;

    /// TLSR8258 system timer register (32-bit, free-running).
    /// Ticks at system clock rate. At 16 MHz → 16 ticks/µs.
    const REG_SYS_TIMER: u32 = 0x740;

    /// System clock ticks per microsecond (16 MHz default).
    const TICKS_PER_US: u64 = 16;

    struct Tlsr8258TimeDriver;

    impl Tlsr8258TimeDriver {
        const fn new() -> Self {
            Self
        }

        /// Read the raw 32-bit system timer value.
        fn read_sys_timer(&self) -> u32 {
            unsafe { core::ptr::read_volatile(REG_SYS_TIMER as *const u32) }
        }
    }

    /// Track 64-bit time from the 32-bit hardware timer.
    static mut LAST_RAW: u32 = 0;
    static mut HIGH_BITS: u64 = 0;

    impl Driver for Tlsr8258TimeDriver {
        fn now(&self) -> u64 {
            let raw = self.read_sys_timer();
            // Extend 32-bit counter to 64-bit by detecting wraparound.
            // Safe in single-core ISR-masked context.
            unsafe {
                if raw < LAST_RAW {
                    HIGH_BITS += 1u64 << 32;
                }
                LAST_RAW = raw;
                // Convert from system clock ticks to microseconds
                (HIGH_BITS | raw as u64) / TICKS_PER_US
            }
        }

        fn schedule_wake(&self, _at: u64, _waker: &core::task::Waker) {
            // TODO: configure TLSR8258 system timer compare interrupt
            // to fire at the requested time. For now, Embassy polls.
        }
    }

    embassy_time_driver::time_driver_impl!(
        static TIME_DRIVER: Tlsr8258TimeDriver = Tlsr8258TimeDriver::new()
    );
}

// ── TLSR8258 devboard pins (e.g., Sonoff SNZB-02) ─────────────

mod pins {
    pub const BTN1: u8 = 2; // Button
    pub const LED1: u8 = 3; // LED
}

// ── RF interrupt routing ───────────────────────────────────────
// On real hardware, the top-level IRQ handler must route RF
// interrupts to the Telink MAC driver. The TLSR8258 RF IRQ is
// bit 4 of the IRQ status register at 0x643.

mod rf_irq {
    /// IRQ status register.
    const REG_IRQ_SRC: u32 = 0x643;
    /// RF IRQ mask bit.
    const FLD_IRQ_ZB_RT: u8 = 1 << 4;

    unsafe extern "C" {
        fn rf_rx_irq_handler();
        fn rf_tx_irq_handler();
    }

    /// Call from the platform's top-level IRQ handler.
    /// Checks RF IRQ source and dispatches to the Telink MAC driver.
    #[allow(dead_code)]
    pub unsafe fn dispatch_rf_irq() {
        unsafe {
            let src = core::ptr::read_volatile(REG_IRQ_SRC as *const u8);
            if src & FLD_IRQ_ZB_RT != 0 {
                // RF RX/TX IRQ — the driver's handler checks sub-status
                rf_rx_irq_handler();
                rf_tx_irq_handler();
            }
        }
    }
}

// ── Low-power sleep (for SED mode) ────────────────────────────

mod sleep {
    /// TLSR8258 power management: CPU wait-for-interrupt.
    /// Halts CPU until next interrupt — RAM and peripherals retained.
    #[inline]
    pub fn wfi() {
        // On tc32 / thumbv6m, WFI halts the core until an interrupt.
        // ARM-compatible instruction.
        unsafe { core::arch::asm!("wfi") };
    }

    /// Enter suspend mode with timer wakeup.
    /// The real implementation would call Telink PM_LowPwrEnter().
    #[allow(dead_code)]
    pub fn light_sleep_ms(_ms: u32) {
        // TODO: call into Telink PM driver for real low-power suspend
        // PM_LowPwrEnter(SUSPEND_MODE, PM_WAKEUP_TIMER, wakeup_tick);
        wfi();
    }
}

// ── Entry point ────────────────────────────────────────────────

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    log::info!("TLSR8258 Zigbee sensor starting");

    gpio::configure_input_pullup(pins::BTN1);
    gpio::set_output(pins::LED1);

    // Blink LED
    for _ in 0..3 {
        gpio::write(pins::LED1, true);
        Timer::after(Duration::from_millis(100)).await;
        gpio::write(pins::LED1, false);
        Timer::after(Duration::from_millis(100)).await;
    }

    let mac = TelinkMac::new();

    let mut temp_cluster = TemperatureCluster::new(-4000, 12500);
    let mut hum_cluster = HumidityCluster::new(0, 10000);

    let mut device = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::EndDevice)
        .manufacturer("Zigbee-RS")
        .model("TLSR8258-Sensor")
        .sw_build("0.1.0")
        .channels(zigbee_types::ChannelMask::ALL_2_4GHZ)
        .endpoint(1, PROFILE_HOME_AUTOMATION, 0x0302, |ep| {
            ep.cluster_server(0x0000) // Basic
                .cluster_server(0x0402) // Temperature Measurement
                .cluster_server(0x0405) // Relative Humidity
        })
        .build();

    log::info!("Device ready — press button to join/leave");

    let mut button_was_pressed = false;
    let mut tick: u32 = 0;

    loop {
        let pressed = !gpio::read_input(pins::BTN1);
        if pressed && !button_was_pressed {
            if device.is_joined() {
                log::info!("Button → leaving network");
            } else {
                log::info!("Button → joining network");
            }
            device.user_action(UserAction::Toggle);
            Timer::after(Duration::from_millis(300)).await;
        }
        button_was_pressed = pressed;

        let temp_hundredths: i16 = 2250 + ((tick % 50) as i16 - 25);
        let hum_hundredths: u16 = 5000 + ((tick % 100) as u16) * 10;

        temp_cluster.set_temperature(temp_hundredths);
        hum_cluster.set_humidity(hum_hundredths);

        if device.is_joined() {
            log::info!(
                "T={}.{:02}°C  H={}.{:02}%",
                temp_hundredths / 100,
                (temp_hundredths % 100).unsigned_abs(),
                hum_hundredths / 100,
                hum_hundredths % 100,
            );
        }

        tick = tick.wrapping_add(1);
        Timer::after(Duration::from_secs(REPORT_INTERVAL_SECS)).await;
    }
}
