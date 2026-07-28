//! Smart plug application logic.
//!
//! Features:
//! - BL0937 energy monitoring (voltage, current, power)
//! - Relay control via On/Off cluster
//! - Overload protection (configurable thresholds)
//! - Energy persistence via NvStorage

use core::mem::MaybeUninit;

use zigbee_aps::PROFILE_HOME_AUTOMATION;
use zigbee_mac::{MacError, telink::TelinkMac};
use zigbee_nwk::DeviceType;
use zigbee_runtime::ZigbeeDevice;
use zigbee_runtime::event_loop::{StackEvent, StartError};
use zigbee_runtime::power::PowerMode;
use zigbee_runtime::profile::smart_plug::{ElectricalReading, SmartPlug, SmartPlugReporting};
use zigbee_zcl::clusters::basic::PowerSource;
use zigbee_zcl::{ClusterId, DeviceId};

use tlsr8258_ts011f::{leds as board, storage};

use crate::board::{Cf1Counter, CfCounter, SelPinOutput};
use crate::energy;

const DEVICE_EUI_OFFSET: u8 = 0x53; // 'S' for Smart plug

/// Overload thresholds (configurable via custom attributes).
struct OverloadConfig {
    voltage_max: u16, // centi-volts (e.g. 26400 = 264.00V)
    current_max: u16, // centi-amps (e.g. 1600 = 16.00A)
    power_max: u16,   // watts
}

impl Default for OverloadConfig {
    fn default() -> Self {
        Self {
            voltage_max: 26400, // 264V
            current_max: 1600,  // 16A
            power_max: 3680,    // 3680W (230V * 16A)
        }
    }
}

fn failure() -> ! {
    board::LED_GREEN.write(false);
    board::LED_BLUE.write(false);
    board::LED_RED.write(true);
    loop {
        tlsr8258_hal::timer::sleep_ticks(tlsr8258_hal::timer::ms(1_000));
    }
}

