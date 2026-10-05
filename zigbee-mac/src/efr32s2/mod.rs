//! EFR32MG21 (Series 2) MAC backend — pure Rust, zero vendor blobs.
//!
//! Implements `MacDriver` for the Silicon Labs EFR32MG21 ARM Cortex-M33 SoC.
//! The EFR32MG21 is a Series 2 multi-protocol radio (BLE + IEEE 802.15.4)
//! with Secure Element and TrustZone support.
//!
//! This is a **pure-Rust radio driver**: all radio configuration uses direct
//! register access. No RAIL library, no GSDK binary blobs are linked.
//!
//! # IMPORTANT — Scaffold Implementation
//! The radio register values are simplified approximations. The exact register
//! sequences for 802.15.4 mode need verification against the EFR32xG21 Reference
//! Manual or extraction from the RAIL library source. The driver compiles and
//! has the correct structure, but register values need to be verified before
//! use on real hardware.

pub mod driver;

use crate::frames;
use crate::pib::{PibAttribute, PibPayload, PibValue};
use crate::primitives::*;
use crate::{MacCapabilities, MacDriver, MacError, PlatformServices};
use driver::{Efr32s2Driver, RadioConfig, RadioError};
use zigbee_types::*;

use embassy_futures::select;
use embassy_time::{Instant, Timer};

/// Maximum MAC payload size (127 - MAC overhead).
const MAX_MAC_PAYLOAD: usize = 102;

/// Frames received while waiting for an ACK, held for normal RX processing.
const PENDING_RX_CAPACITY: usize = 2;

/// macAckWaitDuration upper bound used by this backend (µs).
const ACK_WAIT_US: u64 = 1500;

/// EFR32MG21 IEEE 802.15.4 MAC driver.
///
/// Pure-Rust implementation using direct register access — no RAIL FFI.
pub struct Efr32s2Mac {
    driver: Efr32s2Driver,
    short_address: ShortAddress,
    pan_id: PanId,
    channel: u8,
    extended_address: IeeeAddress,
    coord_short_address: ShortAddress,
    rx_on_when_idle: bool,
    association_permit: bool,
    auto_request: bool,
    dsn: u8,
    bsn: u8,
    beacon_payload: PibPayload,
    max_csma_backoffs: u8,
    min_be: u8,
    max_be: u8,
    max_frame_retries: u8,
    promiscuous: bool,
    /// Buffer for frames received during association (e.g. Transport-Key).
    /// Returned by the next mlme_poll() call.
    pending_assoc_frame: Option<([u8; 128], usize)>,
    /// Frames accepted by the destination filter while an ACK wait was in
    /// progress. Drained by `mcps_data_indication_timeout` and `mlme_poll`.
    pending_rx: heapless::Deque<driver::RxFrame, PENDING_RX_CAPACITY>,
    /// CSMA-CA backoff PRNG state (xorshift32). Zero means "not yet seeded";
    /// it is seeded lazily from the EUI-64 and the monotonic clock once the
    /// time driver is running.
    rng_state: u32,
}

impl Efr32s2Mac {
    pub fn new() -> Self {
        let config = RadioConfig::default();
        let ieee = Self::read_factory_ieee();
        log::info!(
            "[MAC] IEEE: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            ieee[0],
            ieee[1],
            ieee[2],
            ieee[3],
            ieee[4],
            ieee[5],
            ieee[6],
            ieee[7]
        );
        Self {
            driver: Efr32s2Driver::new(config),
            short_address: ShortAddress(0xFFFF),
            pan_id: PanId(0xFFFF),
            channel: 11,
            extended_address: ieee,
            coord_short_address: ShortAddress(0xFFFF),
            rx_on_when_idle: false,
            association_permit: false,
            auto_request: true,
            dsn: 0,
            bsn: 0,
            beacon_payload: PibPayload::new(),
            max_csma_backoffs: 4,
            min_be: 3,
            max_be: 5,
            max_frame_retries: 3,
            promiscuous: false,
            pending_assoc_frame: None,
            pending_rx: heapless::Deque::new(),
            rng_state: 0,
        }
    }

    /// Read the factory-programmed IEEE 802.15.4 EUI-64 address.
    ///
    /// EFR32MG21 stores a unique 64-bit EUI in the Device Information (DI) page
    /// at address 0x0FE0_81A0. This is programmed at the factory and cannot be
    /// changed.
    fn read_factory_ieee() -> [u8; 8] {
        // EFR32MG21 Device Information page — EUI64
        const DI_EUI64_ADDR: u32 = 0x0FE0_81A0; // TODO: verify against EFR32xG21 RM

        let mut eui64 = [0u8; 8];
        let lo = unsafe { core::ptr::read_volatile(DI_EUI64_ADDR as *const u32) };
        let hi = unsafe { core::ptr::read_volatile((DI_EUI64_ADDR + 4) as *const u32) };

        eui64[0] = (lo >> 0) as u8;
        eui64[1] = (lo >> 8) as u8;
        eui64[2] = (lo >> 16) as u8;
        eui64[3] = (lo >> 24) as u8;
        eui64[4] = (hi >> 0) as u8;
        eui64[5] = (hi >> 8) as u8;
        eui64[6] = (hi >> 16) as u8;
        eui64[7] = (hi >> 24) as u8;

        // Validate — if all-zeros or all-ones, use fallback
        if eui64 == [0xFF; 8] || eui64 == [0x00; 8] {
            log::warn!("efr32s2: no factory EUI64 — using fallback");
            return [0x00, 0x0D, 0x6F, 0xFF, 0xFE, 0xDE, 0xAD, 0x02];
        }

        eui64
    }

