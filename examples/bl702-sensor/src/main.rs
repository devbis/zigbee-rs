//! # BL702 Zigbee Temperature Sensor
//!
//! A `no_std` firmware for the BL702 (Bouffalo Lab, RISC-V),
//! implementing a Zigbee 3.0 end device that exposes a ZCL Temperature
//! Measurement cluster (0x0402).
//!
//! ## Hardware
//! - BL702 module (XT-ZB1, DT-BL10, Pine64 Pinenut, or similar)
//! - Built-in IEEE 802.15.4 + BLE 5.0 radio
//!
//! ## Radio driver
//! The BL702 backend uses FFI bindings to Bouffalo's `lmac154` C library
//! (`liblmac154.a`) for 802.15.4 radio access.
//!
//! ## Building
//! ```bash
//! cd examples/bl702-sensor
//! LMAC154_LIB_DIR=/path/to/lib cargo build --release
//! ```

#![no_std]
#![no_main]

use panic_halt as _;

use zigbee_mac::bl702::Bl702Mac;
use zigbee_nwk::DeviceType;
use zigbee_runtime::ZigbeeDevice;
use zigbee_zcl::clusters::temperature::TemperatureCluster;

// HA profile + device type
const PROFILE_HA: u16 = 0x0104;
const DEVICE_TYPE_TEMP_SENSOR: u16 = 0x0302;

// ZCL cluster IDs
const CLUSTER_BASIC: u16 = 0x0000;
const CLUSTER_TEMPERATURE: u16 = 0x0402;

/// Entry point — placeholder until Embassy executor is available for BL702.
///
/// On real hardware you would:
/// 1. Init BL702 clocks via bl702-pac
/// 2. Set up Embassy time driver using a BL702 timer
/// 3. Register M154 interrupt → driver callbacks
/// 4. Run async executor
#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    // Create 802.15.4 MAC driver
    let mac = Bl702Mac::new();

    // ZCL clusters
    let mut _temp = TemperatureCluster::new(-4000, 12500);

    // Build Zigbee device
    let mut _device = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::EndDevice)
        .manufacturer("Zigbee-RS")
        .model("BL702 Sensor")
        .sw_build("0.1.0")
        .channels(zigbee_types::ChannelMask::default())
        .endpoint(1, PROFILE_HA, DEVICE_TYPE_TEMP_SENSOR, |ep| {
            ep.cluster_server(CLUSTER_BASIC)
                .cluster_server(CLUSTER_TEMPERATURE)
        })
        .build();

    // TODO: Replace with async executor loop once Embassy supports BL702.
    //
    // Intended pattern:
    //   loop {
    //       let event = event_loop::stack_tick(&mut device).await;
    //       // handle events, report temperature
    //       Timer::after(Duration::from_secs(30)).await;
    //   }
    loop {
        core::hint::spin_loop();
    }
}
