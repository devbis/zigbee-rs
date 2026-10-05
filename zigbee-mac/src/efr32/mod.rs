//! EFR32MG1P MAC backend — pure Rust, zero vendor blobs.
//!
//! Implements `MacDriver` for the Silicon Labs EFR32MG1P ARM Cortex-M4F SoC.
//! The EFR32MG1P has a multi-protocol radio (BLE + IEEE 802.15.4) and is found
//! in devices like the IKEA TRÅDFRI motion sensor and many Zigbee modules.
//!
//! This is a **pure-Rust radio driver**: all radio configuration uses direct
//! register access. No RAIL library, no GSDK binary blobs are linked.
//!
//! # IMPORTANT — Scaffold Implementation
//! The radio register values are simplified approximations. The exact register
//! sequences for 802.15.4 mode need verification against the EFR32xG1 Reference
//! Manual or extraction from the RAIL library source. The driver compiles and
//! has the correct structure, but register values need to be verified before
//! use on real hardware.

pub mod driver;
mod rac_seq;

use crate::frames::{self, parse_association_response, parse_mac_addresses};
use crate::pib::{self, PibAttribute, PibPayload, PibValue};
use crate::primitives::*;
use crate::{MacCapabilities, MacDriver, MacError, PlatformServices};
use driver::{Efr32Driver, RadioConfig, RadioError};
use zigbee_types::*;

use embassy_futures::select;
use embassy_time::{Instant, Timer};

#[cfg(feature = "efr32-trace")]
macro_rules! efr32_trace {
    ($($arg:tt)*) => {
        rtt_target::rprintln!($($arg)*);
    };
}

#[cfg(not(feature = "efr32-trace"))]
macro_rules! efr32_trace {
    ($($arg:tt)*) => {};
}

/// Maximum MAC payload size (127 - MAC overhead).
const MAX_MAC_PAYLOAD: usize = 102;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanDiagnostics {
    pub tx_attempts: u8,
    pub tx_failures: u8,
    pub rx_frames: u16,
    pub rx_errors: u16,
    pub beacons: u16,
}

/// EFR32MG1P IEEE 802.15.4 MAC driver.
///
/// Pure-Rust implementation using direct register access — no RAIL FFI.
pub struct Efr32Mac {
    driver: Efr32Driver,
    short_address: ShortAddress,
    pan_id: PanId,
    channel: u8,
    extended_address: IeeeAddress,
    coord_short_address: ShortAddress,
    associated_pan_coord: bool,
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
    scan_diagnostics: ScanDiagnostics,
    /// CSMA-CA backoff PRNG state (xorshift32). Zero means "not yet seeded";
    /// it is seeded lazily from the EUI-64 once the time driver is running.
    rng_state: u32,
    #[cfg(all(feature = "hardware-aes-efr32mg1", target_arch = "arm"))]
    aes_engine: Option<efr32mg1_hal::crypto::AesEngine>,
}

impl Efr32Mac {
    pub fn new() -> Self {
        let ieee = Self::read_factory_ieee();
        Self::from_extended_address(ieee)
    }