    fn next_dsn(&mut self) -> u8 {
        let s = self.dsn;
        self.dsn = self.dsn.wrapping_add(1);
        s
    }

    fn next_bsn(&mut self) -> u8 {
        let s = self.bsn;
        self.bsn = self.bsn.wrapping_add(1);
        s
    }

    /// Power down the radio to save battery between poll cycles.
    /// Saves ~5–10 mA. Call `radio_wake()` before next TX/RX.
    pub fn radio_sleep(&self) {
        self.driver.radio_sleep();
    }

    /// Re-enable the radio after `radio_sleep()`.
    pub fn radio_wake(&mut self) {
        self.driver.radio_wake();
    }

    fn map_radio_err(e: RadioError) -> MacError {
        match e {
            RadioError::CcaFailure => MacError::ChannelAccessFailure,
            RadioError::HardwareError => MacError::RadioError,
            RadioError::InvalidFrame => MacError::FrameTooLong,
            RadioError::CrcError => MacError::RadioError,
            RadioError::NotInitialized => MacError::RadioError,
            RadioError::RxTimeout => MacError::NoData,
        }
    }

    /// Draw a CSMA-CA backoff count in `0..2^be`.
    ///
    /// The xorshift32 state is device-unique (seeded from the EUI-64) and
    /// is re-mixed with the monotonic clock on every draw, so that radio
    /// timing jitter decorrelates nodes that would otherwise collide in
    /// lockstep.
    fn random_backoff(&mut self, be: u8) -> u32 {
        let now = Instant::now().as_ticks();
        if self.rng_state == 0 {
            self.rng_state = csma_seed(&self.extended_address);
        }
        self.rng_state = xorshift32(self.rng_state ^ (now as u32) ^ ((now >> 32) as u32));
        backoff_slots(self.rng_state, be)
    }

    /// Unslotted CSMA-CA + TX + ACK wait + retries.
    ///
    /// Implements IEEE 802.15.4-2011 §5.1.1.4 (unslotted CSMA-CA) with
    /// optional ACK reception and retry loop per `macMaxFrameRetries`.
    /// Returns the ACK Frame Pending bit when an ACK was requested.
    async fn csma_ca_transmit(
        &mut self,
        frame: &[u8],
        ack_requested: bool,
    ) -> Result<bool, MacError> {
        if frame.len() < 3 {
            return Err(MacError::InvalidParameter);
        }
        let max_retries = if ack_requested {
            self.max_frame_retries
        } else {
            0
        };
        const SYMBOL_PERIOD_US: u64 = 16; // 62.5 ksym/s
        const UNIT_BACKOFF_SYMBOLS: u64 = 20; // aUnitBackoffPeriod

        for attempt in 0..=max_retries {
            // ── Unslotted CSMA-CA ──
            let mut nb: u8 = 0;
            let mut be = self.min_be.min(self.max_be);

            let channel_clear = loop {
                let backoff = self.random_backoff(be) as u64;
                let delay_us = backoff * UNIT_BACKOFF_SYMBOLS * SYMBOL_PERIOD_US;
                if delay_us > 0 {
                    Timer::after_micros(delay_us).await;
                }

                let busy = self
                    .driver
                    .clear_channel_assessment()
                    .await
                    .map_err(Self::map_radio_err)?;

                if !busy {
                    break true;
                }

                nb = nb.saturating_add(1);
                be = core::cmp::min(be.saturating_add(1), self.max_be);
                if nb > self.max_csma_backoffs {
                    break false;
                }
            };

            if !channel_clear {
                if attempt == max_retries {
                    return Err(MacError::ChannelAccessFailure);
                }
                continue;
            }

            // ── TX ──
            self.driver
                .transmit(frame)
                .await
                .map_err(Self::map_radio_err)?;

            if !ack_requested {
                return Ok(false);
            }

            // ── ACK wait ──
            // Keep listening until the deadline: a foreign ACK or an
            // unrelated frame must not end the wait. Frames addressed to us
            // are acknowledged and queued for normal RX processing.
            let seq = frame[2];
            if let Some(frame_pending) = self.wait_for_ack(seq).await {
                return Ok(frame_pending);
            }

            if attempt == max_retries {
                return Err(MacError::NoAck);
            }
            log::debug!(
                "efr32s2: no ACK for seq={}, retry {}/{}",
                seq,
                attempt + 1,
                max_retries
            );
        }

        Err(MacError::NoAck)
    }

