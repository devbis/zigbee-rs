//! Zigbee Device Runtime — the top-level integration layer.
//!
//! This crate provides:
//! - `ZigbeeDevice` builder API for easy device creation
//! - Event loop that drives MAC→NWK→APS→ZCL processing
//! - NV storage abstraction for persistent state
//! - Power management hooks for sleepy end devices
//! - Pre-built device type templates (sensor, light, switch, etc.)
//!
//! # Example
//! ```rust,no_run,ignore
//! use zigbee_runtime::ZigbeeDevice;
//! use zigbee_mac::mock::MockMac;
//!
//! let mac = MockMac::new([1,2,3,4,5,6,7,8]);
//! let mut device = ZigbeeDevice::builder(mac)
//!     .device_type(DeviceType::EndDevice)
//!     .endpoint(1, 0x0104, 0x0302, |ep| {
//!         ep.cluster_server(0x0000)  // Basic
//!           .cluster_server(0x0402)  // Temperature Measurement
//!     })
//!     .build();
//!
//! device.start().await;
//! ```

#![no_std]
#![allow(async_fn_in_trait)]

pub mod builder;
pub mod event_loop;
pub mod nv_storage;
pub mod power;
pub mod templates;

use zigbee_aps::ApsAddress;
use zigbee_bdb::BdbLayer;
use zigbee_mac::{MacDriver, MacError, McpsDataIndication};
use zigbee_types::*;
use zigbee_zcl::foundation::reporting::ReportingEngine;
use zigbee_zcl::frame::ZclFrame;

use crate::power::PowerManager;

/// Maximum number of endpoints on a device (endpoint 0 is ZDO, 1-240 are application)
pub const MAX_ENDPOINTS: usize = 8;
/// Maximum clusters per endpoint
pub const MAX_CLUSTERS_PER_ENDPOINT: usize = 16;

/// Endpoint configuration.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    pub endpoint: u8,
    pub profile_id: u16,
    pub device_id: u16,
    pub device_version: u8,
    pub server_clusters: heapless::Vec<u16, MAX_CLUSTERS_PER_ENDPOINT>,
    pub client_clusters: heapless::Vec<u16, MAX_CLUSTERS_PER_ENDPOINT>,
}

/// User-initiated actions, triggered by button presses or application logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    /// Join a network (BDB commissioning).
    Join,
    /// Leave the current network.
    Leave,
    /// Toggle join/leave based on current state.
    Toggle,
    /// Open permit joining (coordinator/router only).
    PermitJoin(u8),
    /// Factory reset — leave network and clear all state.
    FactoryReset,
}

/// The running Zigbee device — owns the full BDB→ZDO→APS→NWK→MAC stack.
pub struct ZigbeeDevice<M: MacDriver> {
    /// BDB layer (transitively owns ZDO → APS → NWK → MAC).
    bdb: BdbLayer<M>,
    /// Application endpoint configurations.
    endpoints: heapless::Vec<EndpointConfig, MAX_ENDPOINTS>,
    /// ZCL attribute reporting engine.
    reporting: ReportingEngine,
    /// Power management.
    power: PowerManager,
    /// Pending user action (set by button press, consumed by tick).
    pending_action: Option<UserAction>,
    /// ZCL transaction sequence counter.
    zcl_seq: u8,
    /// Device metadata.
    manufacturer_name: &'static str,
    model_identifier: &'static str,
    sw_build_id: &'static str,
    /// Channel mask for network scanning.
    channel_mask: ChannelMask,
}

impl<M: MacDriver> ZigbeeDevice<M> {
    /// Create a new device builder.
    pub fn builder(mac: M) -> builder::DeviceBuilder<M> {
        builder::DeviceBuilder::new(mac)
    }

    /// Allocate the next ZCL sequence number.
    fn next_zcl_seq(&mut self) -> u8 {
        let s = self.zcl_seq;
        self.zcl_seq = self.zcl_seq.wrapping_add(1);
        s
    }

