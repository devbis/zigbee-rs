//! # Telink B91 Zigbee Temperature Sensor
//!
//! A `no_std` firmware for the Telink B91 (RISC-V),
//! implementing a Zigbee 3.0 end device that exposes Temperature
//! Measurement (0x0402) and Relative Humidity (0x0405) clusters.
//!
//! ## Hardware
//! - Telink B91 module (RISC-V 32-bit, 512KB Flash, 256KB SRAM)
//! - Built-in IEEE 802.15.4 + BLE 5.0 radio
//! - Button (GPIO2 on B91 devboard): join/leave network
//!
//! ## Radio driver
//! The Telink backend uses FFI bindings to Telink's RF driver library
//! (`libdrivers_b91.a`) for 802.15.4 radio access, with interrupt-driven
//! TX/RX via Embassy signals.
//!
//! ## Building
//! ```bash
//! cd examples/telink-b91-sensor
//! TELINK_SDK_DIR=/path/to/tl_zigbee_sdk cargo build --release
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

// ── Minimal GPIO helpers (B91 register-based) ──────────────────

mod gpio {
    /// B91 GPIO group A base address.
    const GPIO_PA_BASE: u32 = 0x140300;
    /// Offset for input data register.
    const GPIO_IN_OFFSET: u32 = 0x00;
    /// Offset for output data register.
    const GPIO_OUT_OFFSET: u32 = 0x04;
    /// Offset for output enable register.
    const GPIO_OEN_OFFSET: u32 = 0x08;
    /// Offset for input enable register.
    const GPIO_IEN_OFFSET: u32 = 0x0C;
    /// Offset for pull-up enable register.
    const GPIO_PU_OFFSET: u32 = 0x14;

    pub fn configure_input_pullup(pin: u8) {
        unsafe {
            let ien = (GPIO_PA_BASE + GPIO_IEN_OFFSET) as *mut u32;
            core::ptr::write_volatile(ien, core::ptr::read_volatile(ien) | (1 << pin));
            let pu = (GPIO_PA_BASE + GPIO_PU_OFFSET) as *mut u32;
            core::ptr::write_volatile(pu, core::ptr::read_volatile(pu) | (1 << pin));
        }
    }

    pub fn set_output(pin: u8) {
        unsafe {
            let oen = (GPIO_PA_BASE + GPIO_OEN_OFFSET) as *mut u32;
            core::ptr::write_volatile(oen, core::ptr::read_volatile(oen) | (1 << pin));
        }
    }

    pub fn write(pin: u8, high: bool) {
        unsafe {
            let reg = (GPIO_PA_BASE + GPIO_OUT_OFFSET) as *mut u32;
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
            let reg = (GPIO_PA_BASE + GPIO_IN_OFFSET) as *const u32;
            let val = core::ptr::read_volatile(reg);
            (val >> pin) & 1 == 1
        }
    }
}

// ── Embassy time driver (reads B91 system timer) ───────────────

mod time_driver {
    use embassy_time_driver::Driver;

    /// B91 system timer register (32-bit, free-running).
    /// Ticks at system clock rate. At 16 MHz → 16 ticks/µs.
    const REG_STIMER_TICK: u32 = 0x140200;

    /// System clock ticks per microsecond (16 MHz default).
    const TICKS_PER_US: u64 = 16;

    struct B91TimeDriver;

    impl B91TimeDriver {
        const fn new() -> Self {
            Self
        }

        fn read_sys_timer(&self) -> u32 {
            unsafe { core::ptr::read_volatile(REG_STIMER_TICK as *const u32) }
        }
    }

    /// Track 64-bit time from the 32-bit hardware timer.
    static mut LAST_RAW: u32 = 0;
    static mut HIGH_BITS: u64 = 0;

    impl Driver for B91TimeDriver {
        fn now(&self) -> u64 {
            let raw = self.read_sys_timer();
            // Extend 32-bit counter to 64-bit by detecting wraparound.
            // Safe in single-core ISR-masked context.
            unsafe {
                if raw < LAST_RAW {
                    HIGH_BITS += 1u64 << 32;
                }
                LAST_RAW = raw;
                (HIGH_BITS | raw as u64) / TICKS_PER_US
            }
        }

        fn schedule_wake(&self, _at: u64, _waker: &core::task::Waker) {
            // TODO: configure B91 stimer compare interrupt
            // to fire at the requested time. For now, Embassy polls.
        }
    }

    embassy_time_driver::time_driver_impl!(
        static TIME_DRIVER: B91TimeDriver = B91TimeDriver::new()
    );
}

// ── Logging ─────────────────────────────────────────────────────
// log::info!() etc. compile as no-ops without a registered logger.
// For real debug output, use Telink UART or BDT (Burning & Debug Tool).

// ── B91 devboard pin assignments ───────────────────────────────

mod pins {
    pub const BTN1: u8 = 2; // GPIO2 — button
    pub const LED1: u8 = 3; // GPIO3 — green LED
    pub const LED2: u8 = 4; // GPIO4 — blue LED
}

// ── RF interrupt routing ───────────────────────────────────────
// On real hardware, the top-level IRQ handler must route RF
// interrupts to the Telink MAC driver. The B91 uses PLIC; RF IRQ
// is typically interrupt source #15 (ZB_RT).

mod rf_irq {
    // B91 PLIC: the riscv-rt trap handler dispatches by IRQ number.
    // RF IRQ is typically source #15 (ZB_RT).
    unsafe extern "C" {
        fn rf_rx_irq_handler();
        fn rf_tx_irq_handler();
    }

    /// Call from the RISC-V trap handler when RF IRQ fires.
    #[allow(dead_code)]
    pub unsafe fn dispatch_rf_irq() {
        unsafe {
            rf_rx_irq_handler();
            rf_tx_irq_handler();
        }
    }
}

// ── Low-power sleep (for SED mode) ────────────────────────────

mod sleep {
    /// RISC-V WFI — halts CPU until next interrupt.
    /// RAM and peripherals retained, minimal power savings.
    #[inline]
    pub fn wfi() {
        unsafe { core::arch::asm!("wfi") };
    }

    /// Enter suspend mode with timer wakeup.
    /// The real implementation would call Telink pm_sleep_wakeup().
    #[allow(dead_code)]
    pub fn light_sleep_ms(_ms: u32) {
        // TODO: call into Telink PM driver for real low-power suspend
        // pm_sleep_wakeup(SUSPEND_MODE, PM_WAKEUP_TIMER, tick);
        wfi();
    }
}

// ── Entry point ────────────────────────────────────────────────

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    log::info!("Telink B91 Zigbee sensor starting");

    // Configure GPIO
    gpio::configure_input_pullup(pins::BTN1);
    gpio::set_output(pins::LED1);
    gpio::set_output(pins::LED2);

    // Blink LED to show alive
    for _ in 0..3 {
        gpio::write(pins::LED1, true);
        Timer::after(Duration::from_millis(100)).await;
        gpio::write(pins::LED1, false);
        Timer::after(Duration::from_millis(100)).await;
    }

    // Create MAC driver
    let mac = TelinkMac::new();

    // ZCL cluster instances
    let mut temp_cluster = TemperatureCluster::new(-4000, 12500);
    let mut hum_cluster = HumidityCluster::new(0, 10000);

    // Build Zigbee device
    let mut device = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::EndDevice)
        .manufacturer("Zigbee-RS")
        .model("B91-Sensor")
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
        // Button handling (edge detection, active low)
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

        // Simulated sensor readings
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