    fn from_extended_address(ieee: IeeeAddress) -> Self {
        let config = Self::radio_config_for_address(ieee);
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
            driver: Efr32Driver::new(config),
            short_address: ShortAddress(0xFFFF),
            pan_id: PanId(0xFFFF),
            channel: 11,
            extended_address: ieee,
            coord_short_address: ShortAddress(0xFFFF),
            associated_pan_coord: false,
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
            scan_diagnostics: ScanDiagnostics::default(),
            rng_state: 0,
            #[cfg(all(feature = "hardware-aes-efr32mg1", target_arch = "arm"))]
            aes_engine: None,
        }
    }

    fn radio_config_for_address(ieee: IeeeAddress) -> RadioConfig {
        let mut config = RadioConfig::default();
        config.extended_address = ieee;
        config
    }

    /// Consume the CRYPTO token and install its AES-128 engine after two
    /// on-silicon startup KATs.
    #[cfg(all(feature = "hardware-aes-efr32mg1", target_arch = "arm"))]
    pub fn install_aes_engine(
        &mut self,
        peripheral: efr32mg1_hal::peripherals::Crypto,
    ) -> Result<(), efr32mg1_hal::crypto::AesError> {
        let mut engine = efr32mg1_hal::crypto::AesEngine::new(
            peripheral,
            efr32mg1_hal::crypto::AesEngine::DEFAULT_TIMEOUT_ITERATIONS,
        )?;
        engine.self_test()?;
        self.aes_engine = Some(engine);
        Ok(())
    }

    /// Print a compact radio snapshot for bring-up diagnostics.
    pub fn debug_radio_snapshot(&self, tag: &str) {
        self.driver.debug_snapshot(tag);
    }

    /// Return the factory-programmed IEEE address currently used by the MAC.
    pub fn extended_address(&self) -> IeeeAddress {
        self.extended_address
    }

    pub fn scan_diagnostics(&self) -> ScanDiagnostics {
        self.scan_diagnostics
    }

    pub fn cca_snapshot(&self) -> (i8, u32, bool, u8, u16) {
        let (rssi, status, clear, state) = self.driver.cca_snapshot();
        (rssi, status, clear, state, self.driver.cca_samples())
    }

    pub fn software_ack_snapshot(&self) -> (u32, u32, u32, u8, u8) {
        self.driver.software_ack_snapshot()
    }

    /// Override the factory EUI-64 before radio initialization.
    pub fn with_extended_address(mut self, ieee: IeeeAddress) -> Self {
        self.extended_address = ieee;
        self.driver
            .update_config(|config| config.extended_address = ieee);
        self
    }

    /// Transmit a raw 802.15.4 frame for bring-up diagnostics.
    pub async fn debug_transmit_raw(&mut self, frame: &[u8]) -> Result<(), MacError> {
        self.driver
            .transmit(frame)
            .await
            .map_err(Self::map_radio_err)
    }

    /// Read the factory-programmed IEEE 802.15.4 EUI-64 address.
    ///
    /// EFR32MG1P exposes the chip unique identifier in the Device Information
    /// page DEVINFO.UNIQUEL / DEVINFO.UNIQUEH registers. On this part they sit
    /// at 0x0FE0_81F0 and 0x0FE0_81F4 respectively.
    fn read_factory_ieee() -> [u8; 8] {
        const DI_UNIQUEL_ADDR: u32 = 0x0FE0_81F0;
        const DI_UNIQUEH_ADDR: u32 = 0x0FE0_81F4;

        let mut eui64 = [0u8; 8];
        let lo = unsafe { core::ptr::read_volatile(DI_UNIQUEL_ADDR as *const u32) };
        let hi = unsafe { core::ptr::read_volatile(DI_UNIQUEH_ADDR as *const u32) };

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
            log::warn!("efr32: no factory EUI64 — using fallback");
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
    /// The xorshift32 state is device-unique (seeded from the EUI-64) and is
    /// re-mixed with the monotonic clock and the last CCA RSSI sample on every
    /// draw, so nodes that start in lockstep do not keep colliding.
    fn random_backoff(&mut self, be: u8) -> u32 {
        let now = Instant::now().as_ticks();
        if self.rng_state == 0 {
            self.rng_state = csma_seed(&self.extended_address);
        }
        let (cca_rssi, _, _, _) = self.driver.cca_snapshot();
        let noise = (now as u32) ^ ((now >> 32) as u32) ^ ((cca_rssi as u8 as u32) << 24);
        self.rng_state = xorshift32(self.rng_state ^ noise);
        backoff_slots(self.rng_state, be)
    }

    /// Accept a frame for this node (IEEE 802.15.4 third-level filtering).
    fn accepts_destination(&self, dst: &MacAddress) -> bool {
        self.promiscuous
            || frames::frame_is_for_us(dst, self.pan_id, self.short_address, &self.extended_address)
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
            // Use receive_ack() which doesn't reset RX_DONE or clear buffers.
            // The radio auto-transitioned TX→RX via TRANSITIONS; the ISR may
            // have already captured the ACK before we get here.
            let seq = frame[2];
            let ack_result =
                select::select(self.driver.receive_ack(), Timer::after_micros(1500)).await;

            if let select::Either::First(Ok(rx)) = ack_result
                && let Some(frame_pending) = matching_ack_frame_pending(&rx.data[..rx.len], seq)
            {
                #[cfg(feature = "efr32-trace")]
                efr32_trace!("[MAC] ACK seq={} frame_pending={}", seq, frame_pending);
                return Ok(frame_pending);
            }

            if attempt == max_retries {
                return Err(MacError::NoAck);
            }
            log::debug!(
                "efr32: no ACK for seq={}, retry {}/{}",
                seq,
                attempt + 1,
                max_retries
            );
        }

        Err(MacError::NoAck)
    }

    /// ACK transmission is started synchronously by the pure-Rust FRC IRQ.
    async fn send_ack(&mut self, _seq: u8) {}
}

