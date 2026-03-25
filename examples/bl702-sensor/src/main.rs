//! # BL702 Zigbee Temperature Sensor
//!
//! A `no_std` firmware skeleton for the BL702 (Bouffalo Lab, RISC-V),
//! implementing a Zigbee 3.0 end device that exposes a ZCL Temperature
//! Measurement cluster (0x0402).
//!
//! ## Hardware
//! - BL702 module (XT-ZB1, DT-BL10, Pine64 Pinenut, or similar)
//! - Built-in IEEE 802.15.4 + BLE 5.0 radio
//!
//! ## Radio driver
//! The BL702 backend uses FFI bindings to Bouffalo's `lmac154` C library
//! (`liblmac154.a`) for 802.15.4 radio access. The firmware must link
//! this library and register the M154 interrupt handler at startup.
//!
//! ## Building
//! ```bash
//! cd examples/bl702-sensor
//! cargo build --release --target riscv32imac-unknown-none-elf
//! ```

#![no_std]
#![no_main]

use panic_halt as _;

use zigbee_aps::PROFILE_HOME_AUTOMATION;
use zigbee_nwk::DeviceType;
use zigbee_runtime::event_loop::{self, StackEvent, TickResult};
use zigbee_zcl::clusters::basic::BasicCluster;
use zigbee_zcl::clusters::temperature::TemperatureCluster;

// ZCL cluster IDs
const CLUSTER_BASIC: u16 = 0x0000;
const CLUSTER_TEMPERATURE: u16 = 0x0402;

// HA device type: Temperature Sensor
const DEVICE_TYPE_TEMP_SENSOR: u16 = 0x0302;

// Sensor reading interval
const READING_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Entry point
//
// Note: The BL702 does not yet have a standardized Embassy executor or HAL
// entry point macro. This skeleton uses a bare `main` function. In practice,
// you would need to set up:
//   1. Clock configuration (bl702-hal)
//   2. Embassy time driver (custom, using BL702 timer peripheral)
//   3. Radio interrupt handler (calling bl702 driver callbacks)
//   4. Async executor (embassy-executor with RISC-V support)
// ---------------------------------------------------------------------------

/// Entry point — to be adapted for actual BL702 Embassy runtime.
///
/// This function demonstrates the intended usage pattern. On real hardware,
/// it would run inside an async executor with proper interrupt handling.
fn main() -> ! {
    // -----------------------------------------------------------------------
    // 1. Peripheral initialisation (placeholder)
    // -----------------------------------------------------------------------
    // TODO: Initialize BL702 clocks, GPIOs, and radio peripheral using
    // bl702-hal. Example (when HAL matures):
    //
    //   let dp = bl702_pac::Peripherals::take().unwrap();
    //   let clocks = bl702_hal::clock::Clocks::new(dp.GLB, dp.HBN);
    //   // Enable radio peripheral clock
    //   // Configure radio interrupt handler

    // -----------------------------------------------------------------------
    // 2. IEEE 802.15.4 MAC driver
    // -----------------------------------------------------------------------
    let mac = zigbee_mac::bl702::Bl702Mac::new();

    // -----------------------------------------------------------------------
    // 3. ZCL cluster instances
    // -----------------------------------------------------------------------
    let mut temp_cluster = TemperatureCluster::new(-4000, 12500);

    let _basic = BasicCluster::new(
        b"Zigbee-RS",          // manufacturer name
        b"BL702 Sensor",       // model identifier
        b"20250101",           // date code
        b"0.1.0",              // software build
    );

    // -----------------------------------------------------------------------
    // 4. Build the Zigbee device
    // -----------------------------------------------------------------------
    let mut device = zigbee_runtime::builder(mac)
        .device_type(DeviceType::EndDevice)
        .manufacturer("Zigbee-RS")
        .model("BL702 Sensor")
        .sw_build("0.1.0")
        .channels(zigbee_types::ChannelMask::default())
        .endpoint(1, PROFILE_HOME_AUTOMATION, DEVICE_TYPE_TEMP_SENSOR, |ep| {
            ep.cluster_server(CLUSTER_BASIC)
                .cluster_server(CLUSTER_TEMPERATURE)
        })
        .build();

    // -----------------------------------------------------------------------
    // 5. Main loop (blocking placeholder — async version needed for real use)
    //
    // On real hardware this would be an async loop inside an Embassy executor:
    //
    //   loop {
    //       match event_loop::stack_tick(&mut device).await { ... }
    //       Timer::after(Duration::from_secs(READING_INTERVAL_SECS)).await;
    //   }
    // -----------------------------------------------------------------------
    loop {
        // TODO: Replace with async executor loop.
        //
        // The Zigbee stack requires an async runtime to function. Until
        // Embassy executor support is available for BL702, this is a
        // placeholder demonstrating the API shape.
        //
        // Simulated temperature reading (would come from I2C sensor):
        // temp_cluster.set_temperature(2350); // 23.50 °C

        // Busy-wait (placeholder for proper sleep/async)
        for _ in 0..1_000_000 {
            core::hint::spin_loop();
        }
    }
}
