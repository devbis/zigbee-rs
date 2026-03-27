//! Finding & Binding commissioning — EZ-Mode (BDB v3.0.1 spec §8.5).
//!
//! Finding & Binding (F&B) automatically creates bindings between
//! compatible endpoints on different devices. It uses the Identify
//! cluster to discover targets and ZDP Simple_Desc / Bind_req to
//! match clusters and install bindings.
//!
//! ## Roles
//!
//! ### Initiator (the device that creates bindings)
//! 1. Enter Finding & Binding mode on a local endpoint
//! 2. Broadcast Identify Query to 0xFFFF
//! 3. For each responding (identifying) target:
//!    a. Get `Simple_Desc` for each active endpoint
//!    b. Match client clusters (our output) ↔ server clusters (their input)
//!    c. Create a binding entry for each matching cluster
//! 4. Exit F&B mode after `bdbcMinCommissioningTime` (180 s)
//!
//! ### Target (the device that gets bound TO)
//! 1. Enter Identify mode (LED blink, etc.) on a local endpoint
//! 2. Respond to Identify Query requests
//! 3. Allow initiator to read Simple_Desc and create bindings
//! 4. Exit Identify mode after timeout
//!
//! ## Cluster matching algorithm
//! A binding is created when the initiator's **output** cluster matches
//! the target's **input** cluster (or vice versa), and both endpoints
//! share the same application profile ID.

use heapless::Vec;
use zigbee_aps::apsde::ApsdeDataRequest;
use zigbee_aps::binding::BindingEntry;
use zigbee_aps::{ApsAddress, ApsAddressMode, ApsTxOptions};
use zigbee_mac::MacDriver;
use zigbee_types::ShortAddress;
use zigbee_zcl::ClusterDirection;
use zigbee_zcl::frame::{ZclFrameHeader, ZclFrameType};
use zigbee_zdo::descriptors::SimpleDescriptor;

use crate::attributes::BDB_MIN_COMMISSIONING_TIME;
use crate::{BdbLayer, BdbStatus};

// ── Identify cluster constants ──────────────────────────────

/// ZCL Identify cluster ID
const CLUSTER_IDENTIFY: u16 = 0x0003;

/// Identify Query command ID (cluster-specific, client → server)
const CMD_IDENTIFY_QUERY: u8 = 0x01;

/// Default F&B window (seconds) — spec says minimum 180 s.
const FB_WINDOW_SECONDS: u16 = BDB_MIN_COMMISSIONING_TIME;

/// Identifies a target device that responded to our Identify Query.
#[derive(Debug, Clone)]
struct IdentifyTarget {
    /// NWK short address of the target
    nwk_addr: ShortAddress,
    /// Active endpoints on this target
    endpoints: Vec<u8, 32>,
}

// ── Initiator ───────────────────────────────────────────────

impl<M: MacDriver> BdbLayer<M> {
    /// Run Finding & Binding as **initiator** on the given local endpoint.
    ///
    /// The initiator discovers targets in Identify mode, reads their
    /// simple descriptors, and creates bindings for matching clusters.
    ///
    /// The procedure runs for up to [`BDB_MIN_COMMISSIONING_TIME`] seconds.
    pub async fn finding_binding_initiator(&mut self, local_endpoint: u8) -> Result<(), BdbStatus> {
        if !self.attributes.node_is_on_a_network {
            return Err(BdbStatus::NotOnNetwork);
        }

        // Verify we have a local simple descriptor for this endpoint
        let local_desc = self
            .zdo
            .get_local_descriptor(local_endpoint)
            .ok_or(BdbStatus::NotPermitted)?
            .clone();

        log::info!(
            "[BDB:F&B] Initiator start on ep {} (profile=0x{:04X}, out_clusters={})",
            local_endpoint,
            local_desc.profile_id,
            local_desc.output_clusters.len(),
        );

        // Step 1: Broadcast Identify Query
        let targets = self.send_identify_query().await?;

        if targets.is_empty() {
            log::info!("[BDB:F&B] No identifying targets found");
            self.attributes.commissioning_status =
                crate::attributes::BdbCommissioningStatus::NoIdentifyQueryResponse;
            return Err(BdbStatus::NoIdentifyResponse);
        }

        log::info!("[BDB:F&B] Found {} identifying target(s)", targets.len());

        let mut any_binding_created = false;

        // Step 2–4: For each target, get simple descriptors and create bindings
        for target in &targets {
            match self.process_target(target, &local_desc).await {
                Ok(count) if count > 0 => {
                    log::info!(
                        "[BDB:F&B] Created {} binding(s) with 0x{:04X}",
                        count,
                        target.nwk_addr.0,
                    );
                    any_binding_created = true;
                }
                Ok(_) => {
                    log::debug!(
                        "[BDB:F&B] No matching clusters with 0x{:04X}",
                        target.nwk_addr.0,
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[BDB:F&B] Failed to process target 0x{:04X}: {:?}",
                        target.nwk_addr.0,
                        e,
                    );
                }
            }
        }

        if any_binding_created {
            self.attributes.commissioning_status =
                crate::attributes::BdbCommissioningStatus::Success;
            Ok(())
        } else {
            self.attributes.commissioning_status =
                crate::attributes::BdbCommissioningStatus::NoIdentifyQueryResponse;
            Err(BdbStatus::NoIdentifyResponse)
        }
    }