pub fn run() -> ! {
    type Device = ZigbeeDevice<TelinkMac>;

    // Initialize BL0937 energy monitoring IC
    let cf = CfCounter;
    let cf1 = Cf1Counter;
    let sel = SelPinOutput;
    let calibration = zigbee_bl0937::Calibration::theoretical();
    let mut bl0937 = zigbee_bl0937::Bl0937::new(cf, cf1, sel, calibration, 5);

    // Configure board pins
    tlsr8258_ts011f::configure();
    if board::configure_status_leds().is_err() {
        failure();
    }

    // Configure relay
    let relay = tlsr8258_ts011f::RelayPin::new();

    // Overload protection config
    let overload = OverloadConfig::default();

    // Energy persistence
    let mut energy_secs: u32 = 0;

    // Zigbee device setup
    let mut ieee_address = [0u8; 8];
    tlsr8258_hal::flash::factory_ieee(&mut ieee_address);
    ieee_address[0] = ieee_address[0].wrapping_add(DEVICE_EUI_OFFSET);
    let mac = TelinkMac::with_extended_address(ieee_address);

    static mut DEVICE_STORAGE: MaybeUninit<Device> = MaybeUninit::uninit();

    let device = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::Router)
        .power_mode(PowerMode::AlwaysOn)
        .manufacturer("Zigbee-RS")
        .model("TS011F-SmartPlug")
        .date_code("20260728")
        .sw_build("0.1.0")
        .power_source(PowerSource::MainsSinglePhase)
        .channels(zigbee_types::ChannelMask(1 << 15))
        .endpoint(
            1,
            PROFILE_HOME_AUTOMATION,
            DeviceId::MAINS_POWER_OUTLET,
            |endpoint| {
                endpoint
                    .cluster_server(ClusterId::BASIC)
                    .cluster_server(ClusterId::IDENTIFY)
                    .cluster_server(ClusterId::ON_OFF)
                    .cluster_server(ClusterId::ELECTRICAL_MEASUREMENT)
            },
        )
        .build_into(unsafe { &mut *core::ptr::addr_of_mut!(DEVICE_STORAGE) });

    let mut security_store = storage::security_store();
    if device
        .reset_security_state_if_identity_changed(&mut security_store)
        .is_err()
    {
        failure();
    }

    // NV storage for energy persistence (separate flash partition)
    let mut nv_store = match storage::nv_store() {
        Ok(store) => store,
        Err(_) => failure(),
    };

    // Smart plug component
    let mut smart_plug =
        SmartPlug::new(SmartPlugReporting::default()).with_metering(
            zigbee_zcl::clusters::metering::UNIT_KWH,
            1,
            1_000, // divisor: 1000 → 1 Wh resolution
        );

    // Load saved energy from flash
    energy::load_energy(&mut nv_store, &mut smart_plug);

    'commission: loop {
        let mut attempts = 0u8;
        loop {
            attempts = attempts.saturating_add(1);
            match tlsr8258_rt::block_on(
                device.start_or_resume_with_security_store(&mut security_store),
            ) {
                Ok(_) => break,
                Err(StartError::CommissioningFailed(_)) if attempts < 10 => {
                    tlsr8258_hal::timer::sleep_ticks(tlsr8258_hal::timer::ms(5_000));
                }
                Err(_) => failure(),
            }
        }

        board::LED_RED.write(false);
        board::LED_GREEN.write(true);
        board::LED_BLUE.write(false);

        let mut identify_elapsed = 0u32;
        let one_second = tlsr8258_hal::timer::ms(1_000);
        let mut tick_anchor = tlsr8258_hal::timer::now_ticks();

        loop {
            match tlsr8258_rt::block_on(device.receive()) {
                Ok(indication) => {
                    let event = tlsr8258_rt::block_on(device.process_incoming_with_security_store(
                        &indication,
                        &mut [],
                        &mut security_store,
                    ));
                    match event {
                        Ok(Some(StackEvent::RejoinRequested)) => {
                            let _ = tlsr8258_rt::block_on(
                                device.secure_rejoin_with_security_store(&mut security_store),
                            );
                        }
                        Ok(Some(StackEvent::LeaveRequested)) => {
                            if tlsr8258_rt::block_on(
                                device.factory_reset_with_security_store(&mut security_store),
                            )
                            .is_err()
                            {
                                failure();
                            }
                            board::LED_GREEN.write(false);
                            board::LED_RED.write(true);
                            continue 'commission;
                        }
                        Ok(_) => {}
                        Err(_) => failure(),
                    }

                    if tlsr8258_rt::block_on(device.tick_with_security_store(
                        0,
                        &mut [],
                        &mut security_store,
                    ))
                    .is_err()
                    {
                        failure();
                    }
                }
                Err(MacError::NoData) => {}
                Err(_) => failure(),
            }

            let now = tlsr8258_hal::timer::now_ticks();
            let elapsed = now.wrapping_sub(tick_anchor);
            if elapsed >= one_second {
                let elapsed_secs = (elapsed / one_second).min(u16::MAX as u32) as u16;
                tick_anchor = tick_anchor.wrapping_add(u32::from(elapsed_secs) * one_second);
                identify_elapsed =
                    identify_elapsed.wrapping_add(u32::from(elapsed_secs));

                // BL0937 measurement
                let reading = bl0937.read();

                // Update SmartPlug electrical measurements
                smart_plug.update_electrical(ElectricalReading {
                    rms_voltage: reading.voltage_rms,
                    rms_current: reading.current_rms,
                    active_power_watts: reading.active_power as i16,
                });

                // Set instantaneous demand for metering cluster
                smart_plug
                    .set_instantaneous_demand_watts(reading.active_power as i32);

                // Add energy delivered (convert pulses to Wh)
                let energy_wh = zigbee_bl0937::pulses_to_wh(
                    reading.energy_pulses as u64,
                    bl0937.read().energy_pulses as f32, // re-read for calibration
                    1_000_000,                          // 1 second window
                );
                if energy_wh > 0 {
                    smart_plug.add_energy_delivered_wh(energy_wh);
                }

                // Relay control based on On/Off state
                relay.set_on(smart_plug.is_on());

                // Overload protection
                if smart_plug.is_on() {
                    if reading.voltage_rms > overload.voltage_max
                        || reading.current_rms > overload.current_max
                        || reading.active_power > overload.power_max as i16
                    {
                        // Overload detected — turn off relay
                        relay.set_on(false);
                        board::LED_RED.write(true);
                        board::LED_GREEN.write(false);
                    } else {
                        board::LED_RED.write(false);
                        board::LED_GREEN.write(true);
                    }
                }

                // Energy persistence (every 60 seconds)
                energy_secs += u32::from(elapsed_secs);
                if energy_secs >= energy::PERSIST_INTERVAL_SECS {
                    energy_secs = 0;
                    energy::save_energy(&mut nv_store, &smart_plug);
                }

                if tlsr8258_rt::block_on(device.tick_with_security_store(
                    elapsed_secs,
                    &mut [],
                    &mut security_store,
                ))
                .is_err()
                {
                    failure();
                }

                // On/Off tick for timed-off support (100ms cadence)
                smart_plug.tick_on_off();

                if device.is_identifying(1) {
                    board::LED_BLUE.write((identify_elapsed & 1) == 0);
                } else {
                    board::LED_BLUE.write(false);
                }
            }
        }
    }
}