    /// Wait up to `ACK_WAIT_US` for the ACK matching `seq`.
    ///
    /// Returns the ACK frame-pending bit on a match. Non-ACK frames that pass
    /// the destination filter are acknowledged (when requested) and queued.
    async fn wait_for_ack(&mut self, seq: u8) -> Option<bool> {
        let deadline = Instant::now() + embassy_time::Duration::from_micros(ACK_WAIT_US);
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let rx = match select::select(self.driver.receive(), Timer::after(deadline - now)).await
            {
                select::Either::First(Ok(rx)) => rx,
                select::Either::First(Err(_)) => continue,
                select::Either::Second(_) => return None,
            };
            let data = &rx.data[..rx.len];
            if frames::ack_info(data).is_some() {
                if let Some(frame_pending) = matching_ack(data, seq) {
                    return Some(frame_pending);
                }
                continue;
            }
            let disposition = self.classify(data);
            if disposition.ack {
                self.send_ack(data[2]).await;
            }
            if disposition.accept {
                if self.pending_rx.is_full() {
                    let _ = self.pending_rx.pop_front();
                }
                let _ = self.pending_rx.push_back(rx);
            }
        }
    }

    /// Receive the next frame, draining frames queued during ACK waits first.
    ///
    /// Queued frames were already acknowledged when they were captured.
    async fn next_rx(&mut self, remaining: embassy_time::Duration) -> NextRx {
        if let Some(rx) = self.pending_rx.pop_front() {
            return NextRx::Frame(rx, true);
        }
        match select::select(self.driver.receive(), Timer::after(remaining)).await {
            select::Either::First(Ok(rx)) => NextRx::Frame(rx, false),
            select::Either::First(Err(_)) => NextRx::Error,
            select::Either::Second(_) => NextRx::Timeout,
        }
    }

    /// Hold a data frame received around association (e.g. Transport-Key)
    /// for the next `mlme_poll()`. Returns `true` when a payload was saved.
    fn save_assoc_frame(&mut self, data: &[u8]) -> bool {
        let Some((_, _, frame, _)) = data_payload(data) else {
            return false;
        };
        let payload = frame.as_slice();
        let mut buf = [0u8; 128];
        buf[..payload.len()].copy_from_slice(payload);
        self.pending_assoc_frame = Some((buf, payload.len()));
        log::info!("efr32s2: saved post-assoc frame ({} bytes)", payload.len());
        true
    }

    fn classify(&self, data: &[u8]) -> RxDisposition {
        classify_rx(
            data,
            self.promiscuous,
            self.pan_id,
            self.short_address,
            &self.extended_address,
        )
    }

    /// Send a 3-byte IEEE 802.15.4 ACK frame for the given sequence number.
    async fn send_ack(&mut self, seq: u8) {
        let ack = [0x02u8, 0x00, seq];
        let _ = self.driver.transmit(&ack).await;
    }
}

impl MacDriver for Efr32s2Mac {
    async fn mlme_scan(&mut self, req: MlmeScanRequest) -> Result<MlmeScanConfirm, MacError> {
        let mut pan_descriptors: PanDescriptorList = heapless::Vec::new();
        let mut energy_list: EdList = heapless::Vec::new();

        let scan_duration_ms = ((1u64 << req.scan_duration as u64) * 15360 / 1000) + 1;

        for ch in 11u8..=26 {
            if req.channel_mask.0 & (1u32 << ch) == 0 {
                continue;
            }

            self.driver.update_config(|c| c.channel = ch);

            match req.scan_type {
                ScanType::Ed => {
                    let (rssi, _busy) = self.driver.energy_detect().map_err(Self::map_radio_err)?;
                    let ed = ((rssi as i16 + 100).clamp(0, 255)) as u8;
                    let _ = energy_list.push(EdValue {
                        channel: ch,
                        energy: ed,
                    });
                }
                ScanType::Active => {
                    let seq = self.next_bsn();
                    let beacon_req = frames::build_beacon_request(seq);
                    let _ = self.driver.transmit(&beacon_req).await;

                    let deadline = embassy_time::Instant::now()
                        + embassy_time::Duration::from_millis(scan_duration_ms);
                    while !pan_descriptors.is_full() {
                        let now = embassy_time::Instant::now();
                        if now >= deadline {
                            break;
                        }
                        let remaining = deadline - now;
                        let result =
                            select::select(self.driver.receive(), Timer::after(remaining)).await;

                        if let select::Either::First(Ok(frame)) = result {
                            if let Some(pd) =
                                frames::parse_beacon(ch, &frame.data[..frame.len], frame.lqi)
                            {
                                let _ = pan_descriptors.push(pd);
                            }
                        } else {
                            break;
                        }
                    }
                }
                ScanType::Passive => {
                    let deadline = embassy_time::Instant::now()
                        + embassy_time::Duration::from_millis(scan_duration_ms);
                    while !pan_descriptors.is_full() {
                        let now = embassy_time::Instant::now();
                        if now >= deadline {
                            break;
                        }
                        let remaining = deadline - now;
                        let result =
                            select::select(self.driver.receive(), Timer::after(remaining)).await;

                        if let select::Either::First(Ok(frame)) = result {
                            if let Some(pd) =
                                frames::parse_beacon(ch, &frame.data[..frame.len], frame.lqi)
                            {
                                let _ = pan_descriptors.push(pd);
                            }
                        } else {
                            break;
                        }
                    }
                }
                ScanType::Orphan => {}
            }
        }

        self.driver.update_config(|c| c.channel = self.channel);

        Ok(MlmeScanConfirm {
            scan_type: req.scan_type,
            pan_descriptors,
            energy_list,
        })
    }

