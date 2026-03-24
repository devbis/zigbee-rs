//! Pre-built device type templates.
//!
//! These provide convenient constructors for common Zigbee device types
//! with the correct endpoints, clusters, and device IDs pre-configured.
//!
//! All templates use the Home Automation profile (0x0104).

use crate::builder::DeviceBuilder;
use zigbee_mac::MacDriver;
use zigbee_nwk::DeviceType;

/// HA profile ID
const HA_PROFILE: u16 = 0x0104;

/// Create a temperature sensor device (HA Device ID: 0x0302).
///
/// Endpoint 1:
/// - Basic (0x0000) — server
/// - Power Configuration (0x0001) — server
/// - Identify (0x0003) — server
/// - Temperature Measurement (0x0402) — server
pub fn temperature_sensor<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::EndDevice)
        .endpoint(1, HA_PROFILE, 0x0302, |ep| {
            ep.cluster_server(0x0000) // Basic
                .cluster_server(0x0001) // Power Configuration
                .cluster_server(0x0003) // Identify
                .cluster_server(0x0402) // Temperature Measurement
        })
}

/// Create a temperature + humidity sensor device (HA Device ID: 0x0302).
///
/// Endpoint 1:
/// - Basic, Power Config, Identify
/// - Temperature Measurement (0x0402) — server
/// - Relative Humidity (0x0405) — server
pub fn temperature_humidity_sensor<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::EndDevice)
        .endpoint(1, HA_PROFILE, 0x0302, |ep| {
            ep.cluster_server(0x0000)
                .cluster_server(0x0001)
                .cluster_server(0x0003)
                .cluster_server(0x0402)
                .cluster_server(0x0405)
        })
}

/// Create an on/off light device (HA Device ID: 0x0100).
///
/// Endpoint 1:
/// - Basic, Identify, Groups, Scenes
/// - On/Off (0x0006) — server
pub fn on_off_light<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::Router) // Lights are typically routers
        .endpoint(1, HA_PROFILE, 0x0100, |ep| {
            ep.cluster_server(0x0000) // Basic
                .cluster_server(0x0003) // Identify
                .cluster_server(0x0004) // Groups
                .cluster_server(0x0005) // Scenes
                .cluster_server(0x0006) // On/Off
        })
}

/// Create a dimmable light device (HA Device ID: 0x0101).
///
/// Endpoint 1:
/// - Basic, Identify, Groups, Scenes
/// - On/Off (0x0006) — server
/// - Level Control (0x0008) — server
pub fn dimmable_light<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::Router)
        .endpoint(1, HA_PROFILE, 0x0101, |ep| {
            ep.cluster_server(0x0000)
                .cluster_server(0x0003)
                .cluster_server(0x0004)
                .cluster_server(0x0005)
                .cluster_server(0x0006)
                .cluster_server(0x0008) // Level Control
        })
}

/// Create a color temperature light device (HA Device ID: 0x010C).
///
/// Endpoint 1:
/// - Basic, Identify, Groups, Scenes
/// - On/Off, Level Control
/// - Color Control (0x0300) — server
pub fn color_temperature_light<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::Router)
        .endpoint(1, HA_PROFILE, 0x010C, |ep| {
            ep.cluster_server(0x0000)
                .cluster_server(0x0003)
                .cluster_server(0x0004)
                .cluster_server(0x0005)
                .cluster_server(0x0006)
                .cluster_server(0x0008)
                .cluster_server(0x0300) // Color Control
        })
}

/// Create a contact sensor / IAS zone device (HA Device ID: 0x0402).
///
/// Endpoint 1:
/// - Basic, Power Config, Identify
/// - IAS Zone (0x0500) — server
pub fn contact_sensor<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::EndDevice)
        .endpoint(1, HA_PROFILE, 0x0402, |ep| {
            ep.cluster_server(0x0000)
                .cluster_server(0x0001)
                .cluster_server(0x0003)
                .cluster_server(0x0500) // IAS Zone
        })
}

/// Create an occupancy sensor device (HA Device ID: 0x0107).
///
/// Endpoint 1:
/// - Basic, Power Config, Identify
/// - Occupancy Sensing (0x0406) — server
pub fn occupancy_sensor<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::EndDevice)
        .endpoint(1, HA_PROFILE, 0x0107, |ep| {
            ep.cluster_server(0x0000)
                .cluster_server(0x0001)
                .cluster_server(0x0003)
                .cluster_server(0x0406) // Occupancy Sensing
        })
}

/// Create a smart plug / on-off outlet device (HA Device ID: 0x0009).
///
/// Endpoint 1:
/// - Basic, Identify, Groups, Scenes
/// - On/Off (0x0006) — server
/// - Electrical Measurement (0x0B04) — server
pub fn smart_plug<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::Router)
        .endpoint(1, HA_PROFILE, 0x0009, |ep| {
            ep.cluster_server(0x0000)
                .cluster_server(0x0003)
                .cluster_server(0x0004)
                .cluster_server(0x0005)
                .cluster_server(0x0006)
                .cluster_server(0x0B04) // Electrical Measurement
        })
}

/// Create a thermostat device (HA Device ID: 0x0301).
///
/// Endpoint 1:
/// - Basic, Identify, Groups
/// - Thermostat (0x0201) — server
/// - Temperature Measurement (0x0402) — server (local temp)
pub fn thermostat<M: MacDriver>(mac: M) -> DeviceBuilder<M> {
    DeviceBuilder::new(mac)
        .device_type(DeviceType::Router)
        .endpoint(1, HA_PROFILE, 0x0301, |ep| {
            ep.cluster_server(0x0000)
                .cluster_server(0x0003)
                .cluster_server(0x0004)
                .cluster_server(0x0201) // Thermostat
                .cluster_server(0x0402) // Temperature Measurement
        })
}