    /// Broadcast Identify Query and collect responding targets.
    ///
    /// Builds a real ZCL Identify Query frame and sends it via APS broadcast.
    /// In the current cooperative async model, we send the broadcast and return
    /// an empty list — responses will be collected by the ZCL layer as they
    /// arrive and should be fed back via a target accumulation mechanism.
    async fn send_identify_query(&mut self) -> Result<Vec<IdentifyTarget, 8>, BdbStatus> {
        log::debug!(
            "[BDB:F&B] Broadcasting Identify Query (window={}s)",
            FB_WINDOW_SECONDS,
        );

        // Build ZCL Identify Query frame:
        // Frame control: cluster-specific, client-to-server, disable default response
        let fc = ZclFrameHeader::build_frame_control(
            ZclFrameType::ClusterSpecific,
            false,
            ClusterDirection::ClientToServer,
            true,
        );
        let seq = self.zdo.next_seq();
        // 3-byte ZCL header: FC(1) + SeqNum(1) + CmdId(1), no payload
        let zcl_frame = [fc, seq, CMD_IDENTIFY_QUERY];

        // Send via APSDE-DATA.request as broadcast to all RxOnWhenIdle devices
        let req = ApsdeDataRequest {
            dst_addr_mode: ApsAddressMode::Short,
            dst_address: ApsAddress::Short(ShortAddress(0xFFFD)),
            dst_endpoint: 0xFF, // broadcast to all endpoints
            profile_id: 0x0104, // HA profile
            cluster_id: CLUSTER_IDENTIFY,
            src_endpoint: 0x01, // default endpoint
            payload: &zcl_frame,
            tx_options: ApsTxOptions::default(),
            radius: 0,
            alias_src_addr: None,
            alias_seq: None,
        };

        match self.zdo.aps_mut().apsde_data_request(&req).await {
            Ok(_) => {
                log::debug!("[BDB:F&B] Identify Query broadcast sent");
            }
            Err(e) => {
                log::warn!("[BDB:F&B] Identify Query broadcast failed: {:?}", e);
                return Err(BdbStatus::NotPermitted);
            }
        }

        // Responses arrive asynchronously via the ZCL Identify Query Response
        // handler. In a real implementation, we'd wait for FB_WINDOW_SECONDS
        // collecting targets. For now, return empty — the caller should retry
        // or use an event-driven collection mechanism.
        let _ = FB_WINDOW_SECONDS;
        Ok(Vec::new())
    }

    /// Process a single identifying target: read descriptors, match clusters, bind.
    async fn process_target(
        &mut self,
        target: &IdentifyTarget,
        local_desc: &SimpleDescriptor,
    ) -> Result<usize, BdbStatus> {
        let mut bindings_created = 0;

        for &ep in &target.endpoints {
            // Get the remote simple descriptor
            let remote_desc = match self.zdo.simple_desc_req(target.nwk_addr, ep).await {
                Ok(desc) => desc,
                Err(e) => {
                    log::debug!(
                        "[BDB:F&B] Simple_Desc_req failed for 0x{:04X} ep {}: {:?}",
                        target.nwk_addr.0,
                        ep,
                        e,
                    );
                    continue;
                }
            };

            // Profile must match (or one must be wildcard 0xFFFF)
            if local_desc.profile_id != remote_desc.profile_id
                && local_desc.profile_id != 0xFFFF
                && remote_desc.profile_id != 0xFFFF
            {
                continue;
            }

            // Match clusters and create bindings
            bindings_created += self
                .match_and_bind(local_desc, &remote_desc, target.nwk_addr)
                .await?;
        }

        Ok(bindings_created)
    }