    async fn mlme_associate(
        &mut self,
        req: MlmeAssociateRequest,
    ) -> Result<MlmeAssociateConfirm, MacError> {
        self.channel = req.channel;
        self.driver.update_config(|c| c.channel = req.channel);

        let seq = self.next_dsn();
        let frame = frames::build_association_request(
            seq,
            &req.coord_address,
            &self.extended_address,
            &req.capability_info,
        );

        self.csma_ca_transmit(&frame, true).await?;

        Timer::after(embassy_time::Duration::from_millis(200)).await;

        let mut confirm: Option<MlmeAssociateConfirm> = None;

        for poll_attempt in 0..5u8 {
            if poll_attempt > 0 {
                Timer::after(embassy_time::Duration::from_millis(500)).await;
            }

            let data_req = frames::build_data_request(
                self.next_dsn(),
                &req.coord_address,
                &self.extended_address,
            );
            let _ = self.csma_ca_transmit(&data_req, true).await;

            let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_millis(1500);

            for _ in 0..20u8 {
                let now = embassy_time::Instant::now();
                if now >= deadline {
                    break;
                }
                let (rx, acked) = match self.next_rx(deadline - now).await {
                    NextRx::Timeout => break,
                    NextRx::Error => continue,
                    NextRx::Frame(rx, acked) => (rx, acked),
                };
                let data = &rx.data[..rx.len];
                let disposition = self.classify(data);
                if !disposition.accept {
                    continue;
                }
                if disposition.ack && !acked {
                    self.send_ack(data[2]).await;
                }

                match data[0] & 0x07 {
                    0x03 => {
                        if let Some((addr, status_byte)) = frames::parse_association_response(data)
                        {
                            let status = match status_byte {
                                0x00 => AssociationStatus::Success,
                                0x01 => AssociationStatus::PanAtCapacity,
                                _ => AssociationStatus::PanAccessDenied,
                            };
                            if status_byte == 0 {
                                self.short_address = addr;
                                self.driver.update_config(|c| c.short_address = addr.0);
                            }
                            confirm = Some(MlmeAssociateConfirm {
                                short_address: addr,
                                status,
                            });
                            break;
                        }
                    }
                    0x01 if self.pending_assoc_frame.is_none() => {
                        self.save_assoc_frame(data);
                    }
                    _ => {}
                }
            }

            if confirm.is_some() {
                break;
            }
        }

        // Listen briefly for Transport-Key after association
        if confirm.is_some() && self.pending_assoc_frame.is_none() {
            let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_millis(2000);
            for _ in 0..20u8 {
                let now = embassy_time::Instant::now();
                if now >= deadline {
                    break;
                }
                let (rx, acked) = match self.next_rx(deadline - now).await {
                    NextRx::Timeout => break,
                    NextRx::Error => continue,
                    NextRx::Frame(rx, acked) => (rx, acked),
                };
                let data = &rx.data[..rx.len];
                let disposition = self.classify(data);
                if !disposition.accept {
                    continue;
                }
                if disposition.ack && !acked {
                    self.send_ack(data[2]).await;
                }
                if data[0] & 0x07 == 0x01 && self.save_assoc_frame(data) {
                    break;
                }
            }
        }

        confirm.ok_or(MacError::NoBeacon)
    }

    async fn mlme_associate_response(
        &mut self,
        _rsp: MlmeAssociateResponse,
    ) -> Result<(), MacError> {
        Err(MacError::Unsupported)
    }

    async fn mlme_disassociate(&mut self, _req: MlmeDisassociateRequest) -> Result<(), MacError> {
        self.short_address = ShortAddress(0xFFFF);
        self.pan_id = PanId(0xFFFF);
        self.driver.update_config(|c| {
            c.short_address = 0xFFFF;
            c.pan_id = 0xFFFF;
        });
        Ok(())
    }

    fn mlme_reset(&mut self, set_default_pib: bool) -> Result<(), MacError> {
        if set_default_pib {
            self.short_address = ShortAddress(0xFFFF);
            self.pan_id = PanId(0xFFFF);
            self.channel = 11;
            self.rx_on_when_idle = false;
            self.dsn = 0;
            self.bsn = 0;
        }
        self.driver.update_config(|c| {
            c.channel = self.channel;
            c.short_address = self.short_address.0;
            c.pan_id = self.pan_id.0;
        });
        Ok(())
    }

    async fn mlme_start(&mut self, req: MlmeStartRequest) -> Result<(), MacError> {
        // `Efr32s2Mac` does not implement `ParentMacDriver`: it has no
        // frame-pending-aware ACK path and keeps every parent primitive at
        // its `Unsupported` default. Fail explicitly.
        start_requires_parent_capability(&req)
    }

    async fn mlme_get(&self, attr: PibAttribute) -> Result<PibValue, MacError> {
        use PibAttribute::*;
        Ok(match attr {
            MacShortAddress => PibValue::ShortAddress(self.short_address),
            MacPanId => PibValue::PanId(self.pan_id),
            PhyCurrentChannel => PibValue::U8(self.channel),
            MacExtendedAddress => PibValue::ExtendedAddress(self.extended_address),
            MacCoordShortAddress => PibValue::ShortAddress(self.coord_short_address),
            MacRxOnWhenIdle => PibValue::Bool(self.rx_on_when_idle),
            MacAssociationPermit => PibValue::Bool(self.association_permit),
            MacAutoRequest => PibValue::Bool(self.auto_request),
            MacBeaconPayload => PibValue::Payload(self.beacon_payload.clone()),
            MacMaxCsmaBackoffs => PibValue::U8(self.max_csma_backoffs),
            MacMinBe => PibValue::U8(self.min_be),
            MacMaxBe => PibValue::U8(self.max_be),
            MacMaxFrameRetries => PibValue::U8(self.max_frame_retries),
            MacPromiscuousMode => PibValue::Bool(self.promiscuous),
            MacDsn => PibValue::U8(self.dsn),
            MacBsn => PibValue::U8(self.bsn),
            PhyTransmitPower => PibValue::U8(self.driver.config().tx_power as u8),
            PhyChannelsSupported => PibValue::U32(ChannelMask::ALL_2_4GHZ.0),
            PhyCurrentPage => PibValue::U8(0),
            _ => return Err(MacError::InvalidParameter),
        })
    }