    // ── Network lifecycle ───────────────────────────────────

    /// Initialize and join a Zigbee network via BDB commissioning.
    ///
    /// Performs BDB initialize → commission (network steering).
    /// Returns the assigned short address on success.
    pub async fn start(&mut self) -> Result<u16, event_loop::StartError> {
        log::info!("[Runtime] Starting Zigbee device…");

        // BDB initialize
        self.bdb
            .initialize()
            .await
            .map_err(|_| event_loop::StartError::InitFailed)?;

        // BDB commission (steering for end devices, formation for coordinators)
        self.bdb
            .commission()
            .await
            .map_err(|_| event_loop::StartError::CommissioningFailed)?;

        let addr = self.bdb.zdo().nwk().nib().network_address.0;
        log::info!("[Runtime] Joined network as 0x{:04X}", addr);
        Ok(addr)
    }

    /// Leave the current Zigbee network.
    pub async fn leave(&mut self) -> Result<(), event_loop::StartError> {
        log::info!("[Runtime] Leaving network…");
        self.bdb
            .zdo_mut()
            .nwk_mut()
            .nlme_leave(false)
            .await
            .map_err(|_| event_loop::StartError::InitFailed)?;
        self.bdb.attributes_mut().node_is_on_a_network = false;
        log::info!("[Runtime] Left network");
        Ok(())
    }

    // ── User action API ─────────────────────────────────────

    /// Queue a user action (e.g., from a button press).
    /// Will be processed on the next call to `tick()`.
    pub fn user_action(&mut self, action: UserAction) {
        self.pending_action = Some(action);
    }

    // ── Query state ─────────────────────────────────────────

    /// Whether the device is currently joined to a network.
    pub fn is_joined(&self) -> bool {
        self.bdb.is_on_network()
    }

    /// The device's NWK short address (0xFFFF if not joined).
    pub fn short_address(&self) -> u16 {
        self.bdb.zdo().nwk().nib().network_address.0
    }

    /// The current operating channel (0 if not joined).
    pub fn channel(&self) -> u8 {
        self.bdb.zdo().nwk().nib().logical_channel
    }

    /// The current PAN ID (0xFFFF if not joined).
    pub fn pan_id(&self) -> u16 {
        self.bdb.zdo().nwk().nib().pan_id.0
    }

    /// The device type (coordinator / router / end device).
    pub fn device_type(&self) -> zigbee_nwk::DeviceType {
        self.bdb.zdo().nwk().device_type()
    }

    /// The configured application endpoints.
    pub fn endpoints(&self) -> &[EndpointConfig] {
        &self.endpoints
    }

    /// The manufacturer name.
    pub fn manufacturer_name(&self) -> &str {
        self.manufacturer_name
    }

    /// The model identifier.
    pub fn model_identifier(&self) -> &str {
        self.model_identifier
    }

    /// The configured channel mask.
    pub fn channel_mask(&self) -> ChannelMask {
        self.channel_mask
    }

    // ── MAC proxy ───────────────────────────────────────────

    /// Wait for an incoming MAC frame. Blocks until a frame arrives.
    ///
    /// Use with `select!` and a timer for non-blocking operation:
    /// ```rust,ignore
    /// select! {
    ///     frame = device.receive() => { device.process_incoming(&frame.unwrap()); }
    ///     _ = Timer::after(Duration::from_secs(1)) => { device.tick(1).await; }
    /// }
    /// ```
    pub async fn receive(&mut self) -> Result<McpsDataIndication, MacError> {
        self.bdb
            .zdo_mut()
            .aps_mut()
            .nwk_mut()
            .mac_mut()
            .mcps_data_indication()
            .await
    }

    // ── Incoming frame processing ───────────────────────────