impl MacDriver for Efr32Mac {
    async fn mlme_scan(&mut self, req: MlmeScanRequest) -> Result<MlmeScanConfirm, MacError> {
        self.scan_diagnostics = ScanDiagnostics::default();
        let mut pan_descriptors: PanDescriptorList = heapless::Vec::new();
        let mut energy_list: EdList = heapless::Vec::new();

        let scan_duration_us = pib::scan_duration_us(req.scan_duration);

        for ch in 11u8..=26 {
            if req.channel_mask.0 & (1u32 << ch) == 0 {
                continue;
            }

            self.driver.update_config(|c| c.channel = ch);

            match req.scan_type {
                ScanType::Ed => {
                    efr32_trace!("scan ED ch{}", ch);
                    let (rssi, _busy) = self.driver.energy_detect().map_err(Self::map_radio_err)?;
                    let ed = ((rssi as i16 + 100).clamp(0, 255)) as u8;
                    let _ = energy_list.push(EdValue {
                        channel: ch,
                        energy: ed,
                    });
                }
                ScanType::Active => {
                    let mut rx_frames = 0u16;
                    let mut rx_errors = 0u16;
                    let mut beacons = 0u16;
                    efr32_trace!(
                        "[MAC][EFR32] scan active ch={} dur_us={}",
                        ch,
                        scan_duration_us
                    );
                    let seq = self.next_bsn();
                    let beacon_req = build_beacon_request(seq);
                    self.scan_diagnostics.tx_attempts =
                        self.scan_diagnostics.tx_attempts.saturating_add(1);
                    let tx_result = self.driver.transmit(&beacon_req).await;
                    if tx_result.is_err() {
                        self.scan_diagnostics.tx_failures =
                            self.scan_diagnostics.tx_failures.saturating_add(1);
                        efr32_trace!("[MAC][EFR32] scan beacon_req_err ch={}", ch);
                    }

                    let deadline = embassy_time::Instant::now()
                        + embassy_time::Duration::from_micros(scan_duration_us);
                    while !pan_descriptors.is_full() {
                        let now = embassy_time::Instant::now();
                        if now >= deadline {
                            break;
                        }
                        let remaining = deadline - now;
                        let result =
                            select::select(self.driver.receive(), Timer::after(remaining)).await;

                        match result {
                            select::Either::Second(_) => break,
                            select::Either::First(Err(_)) => {
                                rx_errors = rx_errors.saturating_add(1);
                                self.scan_diagnostics.rx_errors =
                                    self.scan_diagnostics.rx_errors.saturating_add(1);
                                continue;
                            }
                            select::Either::First(Ok(frame)) => {
                                rx_frames = rx_frames.saturating_add(1);
                                self.scan_diagnostics.rx_frames =
                                    self.scan_diagnostics.rx_frames.saturating_add(1);
                                if let Some(pd) = parse_beacon_frame(&frame.data[..frame.len], ch) {
                                    beacons = beacons.saturating_add(1);
                                    self.scan_diagnostics.beacons =
                                        self.scan_diagnostics.beacons.saturating_add(1);
                                    efr32_trace!(
                                        "[MAC][EFR32] scan beacon ch={} pan=0x{:04X} coord=0x{:04X} permit={} lqi={}",
                                        ch,
                                        pd.coord_address.pan_id().0,
                                        match pd.coord_address {
                                            MacAddress::Short(_, addr) => addr.0,
                                            MacAddress::Extended(_, _) => 0xFFFF,
                                        },
                                        pd.superframe_spec.association_permit,
                                        pd.lqi
                                    );
                                    let _ = pan_descriptors.push(pd);
                                }
                            }
                        }
                    }
                    efr32_trace!(
                        "[MAC][EFR32] scan done ch={} frames={} beacons={} rx_errs={}",
                        ch,
                        rx_frames,
                        beacons,
                        rx_errors
                    );
                }
                ScanType::Passive => {
                    let mut rx_frames = 0u16;
                    let mut rx_errors = 0u16;
                    let mut beacons = 0u16;
                    efr32_trace!(
                        "[MAC][EFR32] scan passive ch={} dur_us={}",
                        ch,
                        scan_duration_us
                    );
                    let deadline = embassy_time::Instant::now()
                        + embassy_time::Duration::from_micros(scan_duration_us);
                    while !pan_descriptors.is_full() {
                        let now = embassy_time::Instant::now();
                        if now >= deadline {
                            break;
                        }
                        let remaining = deadline - now;
                        let result =
                            select::select(self.driver.receive(), Timer::after(remaining)).await;

                        match result {
                            select::Either::Second(_) => break,
                            select::Either::First(Err(_)) => {
                                rx_errors = rx_errors.saturating_add(1);
                                continue;
                            }
                            select::Either::First(Ok(frame)) => {
                                rx_frames = rx_frames.saturating_add(1);
                                if let Some(pd) = parse_beacon_frame(&frame.data[..frame.len], ch) {
                                    beacons = beacons.saturating_add(1);
                                    efr32_trace!(
                                        "[MAC][EFR32] scan beacon ch={} pan=0x{:04X} coord=0x{:04X} permit={} lqi={}",
                                        ch,
                                        pd.coord_address.pan_id().0,
                                        match pd.coord_address {
                                            MacAddress::Short(_, addr) => addr.0,
                                            MacAddress::Extended(_, _) => 0xFFFF,
                                        },
                                        pd.superframe_spec.association_permit,
                                        pd.lqi
                                    );
                                    let _ = pan_descriptors.push(pd);
                                }
                            }
                        }
                    }
                    efr32_trace!(
                        "[MAC][EFR32] scan done ch={} frames={} beacons={} rx_errs={}",
                        ch,
                        rx_frames,
                        beacons,
                        rx_errors
                    );
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
        let coord_pan = req.coord_address.pan_id();
        self.driver.update_config(|c| {
            c.channel = req.channel;
            c.pan_id = coord_pan.0;
        });

        if let MacAddress::Short(_, address) = req.coord_address {
            self.coord_short_address = address;
        }

        let seq = self.next_dsn();
        let frame = build_association_request(
            seq,
            coord_pan,
            &req.coord_address,
            &self.extended_address,
            &req.capability_info,
        );

        #[cfg(feature = "efr32-trace")]
        {
            let dump_len = core::cmp::min(frame.len(), 24);
            let mut hex = [0u8; 72];
            for i in 0..dump_len {
                let hi = frame[i] >> 4;
                let lo = frame[i] & 0x0F;
                hex[i * 3] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
                hex[i * 3 + 1] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
                hex[i * 3 + 2] = b' ';
            }
            if let Ok(s) = core::str::from_utf8(&hex[..dump_len * 3]) {
                efr32_trace!("[MAC] assoc_req[{}]: {}", frame.len(), s);
            }
        }

        self.csma_ca_transmit(&frame, true).await?;

        // macResponseWaitTime: 32 * aBaseSuperframeDuration = ~500ms
        Timer::after(embassy_time::Duration::from_millis(500)).await;

        let mut confirm: Option<MlmeAssociateConfirm> = None;

        for poll_attempt in 0..5u8 {
            if poll_attempt > 0 {
                Timer::after(embassy_time::Duration::from_millis(500)).await;
            }

            let data_req = build_data_request_ieee(
                self.next_dsn(),
                &req.coord_address,
                &self.extended_address,
            );

            #[cfg(feature = "efr32-trace")]
            efr32_trace!(
                "[MAC] poll[{}] fc={:04x} len={}",
                poll_attempt,
                u16::from_le_bytes([data_req[0], data_req[1]]),
                data_req.len()
            );

            let poll_result = self.csma_ca_transmit(&data_req, true).await;
            #[cfg(feature = "efr32-trace")]
            if let Err(ref e) = poll_result {
                efr32_trace!("[MAC] poll[{}] tx err: {:?}", poll_attempt, e);
            }
            let _ = poll_result;

            let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_millis(1500);

            for _rx_iter in 0..20u8 {
                let now = embassy_time::Instant::now();
                if now >= deadline {
                    break;
                }
                let remaining = deadline - now;

                let result = select::select(self.driver.receive(), Timer::after(remaining)).await;

                match result {
                    select::Either::Second(_) => {
                        #[cfg(feature = "efr32-trace")]
                        efr32_trace!(
                            "[MAC] poll[{}] rx timeout after {} iters",
                            poll_attempt,
                            _rx_iter
                        );
                        break;
                    }
                    select::Either::First(Err(_e)) => {
                        #[cfg(feature = "efr32-trace")]
                        efr32_trace!("[MAC] poll[{}] rx err: {:?}", poll_attempt, _e);
                        continue;
                    }
                    select::Either::First(Ok(rx)) => {
                        let data = &rx.data[..rx.len];
                        if data.len() < 3 {
                            continue;
                        }
                        // PHR already stripped by driver
                        let fc = u16::from_le_bytes([data[0], data[1]]);
                        let frame_type = fc & 0x07;

                        #[cfg(feature = "efr32-trace")]
                        efr32_trace!(
                            "[MAC] poll[{}] rx fc={:04x} ft={} len={}",
                            poll_attempt,
                            fc,
                            frame_type,
                            data.len()
                        );

                        if frame_type == 0x02 {
                            continue;
                        }

                        // Neither an Association Response for another joiner
                        // nor a foreign data frame may be consumed here.
                        let (_, dst, payload_offset, _) = parse_mac_addresses(data);
                        if !self.accepts_destination(&dst) {
                            continue;
                        }

                        if frame_type == 0x03 {
                            #[cfg(feature = "efr32-trace")]
                            if data.len() != 16 {
                                let dump_len = core::cmp::min(data.len(), 24);
                                let mut hex = [0u8; 72];
                                for i in 0..dump_len {
                                    let hi = data[i] >> 4;
                                    let lo = data[i] & 0x0F;
                                    hex[i * 3] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
                                    hex[i * 3 + 1] =
                                        if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
                                    hex[i * 3 + 2] = b' ';
                                }
                                if let Ok(s) = core::str::from_utf8(&hex[..dump_len * 3]) {
                                    efr32_trace!("[MAC] cmd[{}]: {}", data.len(), s);
                                }
                            }
                            if let Some((addr, status_byte)) = parse_association_response(data) {
                                let status = match status_byte {
                                    0x00 => AssociationStatus::Success,
                                    0x01 => AssociationStatus::PanAtCapacity,
                                    _ => AssociationStatus::PanAccessDenied,
                                };
                                if status_byte == 0 {
                                    self.pan_id = coord_pan;
                                    self.short_address = addr;
                                    self.associated_pan_coord = true;
                                    self.driver.update_config(|c| {
                                        c.pan_id = coord_pan.0;
                                        c.short_address = addr.0;
                                    });
                                }
                                confirm = Some(MlmeAssociateConfirm {
                                    short_address: addr,
                                    status,
                                });
                                break;
                            }
                        }

                        if frame_type == 0x01
                            && self.pending_assoc_frame.is_none()
                            && data.len() > payload_offset
                        {
                            let payload = &data[payload_offset..];
                            let copy_len = payload.len().min(128);
                            // Reuse stack allocation from pending_assoc_frame tuple
                            let mut buf = [0u8; 128];
                            buf[..copy_len].copy_from_slice(&payload[..copy_len]);
                            self.pending_assoc_frame = Some((buf, copy_len));
                            log::info!("efr32: saved post-assoc frame ({} bytes)", copy_len);
                        }
                    }
                }
            }

            if confirm.is_some() {
                break;
            }
        }

        // Listen briefly for Transport-Key after association
        if confirm.is_some() && self.pending_assoc_frame.is_none() {
            #[cfg(feature = "efr32-trace")]
            efr32_trace!("[MAC] post-assoc listen (2s)...");
            let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_millis(2000);
            let mut _rx_count = 0u8;
            for _ in 0..20u8 {
                let now = embassy_time::Instant::now();
                if now >= deadline {
                    break;
                }
                let remaining = deadline - now;
                let result = select::select(self.driver.receive(), Timer::after(remaining)).await;
                if let select::Either::First(Ok(rx)) = result {
                    let data = &rx.data[..rx.len];
                    _rx_count += 1;
                    if data.len() >= 3 {
                        let fc = u16::from_le_bytes([data[0], data[1]]);
                        let frame_type = fc & 0x07;
                        #[cfg(feature = "efr32-trace")]
                        if _rx_count <= 5 {
                            let (_, dst, _, _) = parse_mac_addresses(data);
                            let dst_str = match &dst {
                                MacAddress::Short(_, a) => a.0,
                                MacAddress::Extended(_, _) => 0xEEEE,
                            };
                            efr32_trace!(
                                "[MAC] post-rx[{}] ft={} len={} dst={:04X}",
                                _rx_count,
                                frame_type,
                                data.len(),
                                dst_str
                            );
                        }
                        if frame_type == 0x01 {
                            let (_, dst, payload_offset, _) = parse_mac_addresses(data);
                            // Save data frames for us (destination and PAN).
                            if self.accepts_destination(&dst) && data.len() > payload_offset {
                                let payload = &data[payload_offset..];
                                let copy_len = payload.len().min(128);
                                // Reuse stack allocation
                                let mut buf = [0u8; 128];
                                buf[..copy_len].copy_from_slice(&payload[..copy_len]);
                                self.pending_assoc_frame = Some((buf, copy_len));
                                #[cfg(feature = "efr32-trace")]
                                efr32_trace!("[MAC] saved pending frame: {} bytes", copy_len);
                                break;
                            }
                        }
                    }
                } else {
                    break;
                }
            }
            #[cfg(feature = "efr32-trace")]
            efr32_trace!(
                "[MAC] post-assoc: {} frames, pending={}",
                _rx_count,
                self.pending_assoc_frame.is_some()
            );
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
        self.associated_pan_coord = false;
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
            self.associated_pan_coord = false;
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
        // `Efr32Mac` does not implement `ParentMacDriver`. The ISR software
        // ACK path exists and is silicon-proven, but it composes a fixed
        // `0x02` FCF with the Frame Pending bit clear and there is no
        // per-child source-match table, so a polling child can never be told
        // that data is waiting. Fail explicitly rather than reporting a
        // router start the backend cannot honour.
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
            MacAssociatedPanCoord => PibValue::Bool(self.associated_pan_coord),
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
            (MacAssociatedPanCoord, PibValue::Bool(v)) => {
                self.associated_pan_coord = v;
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

        let parent = MacAddress::Short(self.pan_id, self.coord_short_address);
        let has_short = self.short_address.0 != 0xFFFF && self.short_address.0 != 0xFFFE;

        let passes: u8 = if has_short { 2 } else { 1 };

        // Track whether *any* pass drew a MAC ACK from the parent. A poll that
        // is ACKed but carries no pending data is a completed empty poll
        // (`Ok(None)`); a poll whose Data Requests all went unacknowledged is a
        // lost-parent signal and must surface as an error, not be silently
        // mapped to the same `Ok(None)`. `last_err` remembers why the last
        // unacknowledged pass failed so the caller sees the real cause.
        let mut any_ack = false;
        let mut last_err: Option<MacError> = None;

        for pass in 0..passes {
            let data_req = if pass == 0 && has_short {
                build_data_request(
                    self.next_dsn(),
                    self.pan_id,
                    self.coord_short_address,
                    self.short_address,
                )
                .as_slice()
                .iter()
                .copied()
                .collect::<heapless::Vec<u8, 24>>()
            } else {
                build_data_request_ieee(self.next_dsn(), &parent, &self.extended_address)
            };

            #[cfg(feature = "efr32-trace")]
            if pass == 0 && has_short {
                efr32_trace!(
                    "[MAC] data_poll short src=0x{:04X} dst=0x{:04X}",
                    self.short_address.0,
                    self.coord_short_address.0
                );
            } else {
                efr32_trace!("[MAC] data_poll extended");
            }

            let frame_pending = match self.csma_ca_transmit(&data_req, true).await {
                Ok(frame_pending) => {
                    // An ACK arrived (with or without the frame-pending bit):
                    // the parent is reachable, so this poll is not a no-ACK
                    // loss regardless of what the receive window yields below.
                    any_ack = true;
                    frame_pending
                }
                Err(error) => {
                    last_err = Some(error);
                    continue;
                }
            };
            if !frame_pending {
                // Preserve the extended-address fallback without waiting for
                // a frame the parent's ACK says is not pending.
                continue;
            }

            let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_millis(1500);

            let mut got_none = false;
            for _rx_attempt in 0..40u8 {
                let now = embassy_time::Instant::now();
                if now >= deadline {
                    break;
                }
                let remaining = deadline - now;

                // Keep the post-TX receiver and BUFC contents intact. The
                // parent's indirect frame may start immediately after the
                // ACK; receive() would reset RX_DONE, clear BUFC, and restart
                // RX in that critical turnaround window.
                let rx_result =
                    select::select(self.driver.receive_ack(), Timer::after(remaining)).await;

                match rx_result {
                    select::Either::Second(_) => break,
                    select::Either::First(Err(_)) => continue,
                    select::Either::First(Ok(rx)) => {
                        let data = &rx.data[..rx.len];
                        if data.len() < 3 {
                            continue;
                        }
                        let fc = u16::from_le_bytes([data[0], data[1]]);
                        let frame_type = fc & 0x07;

                        if frame_type == 0x02 {
                            // Only an ACK for this Data Request may end the
                            // window; foreign ACKs on the channel are ignored.
                            if matching_ack_frame_pending(data, data_req[2]) == Some(false) {
                                got_none = true;
                                break;
                            }
                            continue;
                        }

                        if frame_type != 0x01 {
                            continue;
                        }

                        let (_src, dst, payload_offset, _security_use) = parse_mac_addresses(data);
                        if !self.accepts_destination(&dst) {
                            continue;
                        }

                        if (fc >> 5) & 1 != 0 {
                            self.send_ack(data[2]).await;
                        }

                        if data.len() <= payload_offset {
                            continue;
                        }

                        let mac_frame = MacFrame::from_slice(&data[payload_offset..])
                            .unwrap_or_else(MacFrame::new);

                        return Ok(Some(mac_frame));
                    }
                }
            }

            if got_none {
                return Ok(None);
            }
        }

        // No pass produced a data frame. Distinguish an ACKed-but-empty poll
        // (parent answered, nothing pending) from a poll that drew no ACK at
        // all (parent silent) so the runtime can drive recovery on the latter.
        poll_terminal_outcome(any_ack, last_err)
    }

    async fn mlme_poll_timeout(&mut self, timeout_us: u32) -> Result<Option<MacFrame>, MacError> {
        match select::select(
            self.mlme_poll(),
            Timer::after(embassy_time::Duration::from_micros(timeout_us as u64)),
        )
        .await
        {
            select::Either::First(result) => result,
            select::Either::Second(_) => {
                if self.driver.cancel_pending_operation() {
                    Ok(None)
                } else {
                    Err(MacError::RadioError)
                }
            }
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
            if now >= deadline {
                return Err(MacError::NoData);
            }
            let remaining = deadline - now;

            let rx_result = select::select(self.driver.receive(), Timer::after(remaining)).await;

            match rx_result {
                select::Either::Second(_) => {
                    return Err(MacError::NoData);
                }
                select::Either::First(Err(_)) => {
                    continue;
                }
                select::Either::First(Ok(rx)) => {
                    let data = &rx.data[..rx.len];

                    if data.len() < 5 {
                        continue;
                    }

                    let fc = u16::from_le_bytes([data[0], data[1]]);
                    let frame_type = fc & 0x07;

                    if frame_type != 1 {
                        continue;
                    }

                    let (src_address, dst_address, payload_offset, security_use) =
                        parse_mac_addresses(data);

                    // Only 0xFFFF is a MAC broadcast; 0xFFFD/0xFFFC are NWK
                    // values and never valid MAC destinations.
                    if !self.accepts_destination(&dst_address) {
                        #[cfg(feature = "efr32-trace")]
                        efr32_trace!(
                            "[MAC] rx FILTERED (me={:04X} pan={:04X})",
                            self.short_address.0,
                            self.pan_id.0
                        );
                        continue;
                    }

                    // The FRC IRQ already ACKed AR frames that passed the
                    // hardware destination filter; send_ack is a no-op here.
                    if (fc >> 5) & 1 != 0 {
                        self.send_ack(data[2]).await;
                    }

                    if data.len() <= payload_offset {
                        continue;
                    }

                    let mac_frame =
                        MacFrame::from_slice(&data[payload_offset..]).unwrap_or_else(MacFrame::new);

                    log::trace!("efr32: rx {} bytes lqi={}", rx.len, rx.lqi);

                    return Ok(McpsDataIndication {
                        src_address,
                        dst_address,
                        lqi: rx.lqi,
                        payload: mac_frame,
                        security_use,
                    });
                }
            }
        }
    }

    fn capabilities(&self) -> MacCapabilities {
        MacCapabilities::non_parent(102, TxPower(-20), TxPower(19))
    }
}

#[cfg(any(not(feature = "hardware-aes-efr32mg1"), not(target_arch = "arm")))]
impl zigbee_crypto::ForwardAesProvider for Efr32Mac {}

#[cfg(all(feature = "hardware-aes-efr32mg1", target_arch = "arm"))]
impl zigbee_crypto::ForwardAesProvider for Efr32Mac {
    fn forward_cipher(
        &mut self,
        key: &zigbee_crypto::AesKey,
    ) -> impl zigbee_crypto::Aes128Forward + '_ {
        let engine = self
            .aes_engine
            .as_mut()
            .expect("AES engine not installed: call Efr32Mac::install_aes_engine()");
        zigbee_crypto::efr32mg1::HardwareAes128::new(engine, *key)
    }
}

impl PlatformServices for Efr32Mac {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_ieee_is_forwarded_to_radio_ack_filter() {
        let ieee = [0xF1, 0xF0, 0xDA, 0xFE, 0xFF, 0x6F, 0x0D, 0x02];
        let config = Efr32Mac::radio_config_for_address(ieee);

        assert_eq!(config.extended_address, ieee);
    }

    #[test]
    fn matching_ack_reports_frame_pending() {
        assert_eq!(matching_ack_frame_pending(&[0x02, 0x00, 7], 7), Some(false));
        assert_eq!(matching_ack_frame_pending(&[0x12, 0x00, 7], 7), Some(true));
        assert_eq!(matching_ack_frame_pending(&[0x12, 0x00, 8], 7), None);
        assert_eq!(matching_ack_frame_pending(&[0x11, 0x00, 7], 7), None);
    }

    const OUR_EXT: IeeeAddress = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    const OTHER_EXT: IeeeAddress = [0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8];

    fn association_response(dst: &IeeeAddress) -> heapless::Vec<u8, 32> {
        // Command, AR, PAN compression, dst/src extended.
        let mut rsp = heapless::Vec::new();
        rsp.extend_from_slice(&[0x63, 0xCC, 0x05, 0x62, 0x1A])
            .unwrap();
        rsp.extend_from_slice(dst).unwrap();
        rsp.extend_from_slice(&OTHER_EXT).unwrap();
        rsp.extend_from_slice(&[0x02, 0x44, 0x33, 0x00]).unwrap();
        rsp
    }

    #[test]
    fn truncated_association_response_never_reads_out_of_bounds() {
        let rsp = association_response(&OUR_EXT);
        assert_eq!(
            parse_association_response(&rsp),
            Some((ShortAddress(0x3344), 0))
        );
        // The former local parser read data[offset + 3] after only checking
        // offset + 3 <= len, panicking on a response without status byte.
        for len in 0..rsp.len() {
            assert_eq!(parse_association_response(&rsp[..len]), None);
        }
    }

    #[test]
    fn association_capture_rejects_responses_for_other_joiners() {
        let unassociated = |rsp: &[u8]| {
            let (_, dst, _, _) = parse_mac_addresses(rsp);
            frames::frame_is_for_us(&dst, PanId(0xFFFF), ShortAddress(0xFFFF), &OUR_EXT)
        };
        assert!(unassociated(&association_response(&OUR_EXT)));
        assert!(!unassociated(&association_response(&OTHER_EXT)));
    }

    #[test]
    fn poll_rx_rejects_nwk_broadcast_mac_destinations() {
        let pan = PanId(0x1A62);
        let me = ShortAddress(0x3344);
        for (addr, expected) in [
            (0x3344, true),
            (0xFFFF, true),
            (0xFFFD, false),
            (0xFFFC, false),
        ] {
            let dst = MacAddress::Short(pan, ShortAddress(addr));
            assert_eq!(
                frames::frame_is_for_us(&dst, pan, me, &OUR_EXT),
                expected,
                "dst {addr:#06X}"
            );
        }
    }

    #[test]
    fn data_frame_fcf_follows_destination_address_mode() {
        let pan = PanId(0x1A62);
        let build = |dst: MacAddress| {
            frames::build_data_frame(
                1,
                AddressMode::Short,
                ShortAddress(0x3344),
                &OUR_EXT,
                &dst,
                &[0xAA],
                true,
                false,
            )
            .unwrap()
        };
        // Proven on-air short/short framing is byte-identical.
        let short = build(MacAddress::Short(pan, ShortAddress(0x0000)));
        assert_eq!(
            short.as_slice(),
            &[0x61, 0x88, 1, 0x62, 0x1A, 0x00, 0x00, 0x44, 0x33, 0xAA]
        );
        // Extended destination: FCF dst mode 3 and 8 address bytes.
        let ext = build(MacAddress::Extended(pan, OTHER_EXT));
        assert_eq!(u16::from_le_bytes([ext[0], ext[1]]), 0x8C61);
        let (src, dst, offset, _) = parse_mac_addresses(&ext);
        assert_eq!(dst, MacAddress::Extended(pan, OTHER_EXT));
        assert_eq!(src, MacAddress::Short(pan, ShortAddress(0x3344)));
        assert_eq!(&ext[offset..], &[0xAA]);
    }

    #[test]
    fn csma_backoff_is_device_unique_and_bounded() {
        assert_ne!(csma_seed(&OUR_EXT), csma_seed(&OTHER_EXT));
        assert_ne!(csma_seed(&[0; 8]), 0);
        let mut state = csma_seed(&OUR_EXT);
        let mut seen = [false; 8];
        for _ in 0..256 {
            state = xorshift32(state);
            assert_ne!(state, 0);
            let slots = backoff_slots(state, 3);
            assert!(slots < 8);
            seen[slots as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
        assert_eq!(backoff_slots(u32::MAX, 40), 255);
    }

    #[test]
    fn acked_empty_poll_is_distinct_from_a_no_ack_poll() {
        // Parent ACKed at least one Data Request but had nothing pending:
        // an ordinary empty poll, never conflated with a transport failure.
        assert!(matches!(poll_terminal_outcome(true, None), Ok(None)));
        assert!(
            matches!(poll_terminal_outcome(true, Some(MacError::NoAck)), Ok(None)),
            "an ACK on any pass wins even if an earlier pass went unacknowledged"
        );

        // No pass drew an ACK: the parent is silent. The real cause is
        // surfaced (defaulting to NoAck) so the runtime can drive recovery.
        assert!(matches!(
            poll_terminal_outcome(false, None),
            Err(MacError::NoAck)
        ));
        assert!(matches!(
            poll_terminal_outcome(false, Some(MacError::NoAck)),
            Err(MacError::NoAck)
        ));
        assert!(matches!(
            poll_terminal_outcome(false, Some(MacError::ChannelAccessFailure)),
            Err(MacError::ChannelAccessFailure)
        ));
    }
}

// ── Frame builders ──────────────────────────────────────────────

/// Terminal outcome of an `mlme_poll` that returned neither a data frame nor a
/// frame-pending ACK.
///
/// * `any_ack == true`  → the parent acknowledged at least one Data Request but
///   had nothing pending: a completed empty poll, reported as `Ok(None)`.
/// * `any_ack == false` → every Data Request went unacknowledged: the parent is
///   silent, reported as the underlying error (`MacError::NoAck` by default) so
///   the runtime can count the failure and drive recovery instead of treating a
///   lost parent as an ordinary empty poll.
fn poll_terminal_outcome(
    any_ack: bool,
    last_err: Option<MacError>,
) -> Result<Option<MacFrame>, MacError> {
    if any_ack {
        Ok(None)
    } else {
        Err(last_err.unwrap_or(MacError::NoAck))
    }
}

fn matching_ack_frame_pending(frame: &[u8], sequence: u8) -> Option<bool> {
    if frame.len() < 3 {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if frame_control & 0x07 != 0x02 || frame[2] != sequence {
        return None;
    }
    Some(frame_control & 0x0010 != 0)
}

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

fn build_beacon_request(seq: u8) -> [u8; 8] {
    let fc: u16 = 0x0803;
    [fc as u8, (fc >> 8) as u8, seq, 0xFF, 0xFF, 0xFF, 0xFF, 0x07]
}

fn build_association_request(
    seq: u8,
    coord_pan: PanId,
    coord_addr: &MacAddress,
    ext_addr: &IeeeAddress,
    cap: &CapabilityInfo,
) -> heapless::Vec<u8, 32> {
    let mut frame = heapless::Vec::new();

    // FC: Command frame, ACK req, PAN ID compression, DstMode=Short, SrcMode=Extended
    let fc: u16 = 0xC863;
    let _ = frame.push(fc as u8);
    let _ = frame.push((fc >> 8) as u8);
    let _ = frame.push(seq);

    let _ = frame.push(coord_pan.0 as u8);
    let _ = frame.push((coord_pan.0 >> 8) as u8);
    match coord_addr {
        MacAddress::Short(_, a) => {
            let _ = frame.push(a.0 as u8);
            let _ = frame.push((a.0 >> 8) as u8);
        }
        MacAddress::Extended(_, ext) => {
            for b in ext {
                let _ = frame.push(*b);
            }
        }
    }

    for b in ext_addr {
        let _ = frame.push(*b);
    }

    let _ = frame.push(0x01); // Association Request command ID
    let _ = frame.push(cap.to_byte());

    frame
}

fn build_data_request(
    seq: u8,
    pan_id: PanId,
    coord: ShortAddress,
    source: ShortAddress,
) -> [u8; 10] {
    let fc: u16 = 0x8863;
    [
        fc as u8,
        (fc >> 8) as u8,
        seq,
        pan_id.0 as u8,
        (pan_id.0 >> 8) as u8,
        coord.0 as u8,
        (coord.0 >> 8) as u8,
        source.0 as u8,
        (source.0 >> 8) as u8,
        0x04,
    ]
}

fn build_data_request_ieee(
    seq: u8,
    coord: &MacAddress,
    own_ieee: &IeeeAddress,
) -> heapless::Vec<u8, 24> {
    let mut frame: heapless::Vec<u8, 24> = heapless::Vec::new();
    // FC: Command(3), AckReq, PAN compress, src=Extended(11)
    // dst mode depends on coordinator address type
    let dst_mode: u16 = match coord {
        MacAddress::Short(_, _) => 0x02,    // Short
        MacAddress::Extended(_, _) => 0x03, // Extended
    };
    let fc: u16 = 0x0063 | (dst_mode << 10) | (0x03 << 14);
    let _ = frame.push(fc as u8);
    let _ = frame.push((fc >> 8) as u8);
    let _ = frame.push(seq);
    match coord {
        MacAddress::Short(pan, addr) => {
            let _ = frame.push(pan.0 as u8);
            let _ = frame.push((pan.0 >> 8) as u8);
            let _ = frame.push(addr.0 as u8);
            let _ = frame.push((addr.0 >> 8) as u8);
        }
        MacAddress::Extended(pan, ext) => {
            let _ = frame.push(pan.0 as u8);
            let _ = frame.push((pan.0 >> 8) as u8);
            for b in ext {
                let _ = frame.push(*b);
            }
        }
    }
    for b in own_ieee {
        let _ = frame.push(*b);
    }
    let _ = frame.push(0x04);
    frame
}

fn parse_beacon_frame(data: &[u8], channel: u8) -> Option<PanDescriptor> {
    // PHR is now stripped by the driver — data IS the PSDU
    let psdu = data;

    if psdu.len() < 3 {
        return None;
    }

    let fc = u16::from_le_bytes([psdu[0], psdu[1]]);
    if fc & 0x07 != 0x00 {
        return None;
    }

    let (src_address, _dst_address, addr_end, _security_use) = parse_mac_addresses(psdu);
    if addr_end + 2 > psdu.len() {
        return None;
    }

    let superframe_raw = u16::from_le_bytes([psdu[addr_end], psdu[addr_end + 1]]);

    // After superframe spec: GTS spec (1 byte min) + Pending Addr spec (1 byte min)
    // then Zigbee beacon payload starts
    let beacon_payload_offset = if psdu.len() > addr_end + 2 {
        let mut off = addr_end + 2; // after MAC addressing fields + superframe spec
        // GTS specification field
        let gts_spec = psdu[off];
        let gts_count = gts_spec & 0x07;
        off += 1; // GTS spec byte
        if gts_count > 0 {
            off += 1; // GTS directions
            off += gts_count as usize * 3; // each GTS descriptor = 3 bytes
        }
        // Pending address specification field
        if off < psdu.len() {
            let pending_spec = psdu[off];
            let num_short = (pending_spec & 0x07) as usize;
            let num_ext = ((pending_spec >> 4) & 0x07) as usize;
            off += 1; // pending addr spec byte
            off += num_short * 2 + num_ext * 8;
        }
        off
    } else {
        9
    };

    let zigbee_beacon = if psdu.len() >= beacon_payload_offset + 15 {
        let offset = beacon_payload_offset;
        ZigbeeBeaconPayload {
            protocol_id: psdu[offset],
            stack_profile: psdu[offset + 1] & 0x0F,
            protocol_version: (psdu[offset + 1] >> 4) & 0x0F,
            router_capacity: psdu[offset + 2] & 0x04 != 0,
            device_depth: (psdu[offset + 2] >> 3) & 0x0F,
            end_device_capacity: psdu[offset + 2] & 0x80 != 0,
            extended_pan_id: {
                let mut epid = [0u8; 8];
                epid.copy_from_slice(&psdu[offset + 3..offset + 11]);
                epid
            },
            tx_offset: [psdu[offset + 11], psdu[offset + 12], psdu[offset + 13]],
            update_id: if psdu.len() > offset + 14 {
                psdu[offset + 14]
            } else {
                0
            },
        }
    } else {
        ZigbeeBeaconPayload {
            protocol_id: 0,
            stack_profile: 2,
            protocol_version: 2,
            router_capacity: true,
            device_depth: 0,
            end_device_capacity: true,
            extended_pan_id: [0u8; 8],
            tx_offset: [0xFF, 0xFF, 0xFF],
            update_id: 0,
        }
    };

    Some(PanDescriptor {
        coord_address: src_address,
        channel,
        superframe_spec: SuperframeSpec::from_raw(superframe_raw),
        lqi: 0xFF,
        security_use: false,
        zigbee_beacon,
    })
}