    async fn mlme_set(&mut self, attr: PibAttribute, value: PibValue) -> Result<(), MacError> {
        use PibAttribute::*;
        match (attr, value) {
            (MacShortAddress, PibValue::ShortAddress(v)) => {
                self.short_address = v;
                self.driver.update_config(|c| c.short_address = v.0);
            }
            (MacPanId, PibValue::PanId(v)) => {
                self.pan_id = v;
                self.driver.update_config(|c| c.pan_id = v.0);
            }
            (PhyCurrentChannel, PibValue::U8(v)) => {
                self.channel = v;
                self.driver.update_config(|c| c.channel = v);
            }
            (MacExtendedAddress, PibValue::ExtendedAddress(v)) => {
                self.extended_address = v;
                self.driver.update_config(|c| c.extended_address = v);
            }
            (MacCoordShortAddress, PibValue::ShortAddress(v)) => {
                self.coord_short_address = v;
            }
            (MacRxOnWhenIdle, PibValue::Bool(v)) => {
                self.rx_on_when_idle = v;
            }
            (MacAssociationPermit, PibValue::Bool(v)) => {
                self.association_permit = v;
            }
            (MacAutoRequest, PibValue::Bool(v)) => {
                self.auto_request = v;
            }
            (MacBeaconPayload, PibValue::Payload(v)) => {
                self.beacon_payload = v;
            }
            (MacMaxCsmaBackoffs, PibValue::U8(v)) => {
                self.max_csma_backoffs = v;
            }
            (MacMinBe, PibValue::U8(v)) => {
                self.min_be = v;
            }
            (MacMaxBe, PibValue::U8(v)) => {
                self.max_be = v;
            }
            (MacMaxFrameRetries, PibValue::U8(v)) => {
                self.max_frame_retries = v;
            }
            (MacPromiscuousMode, PibValue::Bool(v)) => {
                self.promiscuous = v;
                self.driver.update_config(|c| c.promiscuous = v);
            }
            (MacDsn, PibValue::U8(v)) => {
                self.dsn = v;
            }
            (MacBsn, PibValue::U8(v)) => {
                self.bsn = v;
            }
            (PhyTransmitPower, PibValue::U8(v)) => {
                self.driver.update_config(|c| c.tx_power = v as i8);
            }
            _ => return Err(MacError::InvalidParameter),
        }
        Ok(())
    }

    async fn mlme_poll(&mut self) -> Result<Option<MacFrame>, MacError> {
        if let Some((buf, len)) = self.pending_assoc_frame.take() {
            log::info!(
                "[MAC:Poll] Returning saved association frame ({} bytes)",
                len
            );
            return Ok(MacFrame::from_slice(&buf[..len]));
        }

        // Frames captured during earlier ACK waits are delivered first.
        while let Some(rx) = self.pending_rx.pop_front() {
            if let Some((_, _, frame, _)) = data_payload(&rx.data[..rx.len]) {
                return Ok(Some(frame));
            }
        }

        let parent = MacAddress::Short(self.pan_id, self.coord_short_address);
        let has_short = self.short_address.0 != 0xFFFF && self.short_address.0 != 0xFFFE;

        let passes: u8 = if has_short { 2 } else { 1 };

        // An ACKed-but-empty poll is `Ok(None)`; a poll whose Data Requests
        // all went unacknowledged surfaces the last error so the runtime can
        // drive parent-loss recovery.
        let mut any_ack = false;
        let mut last_err: Option<MacError> = None;

        for pass in 0..passes {
            let data_req = if pass == 0 && has_short {
                frames::build_data_request_short(self.next_dsn(), &parent, self.short_address)
            } else {
                frames::build_data_request(self.next_dsn(), &parent, &self.extended_address)
            };

            // csma_ca_transmit only accepts the ACK whose sequence number
            // matches this Data Request and reports its frame-pending bit.
            let frame_pending = match self.csma_ca_transmit(&data_req, true).await {
                Ok(frame_pending) => {
                    any_ack = true;
                    frame_pending
                }
                Err(error) => {
                    last_err = Some(error);
                    continue;
                }
            };
            if !frame_pending {
                return Ok(None);
            }

            let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_millis(1500);

            for _rx_attempt in 0..40u8 {
                let now = embassy_time::Instant::now();
                if now >= deadline {
                    break;
                }
                let (rx, acked) = match self.next_rx(deadline - now).await {
                    NextRx::Timeout => break,
                    NextRx::Error => continue,
                    NextRx::Frame(rx, acked) => (rx, acked),
                };
                let data = &rx.data[..rx.len];
                let disposition = self.classify(data);
                if !disposition.accept {
                    continue;
                }
                if disposition.ack && !acked {
                    self.send_ack(data[2]).await;
                }
                if let Some((_, _, frame, _)) = data_payload(data) {
                    return Ok(Some(frame));
                }
            }
        }

        if any_ack {
            Ok(None)
        } else {
            Err(last_err.unwrap_or(MacError::NoAck))
        }
    }