    /// Process an incoming MAC frame through the full stack.
    ///
    /// MAC → NWK → APS → ZDO (endpoint 0) or ZCL (app endpoints).
    pub fn process_incoming(
        &mut self,
        indication: &McpsDataIndication,
    ) -> Option<event_loop::StackEvent> {
        let mac_payload = indication.payload.as_slice();

        // NWK layer: parse NWK header, check if frame is for us
        let nwk_indication = {
            let nwk = self.bdb.zdo_mut().aps_mut().nwk_mut();
            let (header, consumed) = zigbee_nwk::frames::NwkHeader::parse(mac_payload)?;

            let dst = header.dst_addr;
            let src = header.src_addr;
            let nwk_addr = nwk.nib().network_address;
            let is_for_us = dst == nwk_addr
                || dst == ShortAddress::BROADCAST
                || dst == ShortAddress(0xFFFF)
                || dst == ShortAddress(0xFFFD);

            if !is_for_us {
                return None;
            }

            let payload = &mac_payload[consumed..];
            let mut buf = [0u8; 128];
            let len = payload.len().min(128);
            buf[..len].copy_from_slice(&payload[..len]);
            (dst, src, header.frame_control.security, buf, len)
        };

        let (dst, src, nwk_security, buf, len) = nwk_indication;

        // APS layer: parse APS header
        let aps_indication = self.bdb.zdo().aps().process_incoming_aps_frame(
            &buf[..len],
            src,
            dst,
            indication.lqi,
            nwk_security,
        )?;

        // Route by destination endpoint
        let dst_ep = aps_indication.dst_endpoint;
        let cluster_id = aps_indication.cluster_id;
        let profile_id = aps_indication.profile_id;
        let src_addr = match aps_indication.src_address {
            ApsAddress::Short(a) => a.0,
            _ => 0,
        };

        if dst_ep == 0x00 {
            // ZDO endpoint — handle ZDP commands
            log::debug!(
                "[Runtime] ZDO frame: cluster=0x{:04X} from 0x{:04X}",
                cluster_id,
                src_addr
            );
            // TODO: dispatch to ZDO handler
            return Some(event_loop::StackEvent::CommandReceived {
                src_addr,
                endpoint: 0,
                cluster_id,
                command_id: 0,
            });
        }

        // Application endpoint — parse ZCL frame
        let zcl_frame = match ZclFrame::parse(aps_indication.payload) {
            Ok(f) => f,
            Err(_) => {
                log::warn!("[Runtime] Failed to parse ZCL frame on ep {}", dst_ep);
                return None;
            }
        };

        let cmd_id = zcl_frame.header.command_id.0;

        // Check if this is a Report Attributes (0x0A) — incoming report from remote
        if zcl_frame.header.frame_type() == zigbee_zcl::frame::ZclFrameType::Global
            && cmd_id == 0x0A
        {
            return Some(event_loop::StackEvent::AttributeReport {
                src_addr,
                endpoint: dst_ep,
                cluster_id,
                attr_id: if aps_indication.payload.len() >= 5 {
                    u16::from_le_bytes([aps_indication.payload[3], aps_indication.payload[4]])
                } else {
                    0
                },
            });
        }

        // Cluster-specific or other global command
        Some(event_loop::StackEvent::CommandReceived {
            src_addr,
            endpoint: dst_ep,
            cluster_id,
            command_id: cmd_id,
        })
    }

    // ── Reporting ───────────────────────────────────────────

    /// Access the reporting engine (e.g., to configure reports).
    pub fn reporting(&self) -> &ReportingEngine {
        &self.reporting
    }

    /// Mutable access to the reporting engine.
    pub fn reporting_mut(&mut self) -> &mut ReportingEngine {
        &mut self.reporting
    }

    // ── Layer access (for advanced use) ─────────────────────

    /// Access the BDB layer.
    pub fn bdb(&self) -> &BdbLayer<M> {
        &self.bdb
    }

    /// Mutable access to the BDB layer.
    pub fn bdb_mut(&mut self) -> &mut BdbLayer<M> {
        &mut self.bdb
    }
}