    /// Cluster matching algorithm (BDB spec §8.5).
    ///
    /// Creates bindings where:
    /// - Our **output** cluster matches their **input** cluster
    /// - Our **input** cluster matches their **output** cluster
    async fn match_and_bind(
        &mut self,
        local: &SimpleDescriptor,
        remote: &SimpleDescriptor,
        remote_addr: ShortAddress,
    ) -> Result<usize, BdbStatus> {
        let our_ieee = self.zdo.nwk().nib().ieee_address;
        let remote_ieee = self
            .zdo
            .nwk()
            .find_ieee_by_short(remote_addr)
            .unwrap_or_default();
        let mut count = 0;

        // Our output clusters → their input clusters (client → server binding)
        for &out_cluster in &local.output_clusters {
            if remote.input_clusters.contains(&out_cluster) {
                let entry = BindingEntry::unicast(
                    our_ieee,
                    local.endpoint,
                    out_cluster,
                    remote_ieee,
                    remote.endpoint,
                );
                match self.create_binding(remote_addr, &entry).await {
                    Ok(()) => count += 1,
                    Err(BdbStatus::BindingTableFull) => return Err(BdbStatus::BindingTableFull),
                    Err(_) => {}
                }
            }
        }

        // Our input clusters → their output clusters (server → client binding)
        for &in_cluster in &local.input_clusters {
            if remote.output_clusters.contains(&in_cluster) {
                let entry = BindingEntry::unicast(
                    our_ieee,
                    local.endpoint,
                    in_cluster,
                    remote_ieee,
                    remote.endpoint,
                );
                match self.create_binding(remote_addr, &entry).await {
                    Ok(()) => count += 1,
                    Err(BdbStatus::BindingTableFull) => return Err(BdbStatus::BindingTableFull),
                    Err(_) => {}
                }
            }
        }

        // Group binding (if bdbCommissioningGroupID != 0xFFFF)
        if self.attributes.commissioning_group_id != 0xFFFF {
            for &out_cluster in &local.output_clusters {
                if remote.input_clusters.contains(&out_cluster) {
                    let entry = BindingEntry::group(
                        our_ieee,
                        local.endpoint,
                        out_cluster,
                        self.attributes.commissioning_group_id,
                    );
                    match self.create_binding(remote_addr, &entry).await {
                        Ok(()) => count += 1,
                        Err(BdbStatus::BindingTableFull) => {
                            return Err(BdbStatus::BindingTableFull);
                        }
                        Err(_) => {}
                    }
                }
            }
        }

        Ok(count)
    }

    /// Install a binding in the local APS binding table and send a
    /// ZDP Bind_req to the remote device for bidirectional awareness.
    async fn create_binding(
        &mut self,
        remote_addr: ShortAddress,
        entry: &BindingEntry,
    ) -> Result<(), BdbStatus> {
        // Add to local binding table
        if self
            .zdo
            .aps_mut()
            .binding_table_mut()
            .add(entry.clone())
            .is_err()
            && self.zdo.aps().binding_table().is_full()
        {
            return Err(BdbStatus::BindingTableFull);
        }

        log::debug!(
            "[BDB:F&B] Binding created: ep {} cluster 0x{:04X}",
            entry.src_endpoint,
            entry.cluster_id,
        );

        // Send ZDP Bind_req to remote device (best-effort, don't fail on error)
        if let Err(e) = self.zdo.bind_req(remote_addr, entry).await {
            log::debug!(
                "[BDB:F&B] Remote Bind_req to 0x{:04X} returned {:?} (local binding still valid)",
                remote_addr.0,
                e,
            );
        }

        Ok(())
    }
}

// ── Target ──────────────────────────────────────────────────

impl<M: MacDriver> BdbLayer<M> {
    /// Enter Finding & Binding as **target** on the given local endpoint.
    ///
    /// The target enters Identify mode so that initiators can discover it.
    /// It responds to Identify Query and allows initiators to read its
    /// simple descriptor and create bindings.
    ///
    /// The target stays in Identify mode for [`BDB_MIN_COMMISSIONING_TIME`]
    /// seconds (180 s).
    pub async fn finding_binding_target(&mut self, local_endpoint: u8) -> Result<(), BdbStatus> {
        if !self.attributes.node_is_on_a_network {
            return Err(BdbStatus::NotOnNetwork);
        }

        // Verify we have a local simple descriptor for this endpoint
        if self.zdo.get_local_descriptor(local_endpoint).is_none() {
            return Err(BdbStatus::NotPermitted);
        }

        log::info!(
            "[BDB:F&B] Target mode on ep {} for {}s",
            local_endpoint,
            FB_WINDOW_SECONDS,
        );

        // TODO: Set the local Identify cluster's IdentifyTime attribute
        // to bdbcMinCommissioningTime (180 s). This will:
        // 1. Start the identify effect (LED blink, etc.)
        // 2. Cause the device to respond to Identify Query
        // 3. Automatically stop after the timeout

        // The device's normal APS/ZCL processing handles incoming
        // Simple_Desc_req and Bind_req from the initiator.

        self.attributes.commissioning_status = crate::attributes::BdbCommissioningStatus::Success;
        Ok(())
    }
}