    async fn mcps_data(&mut self, req: McpsDataRequest<'_>) -> Result<McpsDataConfirm, MacError> {
        if req.payload.len() > MAX_MAC_PAYLOAD {
            return Err(MacError::FrameTooLong);
        }

        let msdu_handle = req.msdu_handle;
        let ack_requested = req.tx_options.ack_tx;

        let seq = self.next_dsn();
        let mac_frame = frames::build_data_frame(
            seq,
            req.src_addr_mode,
            self.short_address,
            &self.extended_address,
            &req.dst_address,
            req.payload,
            ack_requested,
            req.tx_options.frame_pending,
        )
        .map_err(|_| MacError::FrameTooLong)?;

        self.csma_ca_transmit(&mac_frame, ack_requested).await?;

        Ok(McpsDataConfirm {
            msdu_handle,
            timestamp: None,
        })
    }

    async fn mcps_data_indication(&mut self) -> Result<McpsDataIndication, MacError> {
        self.mcps_data_indication_timeout(5_000_000).await
    }

    async fn mcps_data_indication_timeout(
        &mut self,
        timeout_us: u32,
    ) -> Result<McpsDataIndication, MacError> {
        let deadline =
            embassy_time::Instant::now() + embassy_time::Duration::from_micros(timeout_us as u64);

        loop {
            let now = embassy_time::Instant::now();
            if now >= deadline && self.pending_rx.is_empty() {
                return Err(MacError::NoData);
            }
            let (rx, acked) = match self.next_rx(deadline.saturating_duration_since(now)).await {
                NextRx::Timeout => return Err(MacError::NoData),
                NextRx::Error => continue,
                NextRx::Frame(rx, acked) => (rx, acked),
            };
            let data = &rx.data[..rx.len];
            let disposition = self.classify(data);
            if !disposition.accept {
                continue;
            }
            // Acknowledge only after the destination filter accepted the
            // frame; broadcasts and frames for other nodes are never ACKed.
            if disposition.ack && !acked {
                self.send_ack(data[2]).await;
            }
            let Some((src_address, dst_address, payload, security_use)) = data_payload(data) else {
                continue;
            };

            log::trace!("efr32s2: rx {} bytes lqi={}", rx.len, rx.lqi);

            return Ok(McpsDataIndication {
                src_address,
                dst_address,
                lqi: rx.lqi,
                payload,
                security_use,
            });
        }
    }

    fn capabilities(&self) -> MacCapabilities {
        MacCapabilities::non_parent(102, TxPower(-20), TxPower(19))
    }
}

impl zigbee_crypto::ForwardAesProvider for Efr32s2Mac {}
impl PlatformServices for Efr32s2Mac {
    fn monotonic_micros(&self) -> u32 {
        Instant::now().as_micros() as u32
    }

    async fn delay_micros(&mut self, duration_us: u32) {
        Timer::after_micros(duration_us as u64).await;
    }

    fn fill_random(&mut self, _output: &mut [u8]) -> Result<(), MacError> {
        Err(MacError::Unsupported)
    }
}

// ── RX helpers ──────────────────────────────────────────────────

/// Result of [`Efr32s2Mac::next_rx`].
enum NextRx {
    /// A frame and whether it was already acknowledged when it was queued.
    Frame(driver::RxFrame, bool),
    /// The radio reported a receive error (e.g. CRC failure).
    Error,
    /// The receive window expired.
    Timeout,
}

/// Destination-filter decision for a received frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RxDisposition {
    /// Hand the frame to the MAC/NWK receive path.
    accept: bool,
    /// Transmit a software ACK. Only set for unicast frames addressed to this
    /// node with the ACK Request bit, so the MAC never acknowledges frames
    /// meant for other nodes or broadcasts.
    ack: bool,
}

/// Third-level IEEE 802.15.4 filtering for a received (non-ACK) frame.
///
/// Frames shorter than their addressing fields, ACK frames, and frames
/// without a destination address (only meaningful to a PAN coordinator,
/// which this backend never is) are rejected unless promiscuous.
fn classify_rx(
    data: &[u8],
    promiscuous: bool,
    our_pan: PanId,
    our_short: ShortAddress,
    our_extended: &IeeeAddress,
) -> RxDisposition {
    const REJECT: RxDisposition = RxDisposition {
        accept: false,
        ack: false,
    };
    if data.len() < 3 {
        return REJECT;
    }
    let fc = u16::from_le_bytes([data[0], data[1]]);
    if fc & 0x07 == 0x02 || data.len() < 3 + frames::addressing_size(fc) {
        return REJECT;
    }
    let dst = match (fc >> 10) & 0x03 {
        0x02 | 0x03 => frames::parse_mac_addresses(data).1,
        _ => {
            return RxDisposition {
                accept: promiscuous,
                ack: false,
            };
        }
    };
    let for_us = frames::frame_is_for_us(&dst, our_pan, our_short, our_extended);
    let unicast = !matches!(dst, MacAddress::Short(_, ShortAddress(0xFFFF)));
    RxDisposition {
        accept: promiscuous || for_us,
        ack: for_us && unicast && fc & (1 << 5) != 0,
    }
}

/// Frame-pending bit of an ACK whose sequence number matches `seq`.
///
/// ACKs carry no addresses; the sequence number is the only way to tell our
/// acknowledgement from one exchanged by other nodes on the channel.
fn matching_ack(data: &[u8], seq: u8) -> Option<bool> {
    frames::ack_info(data)
        .filter(|(ack_seq, _)| *ack_seq == seq)
        .map(|(_, frame_pending)| frame_pending)
}

/// Extract addresses and payload from a MAC data frame.
fn data_payload(data: &[u8]) -> Option<(MacAddress, MacAddress, MacFrame, bool)> {
    if data.len() < 3 || data[0] & 0x07 != 0x01 {
        return None;
    }
    let (src, dst, payload_offset, security_use) = frames::parse_mac_addresses(data);
    let payload = data.get(payload_offset..).filter(|p| !p.is_empty())?;
    Some((src, dst, MacFrame::from_slice(payload)?, security_use))
}

// ── CSMA-CA randomness ──────────────────────────────────────────

/// Device-unique non-zero xorshift32 seed derived from the EUI-64.
fn csma_seed(eui64: &IeeeAddress) -> u32 {
    let seed = u32::from_le_bytes([eui64[0], eui64[1], eui64[2], eui64[3]])
        ^ u32::from_le_bytes([eui64[4], eui64[5], eui64[6], eui64[7]]).rotate_left(16)
        ^ 0x9E37_79B9;
    seed.max(1)
}

fn xorshift32(mut value: u32) -> u32 {
    if value == 0 {
        value = 0x9E37_79B9;
    }
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    value
}

/// Map a random word onto `0..2^be` backoff periods (macMaxBE ≤ 8).
fn backoff_slots(random: u32, be: u8) -> u32 {
    random & ((1u32 << be.min(8)) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUR_PAN: PanId = PanId(0x1A62);
    const OUR_SHORT: ShortAddress = ShortAddress(0x3344);
    const OUR_EXT: IeeeAddress = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    const OTHER_EXT: IeeeAddress = [0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8];

    fn data_frame(dst: MacAddress, ack: bool) -> heapless::Vec<u8, 125> {
        frames::build_data_frame(
            7,
            AddressMode::Short,
            ShortAddress(0x0000),
            &OTHER_EXT,
            &dst,
            &[0xAA, 0xBB],
            ack,
            false,
        )
        .unwrap()
    }

    fn classify(data: &[u8]) -> RxDisposition {
        classify_rx(data, false, OUR_PAN, OUR_SHORT, &OUR_EXT)
    }

    #[test]
    fn acks_only_unicast_frames_addressed_to_us() {
        let ours = data_frame(MacAddress::Short(OUR_PAN, OUR_SHORT), true);
        assert_eq!(
            classify(&ours),
            RxDisposition {
                accept: true,
                ack: true
            }
        );

        let ours_no_ar = data_frame(MacAddress::Short(OUR_PAN, OUR_SHORT), false);
        assert_eq!(
            classify(&ours_no_ar),
            RxDisposition {
                accept: true,
                ack: false
            }
        );

        let other_node = data_frame(MacAddress::Short(OUR_PAN, ShortAddress(0x0001)), true);
        assert_eq!(
            classify(&other_node),
            RxDisposition {
                accept: false,
                ack: false
            }
        );

        let other_pan = data_frame(MacAddress::Short(PanId(0x2222), OUR_SHORT), true);
        assert!(!classify(&other_pan).accept);
        assert!(!classify(&other_pan).ack);

        let other_ext = data_frame(MacAddress::Extended(OUR_PAN, OTHER_EXT), true);
        assert!(!classify(&other_ext).ack);
    }

    #[test]
    fn never_acks_broadcasts_or_nwk_broadcast_values() {
        // A malformed broadcast with AR set must still not be acknowledged.
        let broadcast = data_frame(MacAddress::Short(OUR_PAN, ShortAddress(0xFFFF)), true);
        assert_eq!(
            classify(&broadcast),
            RxDisposition {
                accept: true,
                ack: false
            }
        );
        // 0xFFFD/0xFFFC are NWK broadcast values, never MAC destinations.
        for nwk_bcast in [0xFFFD, 0xFFFC] {
            let frame = data_frame(MacAddress::Short(OUR_PAN, ShortAddress(nwk_bcast)), true);
            assert!(!classify(&frame).accept);
            assert!(!classify(&frame).ack);
        }
    }

    #[test]
    fn association_response_to_our_ieee_is_acked_while_unassociated() {
        // Unassociated: PAN and short address still 0xFFFF.
        let mut rsp = heapless::Vec::<u8, 32>::new();
        rsp.extend_from_slice(&[0x63, 0xCC, 0x05, 0x62, 0x1A])
            .unwrap();
        rsp.extend_from_slice(&OUR_EXT).unwrap();
        rsp.extend_from_slice(&OTHER_EXT).unwrap();
        rsp.extend_from_slice(&[0x02, 0x44, 0x33, 0x00]).unwrap();
        let d = classify_rx(&rsp, false, PanId(0xFFFF), ShortAddress(0xFFFF), &OUR_EXT);
        assert_eq!(
            d,
            RxDisposition {
                accept: true,
                ack: true
            }
        );
        assert_eq!(
            frames::parse_association_response(&rsp),
            Some((OUR_SHORT, 0x00))
        );

        // The same response for another joiner is neither accepted nor ACKed.
        let d = classify_rx(&rsp, false, PanId(0xFFFF), ShortAddress(0xFFFF), &OTHER_EXT);
        assert!(!d.accept && !d.ack);
    }

    #[test]
    fn truncated_association_response_is_rejected_without_panic() {
        // dst ext + src ext + PAN compression: command id at offset 21.
        let mut rsp = heapless::Vec::<u8, 32>::new();
        rsp.extend_from_slice(&[0x63, 0xCC, 0x05, 0x62, 0x1A])
            .unwrap();
        rsp.extend_from_slice(&OUR_EXT).unwrap();
        rsp.extend_from_slice(&OTHER_EXT).unwrap();
        rsp.extend_from_slice(&[0x02, 0x44, 0x33]).unwrap(); // status byte missing
        for len in 0..=rsp.len() {
            assert_eq!(frames::parse_association_response(&rsp[..len]), None);
            let _ = classify(&rsp[..len]);
            let _ = data_payload(&rsp[..len]);
        }
    }

    #[test]
    fn rejects_acks_truncated_and_destinationless_frames() {
        assert!(!classify(&[0x02, 0x00, 0x07]).accept);
        // FCF claims a short destination but the address bytes are missing.
        assert!(!classify(&[0x61, 0x88, 0x07, 0x62]).accept);
        // No destination address (only meaningful to a PAN coordinator).
        let no_dst = [0x41, 0x80, 0x07, 0x62, 0x1A, 0x00, 0x00, 0xAA];
        assert!(!classify(&no_dst).accept);
        assert!(classify_rx(&no_dst, true, OUR_PAN, OUR_SHORT, &OUR_EXT).accept);
        assert!(!classify_rx(&no_dst, true, OUR_PAN, OUR_SHORT, &OUR_EXT).ack);
    }

    #[test]
    fn promiscuous_accepts_foreign_frames_but_never_acks_them() {
        let other_node = data_frame(MacAddress::Short(OUR_PAN, ShortAddress(0x0001)), true);
        let d = classify_rx(&other_node, true, OUR_PAN, OUR_SHORT, &OUR_EXT);
        assert!(d.accept);
        assert!(!d.ack);
    }

    #[test]
    fn ack_must_match_data_request_sequence() {
        assert_eq!(matching_ack(&[0x12, 0x00, 0x42], 0x42), Some(true));
        assert_eq!(matching_ack(&[0x02, 0x00, 0x42], 0x42), Some(false));
        assert_eq!(matching_ack(&[0x02, 0x00, 0x41], 0x42), None);
        assert_eq!(matching_ack(&[0x12, 0x00, 0x41], 0x42), None);
        assert_eq!(matching_ack(&[0x41, 0x88, 0x42], 0x42), None);
        assert_eq!(matching_ack(&[0x02, 0x00], 0x42), None);
    }

    #[test]
    fn data_frames_use_destination_address_mode() {
        let short = data_frame(MacAddress::Short(OUR_PAN, ShortAddress(0x0000)), true);
        assert_eq!(u16::from_le_bytes([short[0], short[1]]), 0x8861);
        assert_eq!(short.len(), 3 + 2 + 2 + 2 + 2);

        let ext = data_frame(MacAddress::Extended(OUR_PAN, OTHER_EXT), true);
        assert_eq!(u16::from_le_bytes([ext[0], ext[1]]), 0x8C61);
        assert_eq!(ext.len(), 3 + 2 + 8 + 2 + 2);
        let (src, dst, offset, _) = frames::parse_mac_addresses(&ext);
        assert_eq!(dst, MacAddress::Extended(OUR_PAN, OTHER_EXT));
        assert_eq!(src, MacAddress::Short(OUR_PAN, ShortAddress(0x0000)));
        assert_eq!(&ext[offset..], &[0xAA, 0xBB]);
    }

    #[test]
    fn data_requests_match_coordinator_address_mode() {
        let short_coord = MacAddress::Short(OUR_PAN, ShortAddress(0x0000));
        let ieee_src = frames::build_data_request(9, &short_coord, &OUR_EXT);
        assert_eq!(u16::from_le_bytes([ieee_src[0], ieee_src[1]]), 0xC863);
        assert_eq!(ieee_src.len(), 3 + 2 + 2 + 8 + 1);
        let (src, dst, offset, _) = frames::parse_mac_addresses(&ieee_src);
        assert_eq!(dst, short_coord);
        assert_eq!(src, MacAddress::Extended(OUR_PAN, OUR_EXT));
        assert_eq!(&ieee_src[offset..], &[0x04]);

        let short_src = frames::build_data_request_short(9, &short_coord, OUR_SHORT);
        let (src, _, offset, _) = frames::parse_mac_addresses(&short_src);
        assert_eq!(src, MacAddress::Short(OUR_PAN, OUR_SHORT));
        assert_eq!(&short_src[offset..], &[0x04]);
    }

    #[test]
    fn csma_backoff_is_device_unique_and_bounded() {
        let a = csma_seed(&OUR_EXT);
        let b = csma_seed(&OTHER_EXT);
        assert_ne!(a, b);
        assert_ne!(csma_seed(&[0; 8]), 0);

        let mut state = a;
        let mut seen = [false; 8];
        for _ in 0..256 {
            state = xorshift32(state);
            assert_ne!(state, 0);
            let slots = backoff_slots(state, 3);
            assert!(slots < 8);
            seen[slots as usize] = true;
        }
        assert!(seen.iter().all(|s| *s), "backoff never covered full window");
        assert_eq!(backoff_slots(u32::MAX, 0), 0);
        assert_eq!(backoff_slots(u32::MAX, 5), 31);
        // macMaxBE is capped at 8 even if the PIB holds a larger value.
        assert_eq!(backoff_slots(u32::MAX, 40), 255);
    }
}
