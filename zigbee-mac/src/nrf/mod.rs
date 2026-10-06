//! nRF MAC backend.
//!
//! Implements `MacDriver` using Embassy's ieee802154 radio driver for
//! Nordic nRF52840/nRF52833. Both chips share the same 802.15.4
//! radio with DMA-driven TX/RX.
//!
//! # Hardware features used
//! - Auto-CRC generation/checking
//! - Software address filtering (PAN ID + short/extended address)
//! - Software ACK generation and matching
//! - Hardware CCA before transmission, with unslotted CSMA-CA driven by the
//!   PIB (`macMinBE`, `macMaxBE`, `macMaxCSMABackoffs`) and a backoff PRNG
//!   seeded from the hardware RNG
//! - RSSI measurement
//! - Fail-closed AES-128 through the Nordic ECB EasyDMA peripheral
//!
//! # Software acknowledgement
//! Received data/command frames are acknowledged only when the AR bit is set
//! and the destination names this node exactly
//! ([`frames::software_ack_sequence`]); broadcasts and frames for other nodes
//! are never acknowledged on their behalf. Embassy-nrf 0.3 exposes only a
//! CCA-gated transmit, so a software ACK is sent from RX_IDLE as
//! `CCASTART → TXEN` (~128 µs CCA + TX ramp-up). That keeps it inside the
//! sender's macAckWaitDuration (864 µs) in practice but exceeds the nominal
//! 192 µs aTurnaroundTime, and a busy CCA silently drops the ACK (the sender
//! retries). Switching CCA mode for every ACK would force an extra
//! disable/ramp-up cycle and make the turnaround worse, so it is not done.
//! ACK timing remains a hardware-in-the-loop gate.
//!
//! # Cancellation safety
//! `Radio::try_send` in embassy-nrf 0.3.1 leaves `PACKETPTR` pointing at the
//! caller's buffer with no drop guard. Every transmit here goes through
//! [`send_packet_cancel_safe`], which forces the radio to DISABLED if the
//! future is dropped mid-transmission, before the buffer can be reused.
//! Frames received while waiting for an ACK or poll response are retained in
//! a bounded queue and delivered through `MCPS-DATA.indication` instead of
//! being discarded.
//!
//! # Dependencies
//! - `embassy-nrf` with nrf52840 or nrf52833 feature
//! - Embassy async executor
//!
//! # Supported boards
//! - nRF52840-DK, nRF52840-Dongle, Seeed XIAO nRF52840
//! - nRF52833-DK

mod radio_phy;

pub use radio_phy::{NrfRadioPhy, NrfSoftMac};

use crate::frames::{
    self, BackoffRng, UNIT_BACKOFF_PERIOD_US, ack_info, addressing_size, build_ack,
    build_association_request, build_beacon_request, build_data_frame, build_data_request,
    build_data_request_short, build_disassociation_notification, csma_backoff_slots,
    frame_is_for_us, parse_beacon, parse_dest_address, parse_source_address,
};
use crate::nrf_aes::{EcbDataBlock, EcbRegisters, NrfAesDriver};
use crate::pib::{self, PibAttribute, PibPayload, PibValue};
use crate::primitives::*;
use crate::{MacCapabilities, MacDriver, MacError, PlatformServices};
use core::sync::atomic::{AtomicBool, Ordering};
use zigbee_types::*;

use embassy_futures::select;
use embassy_time::Timer;

// Re-export embassy-nrf from the correct renamed dependency.
#[cfg(all(feature = "nrf52833", not(feature = "nrf52840")))]
use embassy_nrf52833 as embassy_nrf;
#[cfg(feature = "nrf52840")]
use embassy_nrf52840 as embassy_nrf;

use embassy_nrf::radio::Error as RadioError;
use embassy_nrf::radio::Instance as RadioInstance;
use embassy_nrf::radio::ieee802154::{Packet, Radio};
use embassy_nrf::rng::{Instance as RngInstance, Rng};

pub use crate::nrf_aes::NrfAesError;

const ECB_BASE: usize = 0x4000_E000;
const ECB_TASKS_STARTECB: usize = 0x000;
const ECB_TASKS_STOPECB: usize = 0x004;
const ECB_EVENTS_ENDECB: usize = 0x100;
const ECB_EVENTS_ERRORECB: usize = 0x104;
const ECB_INTENCLR: usize = 0x308;
const ECB_ECBDATAPTR: usize = 0x504;
const ECB_WAIT_LIMIT: u32 = 100_000;

/// Depth of the retained-frame receive queue.
///
/// Frames that arrive while the MAC is waiting for something else (an ACK, a
/// poll response, the association response) are kept here instead of being
/// discarded. A channel that already carries an active mesh delivers
/// unrelated broadcast traffic throughout the join, so this has to absorb
/// that background while still retaining the APS Transport-Key. Same depth
/// (and roughly the same RAM) as the association-only queue it replaces.
const PENDING_RX_QUEUE_LEN: usize = 8;

/// Window for the matching Imm-Ack after our transmission ends.
///
/// macAckWaitDuration is 864 µs at 2.4 GHz; the extra margin covers the RX
/// ramp-up after Embassy's TX→DISABLED transition. This is the value proven
/// on hardware by the previous single-receive implementation; the window is
/// now an absolute deadline so unrelated frames no longer end it early.
const ACK_WAIT_US: u64 = 1200;

/// How long a poll waits for the parent's frame after a frame-pending ACK.
///
/// Kept at the previously hardware-proven value: on a busy channel the
/// parent's indirect transmission competes with coordinator traffic.
const POLL_DATA_WAIT_MS: u64 = 1500;

const RADIO_BASE: usize = 0x4000_1000;
const RADIO_TASKS_STOP: usize = RADIO_BASE + 0x00C;
const RADIO_TASKS_DISABLE: usize = RADIO_BASE + 0x010;
const RADIO_TASKS_CCASTOP: usize = RADIO_BASE + 0x030;
const RADIO_SHORTS: usize = RADIO_BASE + 0x200;
const RADIO_INTENCLR: usize = RADIO_BASE + 0x308;
const RADIO_STATE: usize = RADIO_BASE + 0x550;
const RADIO_DISABLE_SPIN_LIMIT: u32 = 10_000;

/// Force the RADIO peripheral to DISABLED from any state.
///
/// Used where no Embassy `Radio` borrow can run (a dropped transmit future)
/// and by [`NrfMac::enter_low_power_idle`]. Returns `false` if the state
/// machine did not reach DISABLED within the spin budget.
fn force_radio_disabled() -> bool {
    // SAFETY: fixed RADIO registers of the nRF52833/nRF52840 RADIO instance.
    // Callers either hold `&mut` access to the owning driver or are running
    // the drop glue of the future that held it, so no other code is driving
    // the radio concurrently.
    unsafe {
        core::ptr::write_volatile(RADIO_SHORTS as *mut u32, 0);
        core::ptr::write_volatile(RADIO_INTENCLR as *mut u32, 0xFFFF_FFFF);
        if core::ptr::read_volatile(RADIO_STATE as *const u32) == 0 {
            core::sync::atomic::compiler_fence(Ordering::SeqCst);
            return true;
        }
        core::ptr::write_volatile(RADIO_TASKS_CCASTOP as *mut u32, 1);
        core::ptr::write_volatile(RADIO_TASKS_STOP as *mut u32, 1);
        core::ptr::write_volatile(RADIO_TASKS_DISABLE as *mut u32, 1);
    }
    for _ in 0..RADIO_DISABLE_SPIN_LIMIT {
        // SAFETY: read-only access to the fixed RADIO STATE register.
        if unsafe { core::ptr::read_volatile(RADIO_STATE as *const u32) } == 0 {
            // EasyDMA may have been reading the buffer; order that before the
            // caller releases or reuses it.
            core::sync::atomic::compiler_fence(Ordering::SeqCst);
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Disables the radio if dropped while a transmission is still armed.
struct TxCancelGuard {
    armed: bool,
}

impl TxCancelGuard {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for TxCancelGuard {
    fn drop(&mut self) {
        if self.armed && !force_radio_disabled() {
            log::error!("[nRF] radio did not reach DISABLED after cancelled TX");
        }
    }
}

/// Transmit `packet` (CCA-gated) without leaving EasyDMA pointed at a freed
/// buffer if this future is dropped before completion.
///
/// `Radio::try_send` has no drop guard of its own: cancelling it mid-CCA or
/// mid-TX leaves `PACKETPTR` aimed at the caller's (stack) `Packet`. The guard
/// stops the radio synchronously in the drop glue, before that storage can be
/// reused.
async fn send_packet_cancel_safe<T: RadioInstance>(
    radio: &mut Radio<'_, T>,
    packet: &mut Packet,
) -> Result<(), RadioError> {
    let guard = TxCancelGuard { armed: true };
    let result = radio.try_send(packet).await;
    guard.disarm();
    result
}

/// A raw frame (FCS excluded) retained for later delivery.
struct PendingRx {
    data: [u8; 127],
    len: u8,
    lqi: u8,
}

impl PendingRx {
    fn frame(&self) -> &[u8] {
        &self.data[..usize::from(self.len)]
    }
}

static ECB_TAKEN: AtomicBool = AtomicBool::new(false);

/// Unique process-wide ownership token for the Nordic ECB peripheral.
///
/// Embassy 0.3 does not expose ECB in its `Peripherals` struct, so this
/// backend provides equivalent singleton ownership locally. This token is
/// not cloneable, and a successful acquisition is never released.
pub struct NrfEcbToken {
    _private: (),
}

impl NrfEcbToken {
    /// Acquire the ECB peripheral exactly once.
    pub fn take() -> Option<Self> {
        ECB_TAKEN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { _private: () })
    }
}

struct NrfEcbRegisters;

impl NrfEcbRegisters {
    fn prepare() -> Self {
        Self::write(ECB_INTENCLR, 0x03);
        Self::write(ECB_EVENTS_ENDECB, 0);
        Self::write(ECB_EVENTS_ERRORECB, 0);
        // Read-back prevents a following task write from overtaking event
        // clearing on the peripheral bus.
        let _ = Self::read(ECB_EVENTS_ERRORECB);
        Self
    }

    #[inline(always)]
    fn read(offset: usize) -> u32 {
        // SAFETY: offsets are fixed registers in the nRF52833/nRF52840 ECB
        // instance. The owning engine retains Embassy's unique ECB token.
        unsafe { core::ptr::read_volatile((ECB_BASE + offset) as *const u32) }
    }

    #[inline(always)]
    fn write(offset: usize, value: u32) {
        // SAFETY: same exclusive-token and fixed-register argument as `read`.
        unsafe { core::ptr::write_volatile((ECB_BASE + offset) as *mut u32, value) }
    }

    fn clear_event(offset: usize) {
        Self::write(offset, 0);
        let _ = Self::read(offset);
    }
}

impl EcbRegisters for NrfEcbRegisters {
    fn clear_end_event(&mut self) {
        Self::clear_event(ECB_EVENTS_ENDECB);
    }

    fn clear_error_event(&mut self) {
        Self::clear_event(ECB_EVENTS_ERRORECB);
    }

    fn set_data_ptr(&mut self, data: *mut EcbDataBlock) {
        Self::write(ECB_ECBDATAPTR, data as u32);
    }

    fn start(&mut self) {
        Self::write(ECB_TASKS_STARTECB, 1);
    }

    fn end_event(&mut self) -> bool {
        Self::read(ECB_EVENTS_ENDECB) != 0
    }

    fn error_event(&mut self) -> bool {
        Self::read(ECB_EVENTS_ERRORECB) != 0
    }

    fn stop(&mut self) {
        Self::write(ECB_TASKS_STOPECB, 1);
    }
}

struct NrfEcbEngine {
    _token: NrfEcbToken,
    driver: NrfAesDriver<NrfEcbRegisters>,
}

impl NrfEcbEngine {
    fn new(token: NrfEcbToken) -> Self {
        Self {
            _token: token,
            driver: NrfAesDriver::new(NrfEcbRegisters::prepare(), ECB_WAIT_LIMIT),
        }
    }

    fn self_test(&mut self) -> Result<(), NrfAesError> {
        self.driver.self_test()
    }

    #[inline(never)]
    fn encrypt_block(&mut self, key: &[u8; 16], block: &mut [u8; 16]) -> Result<(), NrfAesError> {
        let input = *block;
        self.driver.encrypt(key, &input, block)
    }
}

struct NrfAesState {
    installed: Option<NrfEcbEngine>,
    /// Retain the token and DMA storage after a rejected KAT. This leaves AES
    /// unavailable while ensuring a failed abort can never outlive its buffer.
    rejected: Option<NrfEcbEngine>,
}

impl NrfAesState {
    const fn new() -> Self {
        Self {
            installed: None,
            rejected: None,
        }
    }

    fn install(&mut self, token: NrfEcbToken) -> Result<(), NrfAesError> {
        if self.installed.is_some() || self.rejected.is_some() {
            return Err(NrfAesError::AlreadyInstalled);
        }

        let mut engine = NrfEcbEngine::new(token);
        match engine.self_test() {
            Ok(()) => {
                self.installed = Some(engine);
                Ok(())
            }
            Err(error) => {
                self.rejected = Some(engine);
                Err(error)
            }
        }
    }

    fn engine_mut(&mut self) -> Option<&mut NrfEcbEngine> {
        self.installed.as_mut()
    }
}

struct NrfHardwareAes128<'engine> {
    engine: Option<&'engine mut NrfEcbEngine>,
    key: zigbee_crypto::AesKey,
}

impl zigbee_crypto::Aes128Forward for NrfHardwareAes128<'_> {
    type Error = NrfAesError;

    fn encrypt_block(&mut self, block: &mut [u8; 16]) -> Result<(), Self::Error> {
        self.engine
            .as_deref_mut()
            .ok_or(NrfAesError::NotInstalled)?
            .encrypt_block(&self.key, block)
    }
}

/// nRF52840 802.15.4 MAC driver.
///
/// Uses Embassy's hardware abstraction for the nRF radio peripheral.
/// TX/RX are interrupt-driven with DMA. The radio hardware handles
/// CRC generation/checking and CCA are handled by hardware. MAC destination
/// filtering and ACK matching are implemented in software.
///
/// # Usage
/// ```rust,no_run
/// use embassy_nrf::radio::ieee802154::Radio;
///
/// let radio = Radio::new(p.RADIO, Irqs);
/// let rng = Rng::new(p.RNG, Irqs);
/// let mac = NrfMac::new(radio, rng);
/// let nlme = Nlme::new(storage, mac);
/// ```
pub struct NrfMac<'a, T: RadioInstance, R: RngInstance> {
    radio: Radio<'a, T>,
    rng: Rng<'a, R>,
    // PIB state
    short_address: ShortAddress,
    pan_id: PanId,
    channel: u8,
    extended_address: IeeeAddress,
    rx_on_when_idle: bool,
    association_permit: bool,
    auto_request: bool,
    associated_pan_coord: bool,
    dsn: u8,
    bsn: u8,
    beacon_payload: PibPayload,
    max_frame_retries: u8,
    /// macMaxCSMABackoffs
    max_csma_backoffs: u8,
    /// macMinBE
    min_be: u8,
    /// macMaxBE
    max_be: u8,
    promiscuous: bool,
    tx_power: i8,
    /// macCoordShortAddress — short address of the coordinator/parent
    coord_short_address: ShortAddress,
    /// macCoordExtendedAddress — extended address of the coordinator/parent
    coord_extended_address: IeeeAddress,
    /// Raw frames for this node received while waiting for something else
    /// (ACK, poll response, association response).
    ///
    /// Sized for a busy channel: the frame that actually matters during a
    /// join is the APS Transport-Key, and it arrives *after* the association
    /// response, so unrelated broadcasts captured in the meantime must not be
    /// able to crowd it out (the oldest entry is evicted first).
    pending_rx: heapless::Deque<PendingRx, PENDING_RX_QUEUE_LEN>,
    /// CSMA backoff PRNG, seeded from the EUI-64 and the hardware RNG.
    backoff_rng: BackoffRng,
    /// Exclusively-owned Nordic ECB accelerator. Production composition roots
    /// install it and pass both startup KATs before constructing networking.
    aes: NrfAesState,
}

impl<'a, T: RadioInstance, R: RngInstance> NrfMac<'a, T, R> {
    pub fn new(radio: Radio<'a, T>, rng: Rng<'a, R>) -> Self {
        // Read factory-programmed IEEE address from FICR registers
        let ieee = Self::read_ficr_ieee();
        log::info!(
            "[MAC] IEEE: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            ieee[0],
            ieee[1],
            ieee[2],
            ieee[3],
            ieee[4],
            ieee[5],
            ieee[6],
            ieee[7],
        );

        // Seed sequence numbers and the CSMA backoff PRNG from the hardware
        // RNG so that neighbours (and this node across resets) never replay
        // the same backoff sequence; the EUI-64 keeps two seeds distinct even
        // if the RNG were to return identical words.
        let mut rng = rng;
        let mut entropy = [0u8; 4];
        rng.blocking_fill_bytes(&mut entropy);
        let seed = u32::from_le_bytes(entropy);
        let backoff_rng = BackoffRng::new(&ieee, seed);

        Self {
            radio,
            rng,
            short_address: ShortAddress(0xFFFF),
            pan_id: PanId(0xFFFF),
            channel: 11,
            extended_address: ieee,
            rx_on_when_idle: false,
            association_permit: false,
            auto_request: true,
            associated_pan_coord: false,
            dsn: seed as u8,
            bsn: (seed >> 8) as u8,
            beacon_payload: PibPayload::new(),
            max_frame_retries: 3,
            max_csma_backoffs: 4,
            min_be: 3,
            max_be: 5,
            promiscuous: false,
            tx_power: 0,
            coord_short_address: ShortAddress(0x0000),
            coord_extended_address: [0; 8],
            pending_rx: heapless::Deque::new(),
            backoff_rng,
            aes: NrfAesState::new(),
        }
    }

    /// Consume the unique ECB token and install hardware AES after two
    /// back-to-back AES-128 known-answer tests with different keys.
    ///
    /// A failed engine is retained only as rejected/quarantined ownership;
    /// it is never made available to CCM* or AES-MMO.
    pub fn install_aes_engine(&mut self, token: NrfEcbToken) -> Result<(), NrfAesError> {
        self.aes.install(token)
    }

    /// Return the factory-programmed EUI-64 currently used by the MAC.
    pub const fn extended_address(&self) -> IeeeAddress {
        self.extended_address
    }

    /// Read the unique device IEEE (EUI-64) address from nRF52840 FICR registers.
    /// FICR.DEVICEID[0] at 0x10000060 (low 32 bits)
    /// FICR.DEVICEID[1] at 0x10000064 (high 32 bits)
    fn read_ficr_ieee() -> IeeeAddress {
        const FICR_DEVICEID0: *const u32 = 0x1000_0060 as *const u32;
        const FICR_DEVICEID1: *const u32 = 0x1000_0064 as *const u32;
        let lo = unsafe { core::ptr::read_volatile(FICR_DEVICEID0) };
        let hi = unsafe { core::ptr::read_volatile(FICR_DEVICEID1) };
        let mut addr = [0u8; 8];
        addr[0..4].copy_from_slice(&lo.to_le_bytes());
        addr[4..8].copy_from_slice(&hi.to_le_bytes());
        addr
    }

    fn next_dsn(&mut self) -> u8 {
        let seq = self.dsn;
        self.dsn = self.dsn.wrapping_add(1);
        seq
    }

    fn next_random_u32(&mut self) -> u32 {
        self.backoff_rng.next_u32()
    }

    /// Stir a fresh hardware-RNG word into the backoff PRNG and return it.
    fn reseed_from_hardware(&mut self) -> u32 {
        let mut entropy = [0u8; 4];
        self.rng.blocking_fill_bytes(&mut entropy);
        let word = u32::from_le_bytes(entropy);
        self.backoff_rng.mix(word);
        word
    }

    /// Keep a received data frame that arrived outside
    /// `MCPS-DATA.indication`, if it is addressed to this node (or broadcast).
    fn retain_frame(&mut self, data: &[u8], lqi: u8) {
        if data.len() < 3 || data[0] & 0x07 != 0x01 || data.len() > 127 {
            return;
        }
        if !self.promiscuous {
            let fc = u16::from_le_bytes([data[0], data[1]]);
            let Some(dst) = parse_dest_address(data, fc) else {
                return;
            };
            if !frame_is_for_us(
                &dst,
                self.pan_id,
                self.short_address,
                &self.extended_address,
            ) {
                return;
            }
        }
        // Evict the oldest entry instead of rejecting the newest. The frames
        // that complete a join (association response, then the APS
        // Transport-Key) are always the most recent ones, so a queue already
        // filled with unrelated broadcasts must never be allowed to
        // permanently discard them.
        if self.pending_rx.is_full() {
            let _ = self.pending_rx.pop_front();
            log::debug!("[MAC] Retained-frame queue full — evicted oldest frame");
        }
        let mut entry = PendingRx {
            data: [0; 127],
            len: data.len() as u8,
            lqi,
        };
        entry.data[..data.len()].copy_from_slice(data);
        let _ = self.pending_rx.push_back(entry);
    }

    /// Remove and return the first retained frame that answers our poll
    /// (the parent's data frame addressed exactly to us), keeping the order of
    /// everything else.
    fn take_retained_poll_response(&mut self) -> Option<MacFrame> {
        let mut found = None;
        for _ in 0..self.pending_rx.len() {
            let Some(entry) = self.pending_rx.pop_front() else {
                break;
            };
            if found.is_none() && self.is_poll_response(entry.frame()) {
                found = Some(entry);
            } else {
                let _ = self.pending_rx.push_back(entry);
            }
        }
        let entry = found?;
        let data = entry.frame();
        let fc = u16::from_le_bytes([data[0], data[1]]);
        data.get(3 + addressing_size(fc)..)
            .filter(|payload| !payload.is_empty())
            .and_then(MacFrame::from_slice)
    }

    /// Whether `data` is the parent's indirect frame answering our poll.
    fn is_poll_response(&self, data: &[u8]) -> bool {
        frames::is_parent_poll_response(
            data,
            self.pan_id,
            self.short_address,
            &self.extended_address,
            self.coord_short_address,
            &self.coord_extended_address,
        )
    }

    /// CCA-gated transmit that is safe to cancel (see module docs).
    async fn transmit(&mut self, packet: &mut Packet) -> Result<(), RadioError> {
        send_packet_cancel_safe(&mut self.radio, packet).await
    }

    /// Acknowledge `data` in software if it is a data/command frame with the
    /// AR bit set whose destination names this node exactly. Broadcasts and
    /// frames for other nodes are never acknowledged.
    async fn acknowledge_if_required(&mut self, data: &[u8]) {
        if self.promiscuous {
            return;
        }
        let Some(sequence) = frames::software_ack_sequence(
            data,
            self.pan_id,
            self.short_address,
            &self.extended_address,
        ) else {
            return;
        };
        let mut ack = Packet::new();
        ack.copy_from_slice(&build_ack(sequence, false));
        if self.transmit(&mut ack).await.is_err() {
            // CCA busy: the sender will retransmit and we ACK that copy.
            log::debug!("[MAC] software ACK seq={} lost to busy CCA", sequence);
        }
    }

    /// Handle a frame received while waiting for something else: ACK it if
    /// required and retain it for `MCPS-DATA.indication`.
    async fn handle_out_of_band(&mut self, data: &[u8], lqi: u8) {
        self.acknowledge_if_required(data).await;
        self.retain_frame(data, lqi);
    }

    /// Listen for the Imm-Ack of `sequence` until [`ACK_WAIT_US`] elapses.
    ///
    /// Returns `Some(frame_pending)` for a matching ACK and `None` when the
    /// window closes without one. Other frames are retained, not dropped.
    async fn wait_for_ack(&mut self, sequence: u8) -> Option<bool> {
        let deadline =
            embassy_time::Instant::now() + embassy_time::Duration::from_micros(ACK_WAIT_US);
        loop {
            let now = embassy_time::Instant::now();
            if now >= deadline {
                return None;
            }
            let mut rx_pkt = Packet::new();
            match select::select(
                Timer::after(deadline - now),
                self.radio.receive(&mut rx_pkt),
            )
            .await
            {
                select::Either::First(()) => return None,
                select::Either::Second(Err(_)) => {}
                select::Either::Second(Ok(())) => {
                    let lqi = crate::lqi::nrf_from_hardware(rx_pkt.lqi());
                    let data = rx_pkt.as_ref();
                    match ack_info(data) {
                        Some((seq, pending)) if seq == sequence => return Some(pending),
                        Some(_) => {} // another pair's ACK
                        None => self.handle_out_of_band(data, lqi).await,
                    }
                }
            }
        }
    }

    /// After a frame-pending ACK, wait for the parent's data frame.
    ///
    /// Only a data frame from the parent addressed exactly to us completes
    /// the poll. Everything else that arrives meanwhile is acknowledged if it
    /// is for us and retained for `MCPS-DATA.indication`, never returned as
    /// if the parent had sent it.
    async fn receive_poll_response(&mut self) -> Option<MacFrame> {
        let deadline =
            embassy_time::Instant::now() + embassy_time::Duration::from_millis(POLL_DATA_WAIT_MS);
        loop {
            let now = embassy_time::Instant::now();
            if now >= deadline {
                log::debug!("[MAC:Poll] frame pending but no data from parent");
                return None;
            }
            let mut rx_pkt = Packet::new();
            match select::select(
                Timer::after(deadline - now),
                self.radio.receive(&mut rx_pkt),
            )
            .await
            {
                select::Either::First(()) => return None,
                select::Either::Second(Err(_)) => {}
                select::Either::Second(Ok(())) => {
                    let lqi = crate::lqi::nrf_from_hardware(rx_pkt.lqi());
                    let data = rx_pkt.as_ref();
                    if ack_info(data).is_some() {
                        continue;
                    }
                    self.acknowledge_if_required(data).await;
                    if self.is_poll_response(data) {
                        let fc = u16::from_le_bytes([data[0], data[1]]);
                        let header_len = 3 + addressing_size(fc);
                        log::info!("[MAC:Poll] parent frame {} bytes", data.len() - header_len);
                        return data
                            .get(header_len..)
                            .filter(|payload| !payload.is_empty())
                            .and_then(MacFrame::from_slice);
                    }
                    self.retain_frame(data, lqi);
                }
            }
        }
    }

    /// Third-level filter and decode for `MCPS-DATA.indication`.
    fn indication_from(&self, data: &[u8], lqi: u8) -> Option<McpsDataIndication> {
        if data.len() < 5 {
            return None;
        }
        let fc = u16::from_le_bytes([data[0], data[1]]);
        if fc & 0x07 != 0x01 {
            return None;
        }
        let header_len = 3 + addressing_size(fc);
        if data.len() <= header_len {
            return None;
        }
        let dst = parse_dest_address(data, fc)?;
        let src = parse_source_address(data, fc)?;
        if !self.promiscuous
            && !frame_is_for_us(
                &dst,
                self.pan_id,
                self.short_address,
                &self.extended_address,
            )
        {
            log::trace!("[nRF RX] Filtered dst {:?}", dst);
            return None;
        }
        Some(McpsDataIndication {
            src_address: src,
            dst_address: dst,
            lqi,
            payload: MacFrame::from_slice(&data[header_len..])?,
            security_use: (fc >> 3) & 1 != 0,
        })
    }

    /// Set the radio channel (11-26 for 2.4 GHz Zigbee).
    fn set_channel(&mut self, channel: u8) {
        self.channel = channel;
        self.radio.set_channel(channel);
    }

    /// Set radio TX power in dBm. nRF52840 supports -40 to +8 dBm.
    pub fn set_tx_power(&mut self, dbm: i8) {
        self.tx_power = dbm;
        self.radio.set_transmission_power(dbm);
    }

    /// Unslotted CSMA-CA (IEEE 802.15.4-2015 §6.2.5.1) using the PIB
    /// `macMinBE`, `macMaxBE` and `macMaxCSMABackoffs`; the CCA itself is the
    /// hardware CCA performed by `try_send`.
    async fn try_send_with_csma(&mut self, packet: &mut Packet) -> Result<(), MacError> {
        let mut backoffs = 0u8;
        let mut backoff_exponent = core::cmp::min(self.min_be, self.max_be);
        loop {
            let slots = csma_backoff_slots(self.next_random_u32(), backoff_exponent);
            if slots != 0 {
                Timer::after_micros(u64::from(slots) * UNIT_BACKOFF_PERIOD_US).await;
            }

            match self.transmit(packet).await {
                Ok(()) => return Ok(()),
                Err(_) if backoffs < self.max_csma_backoffs => {
                    backoffs += 1;
                    backoff_exponent = core::cmp::min(backoff_exponent + 1, self.max_be);
                }
                Err(_) => return Err(MacError::ChannelAccessFailure),
            }
        }
    }

    /// Transmit an ACK-requested frame and retry until its ACK arrives.
    ///
    /// Returns the ACK's frame-pending bit, or [`MacError::NoAck`] after
    /// `max_retries` retransmissions.
    async fn send_acknowledged_frame(
        &mut self,
        frame: &[u8],
        max_retries: u8,
    ) -> Result<bool, MacError> {
        let dsn = *frame.get(2).ok_or(MacError::InvalidParameter)?;
        for attempt in 0..=max_retries {
            let mut packet = Packet::new();
            packet.copy_from_slice(frame);
            self.try_send_with_csma(&mut packet).await?;

            if let Some(frame_pending) = self.wait_for_ack(dsn).await {
                log::debug!(
                    "[MAC TX] ACK ok dsn={} attempt={} fp={}",
                    dsn,
                    attempt,
                    frame_pending
                );
                return Ok(frame_pending);
            }
        }

        log::warn!("[MAC TX] No ACK after {} retries dsn={}", max_retries, dsn);
        Err(MacError::NoAck)
    }

    /// Stop the RADIO peripheral between sleepy-device poll windows.
    ///
    /// `&mut self` guarantees no Embassy radio future is active. Interrupts
    /// are cleared before requesting DISABLED, and the next Embassy operation
    /// performs the normal DISABLED-to-RX/TX transition.
    pub fn enter_low_power_idle(&mut self) -> Result<(), MacError> {
        self.radio.clear_all_interrupts();
        if force_radio_disabled() {
            Ok(())
        } else {
            Err(MacError::RadioError)
        }
    }

    /// Construct a beacon request MAC command frame.
    fn beacon_request_frame(&mut self) -> Packet {
        let seq = self.next_dsn();
        let mut pkt = Packet::new();
        let frame = build_beacon_request(seq);
        pkt.copy_from_slice(&frame);
        pkt
    }

    /// Scan a single channel for beacons (active scan).
    async fn scan_channel_active(
        &mut self,
        channel: u8,
        duration: u8,
    ) -> Result<heapless::Vec<PanDescriptor, MAX_PAN_DESCRIPTORS>, MacError> {
        self.set_channel(channel);

        // The beacon request must survive a busy channel: a single CCA-busy
        // result would otherwise abort this channel entirely, which is exactly
        // what happens on the channel that already carries the mesh we want to
        // join. Use the normal CSMA-CA backoff path instead of a bare TX.
        let mut pkt = self.beacon_request_frame();
        self.try_send_with_csma(&mut pkt).await?;

        let delay_us = pib::scan_duration_us(duration);
        let mut descriptors = heapless::Vec::new();

        // Listen for beacons until timeout
        let timer_fut = Timer::after_micros(delay_us);
        let rx_fut = self.collect_beacons(channel, &mut descriptors);
        let _ = select::select(timer_fut, rx_fut).await;

        Ok(descriptors)
    }

    /// Fix 4: Scan a single channel passively (listen-only, no beacon request).
    async fn scan_channel_passive(
        &mut self,
        channel: u8,
        duration: u8,
    ) -> Result<heapless::Vec<PanDescriptor, MAX_PAN_DESCRIPTORS>, MacError> {
        self.set_channel(channel);
        let delay_us = pib::scan_duration_us(duration);
        let mut descriptors = heapless::Vec::new();
        let timer_fut = Timer::after_micros(delay_us);
        let rx_fut = self.collect_beacons(channel, &mut descriptors);
        let _ = select::select(timer_fut, rx_fut).await;
        Ok(descriptors)
    }

    /// Receive and parse beacons until the scan-duration timer cancels this
    /// future.
    ///
    /// The loop budget must never be spent on frames that are not beacons.
    /// A Zigbee channel that already carries an active mesh delivers a
    /// continuous stream of data frames, ACKs and CRC failures, so a bounded
    /// count of *receive operations* ends the scan long before the scan
    /// duration elapses and hides the networks that beacon later — including
    /// the coordinator, which is usually the node with permit-join open.
    /// Only a full descriptor list stops the scan early; everything else keeps
    /// listening and lets the caller's timer decide when the channel is done.
    async fn collect_beacons(
        &mut self,
        channel: u8,
        descriptors: &mut heapless::Vec<PanDescriptor, MAX_PAN_DESCRIPTORS>,
    ) -> Result<(), MacError> {
        // A radio that fails back-to-back this many times is wedged rather
        // than merely hearing corrupted traffic. Bail out instead of spinning
        // for the rest of the scan window. Reset on every successful receive.
        const MAX_CONSECUTIVE_RADIO_ERRORS: u32 = 64;

        let mut rx_pkt = Packet::new();
        let mut consecutive_errors = 0u32;

        loop {
            match self.radio.receive(&mut rx_pkt).await {
                Ok(()) => {
                    consecutive_errors = 0;
                    let data = rx_pkt.as_ref();
                    // `Packet::lqi()` is the raw Nordic ED byte; PanDescriptor
                    // carries an IEEE 802.15.4 LQI. Normalize exactly once.
                    let lqi = crate::lqi::nrf_from_hardware(rx_pkt.lqi());
                    let Some(pd) = parse_beacon(channel, data, lqi) else {
                        continue;
                    };
                    // Descriptor slots are the scarce resource on a dense mesh.
                    // A router that beacons twice within one window must not
                    // consume a slot that another PAN still needs.
                    if descriptors
                        .iter()
                        .any(|existing| existing.coord_address == pd.coord_address)
                    {
                        continue;
                    }
                    if descriptors.push(pd).is_err() {
                        return Ok(());
                    }
                }
                Err(_) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_RADIO_ERRORS {
                        return Err(MacError::RadioError);
                    }
                }
            }
        }
    }
}

// ── MacDriver implementation ────────────────────────────────────

impl<T: RadioInstance, R: RngInstance> MacDriver for NrfMac<'_, T, R> {
    async fn mlme_scan(&mut self, req: MlmeScanRequest) -> Result<MlmeScanConfirm, MacError> {
        let mut pan_descriptors = heapless::Vec::new();
        let energy_list = heapless::Vec::new();

        log::info!(
            "[nRF MLME-SCAN] Starting {:?} scan, duration={}",
            req.scan_type,
            req.scan_duration
        );

        for channel in req.channel_mask.iter() {
            let ch = channel.number();
            log::debug!("[nRF MLME-SCAN] Scanning ch {}…", ch);
            match req.scan_type {
                ScanType::Active => match self.scan_channel_active(ch, req.scan_duration).await {
                    Ok(pds) => {
                        if !pds.is_empty() {
                            log::info!("[nRF MLME-SCAN] ch {}: {} beacon(s) found", ch, pds.len());
                        }
                        for pd in pds {
                            let _ = pan_descriptors.push(pd);
                        }
                    }
                    Err(e) => log::error!("[nRF MLME-SCAN] ch {ch}: {e:?}"),
                },
                ScanType::Passive => {
                    // Fix 4: Use passive scan for Passive scan type
                    match self.scan_channel_passive(ch, req.scan_duration).await {
                        Ok(pds) => {
                            for pd in pds {
                                let _ = pan_descriptors.push(pd);
                            }
                        }
                        Err(e) => log::error!("[nRF MLME-SCAN] ch {ch}: {e:?}"),
                    }
                }
                ScanType::Ed => {
                    log::warn!("[nRF] ED scan is not implemented by embassy-nrf");
                    return Err(MacError::Unsupported);
                }
                ScanType::Orphan => {
                    log::warn!("[nRF] Orphan scan not yet implemented");
                }
            }
        }

        if matches!(req.scan_type, ScanType::Active | ScanType::Passive)
            && pan_descriptors.is_empty()
        {
            log::warn!("[nRF MLME-SCAN] No beacons found on any channel");
            return Err(MacError::NoBeacon);
        }

        log::info!(
            "[nRF MLME-SCAN] Scan complete: {} PAN descriptor(s)",
            pan_descriptors.len()
        );

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
        self.associated_pan_coord = false;
        self.set_channel(req.channel);
        // IEEE 802.15.4 §6.4.1: MLME-ASSOCIATE.request sets macPANId to the
        // coordinator's PAN before the exchange, so the association response
        // and the following Transport-Key pass the software address filter
        // (and are acknowledged) with the correct PAN.
        let previous_pan = self.pan_id;
        self.pan_id = req.coord_address.pan_id();
        let result = self.associate_with(req).await;
        if !matches!(
            result,
            Ok(MlmeAssociateConfirm {
                status: AssociationStatus::Success,
                ..
            })
        ) {
            self.pan_id = previous_pan;
        }
        result
    }

    async fn mlme_associate_response(
        &mut self,
        _rsp: MlmeAssociateResponse,
    ) -> Result<(), MacError> {
        // TODO: coordinator/router role
        Err(MacError::Unsupported)
    }

    async fn mlme_disassociate(&mut self, req: MlmeDisassociateRequest) -> Result<(), MacError> {
        if req.tx_indirect {
            return Err(MacError::Unsupported);
        }
        let frame = build_disassociation_notification(
            self.next_dsn(),
            &req.device_address,
            self.short_address,
            &self.extended_address,
            req.reason,
        );
        self.send_acknowledged_frame(&frame, self.max_frame_retries)
            .await?;
        self.short_address = ShortAddress(0xFFFF);
        self.pan_id = PanId(0xFFFF);
        self.associated_pan_coord = false;
        self.pending_rx.clear();
        Ok(())
    }
    fn mlme_reset(&mut self, set_default_pib: bool) -> Result<(), MacError> {
        if set_default_pib {
            let random = self.reseed_from_hardware();
            self.short_address = ShortAddress(0xFFFF);
            self.pan_id = PanId(0xFFFF);
            self.channel = 11;
            self.rx_on_when_idle = false;
            self.association_permit = false;
            self.auto_request = true;
            self.associated_pan_coord = false;
            self.dsn = random as u8;
            self.bsn = (random >> 8) as u8;
            self.max_frame_retries = 3;
            self.max_csma_backoffs = 4;
            self.min_be = 3;
            self.max_be = 5;
            self.promiscuous = false;
            self.coord_short_address = ShortAddress::COORDINATOR;
            self.coord_extended_address = [0; 8];
            self.pending_rx.clear();
        }
        self.set_channel(self.channel);
        Ok(())
    }

    async fn mlme_start(&mut self, req: MlmeStartRequest) -> Result<(), MacError> {
        // `NrfMac` does not implement `ParentMacDriver`: `NrfRadioPhy::send_ack`
        // returns `PhyError::Unsupported` because embassy-nrf exposes only
        // CCA-gated TX, so no acknowledgement — let alone one carrying a
        // Frame Pending bit — can be emitted inside aTurnaroundTime. This
        // backend already failed closed; routing the rejection through the
        // shared helper keeps the reason greppable and validates the request
        // shape.
        start_requires_parent_capability(&req)
    }

    async fn mlme_get(&self, attr: PibAttribute) -> Result<PibValue, MacError> {
        match attr {
            PibAttribute::MacShortAddress => Ok(PibValue::ShortAddress(self.short_address)),
            PibAttribute::MacPanId => Ok(PibValue::PanId(self.pan_id)),
            PibAttribute::MacExtendedAddress => {
                Ok(PibValue::ExtendedAddress(self.extended_address))
            }
            PibAttribute::MacCoordShortAddress => {
                Ok(PibValue::ShortAddress(self.coord_short_address))
            }
            PibAttribute::MacCoordExtendedAddress => {
                Ok(PibValue::ExtendedAddress(self.coord_extended_address))
            }
            PibAttribute::MacAssociatedPanCoord => Ok(PibValue::Bool(self.associated_pan_coord)),
            PibAttribute::MacRxOnWhenIdle => Ok(PibValue::Bool(self.rx_on_when_idle)),
            PibAttribute::MacAssociationPermit => Ok(PibValue::Bool(self.association_permit)),
            PibAttribute::MacAutoRequest => Ok(PibValue::Bool(self.auto_request)),
            PibAttribute::MacDsn => Ok(PibValue::U8(self.dsn)),
            PibAttribute::MacBsn => Ok(PibValue::U8(self.bsn)),
            PibAttribute::MacMaxFrameRetries => Ok(PibValue::U8(self.max_frame_retries)),
            PibAttribute::MacMaxCsmaBackoffs => Ok(PibValue::U8(self.max_csma_backoffs)),
            PibAttribute::MacMinBe => Ok(PibValue::U8(self.min_be)),
            PibAttribute::MacMaxBe => Ok(PibValue::U8(self.max_be)),
            PibAttribute::PhyCurrentChannel => Ok(PibValue::U8(self.channel)),
            PibAttribute::PhyTransmitPower => Ok(PibValue::I8(self.tx_power)),
            PibAttribute::PhyChannelsSupported => Ok(PibValue::U32(ChannelMask::ALL_2_4GHZ.0)),
            PibAttribute::MacPromiscuousMode => Ok(PibValue::Bool(self.promiscuous)),
            PibAttribute::MacBeaconPayload => Ok(PibValue::Payload(self.beacon_payload.clone())),
            PibAttribute::MacBeaconPayloadLength => {
                Ok(PibValue::U8(self.beacon_payload.as_slice().len() as u8))
            }
            PibAttribute::PhyCurrentPage => Ok(PibValue::U8(0)),
            _ => Err(MacError::Unsupported),
        }
    }

    async fn mlme_set(&mut self, attr: PibAttribute, value: PibValue) -> Result<(), MacError> {
        match attr {
            PibAttribute::MacShortAddress => {
                self.short_address = value.as_short_address().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacPanId => {
                self.pan_id = value.as_pan_id().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacRxOnWhenIdle => {
                self.rx_on_when_idle = value.as_bool().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacAssociationPermit => {
                self.association_permit = value.as_bool().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacAutoRequest => {
                self.auto_request = value.as_bool().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::PhyCurrentChannel => {
                let ch = value.as_u8().ok_or(MacError::InvalidParameter)?;
                if !(11..=26).contains(&ch) {
                    return Err(MacError::InvalidParameter);
                }
                self.set_channel(ch);
            }
            PibAttribute::MacPromiscuousMode => {
                self.promiscuous = value.as_bool().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacBeaconPayload => {
                self.beacon_payload = match value {
                    PibValue::Payload(payload) => payload,
                    _ => return Err(MacError::InvalidParameter),
                };
            }
            PibAttribute::PhyTransmitPower => {
                let PibValue::I8(power) = value else {
                    return Err(MacError::InvalidParameter);
                };
                self.tx_power = power;
                self.radio.set_transmission_power(power);
            }
            PibAttribute::MacCoordShortAddress => {
                self.coord_short_address =
                    value.as_short_address().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacCoordExtendedAddress => {
                self.coord_extended_address = value
                    .as_extended_address()
                    .ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacAssociatedPanCoord => {
                self.associated_pan_coord = value.as_bool().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacExtendedAddress => {
                self.extended_address = value
                    .as_extended_address()
                    .ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacDsn => {
                self.dsn = value.as_u8().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacBsn => {
                self.bsn = value.as_u8().ok_or(MacError::InvalidParameter)?;
            }
            PibAttribute::MacMaxFrameRetries => {
                self.max_frame_retries = value.as_u8().ok_or(MacError::InvalidParameter)?;
            }
            // Same ranges as the shared `MacPib` (IEEE 802.15.4-2015 Table 8-94).
            PibAttribute::MacMaxCsmaBackoffs => {
                let backoffs = value.as_u8().ok_or(MacError::InvalidParameter)?;
                if backoffs > 5 {
                    return Err(MacError::InvalidParameter);
                }
                self.max_csma_backoffs = backoffs;
            }
            PibAttribute::MacMinBe => {
                let be = value.as_u8().ok_or(MacError::InvalidParameter)?;
                if be > self.max_be || be > 8 {
                    return Err(MacError::InvalidParameter);
                }
                self.min_be = be;
            }
            PibAttribute::MacMaxBe => {
                let be = value.as_u8().ok_or(MacError::InvalidParameter)?;
                if be < self.min_be || be > 8 {
                    return Err(MacError::InvalidParameter);
                }
                self.max_be = be;
            }
            _ => return Err(MacError::Unsupported),
        }
        Ok(())
    }

    async fn mlme_poll(&mut self) -> Result<Option<MacFrame>, MacError> {
        // A parent frame retained while we were waiting for something else
        // (e.g. the Transport-Key right after association) answers this poll.
        if let Some(frame) = self.take_retained_poll_response() {
            log::info!(
                "[MAC:Poll] Returning retained parent frame ({} bytes)",
                frame.len()
            );
            return Ok(Some(frame));
        }

        let parent = MacAddress::Short(self.pan_id, self.coord_short_address);
        let has_short = self.short_address.0 < 0xFFFE;

        // Up to two passes:
        //   Pass 0: SHORT source address (IEEE 802.15.4 §6.3.4) — matches
        //           most indirect frames.
        //   Pass 1: IEEE source address — a parent may queue a frame by our
        //           extended address (Transport-Key / Rejoin Response on
        //           EmberZNet), and only an IEEE-sourced Data Request
        //           retrieves it. Kept from the hardware-proven behaviour.
        // Without a short address only the IEEE pass runs.
        let passes: u8 = if has_short { 2 } else { 1 };

        for pass in 0..passes {
            let poll_dsn = self.next_dsn();
            let data_req = if pass == 0 && has_short {
                build_data_request_short(poll_dsn, &parent, self.short_address)
            } else {
                build_data_request(poll_dsn, &parent, &self.extended_address)
            };

            // Data Request is ACK-requested: CSMA-CA, ACK wait and
            // macMaxFrameRetries like any other acknowledged frame. The ACK's
            // frame-pending bit says whether the parent holds data for us.
            let frame_pending = match self
                .send_acknowledged_frame(&data_req, self.max_frame_retries)
                .await
            {
                Ok(frame_pending) => frame_pending,
                // The parent never acknowledged the first poll: it is
                // unreachable. Report that so parent-loss handling runs.
                Err(error) if pass == 0 => return Err(error),
                // The short pass already proved the parent reachable.
                Err(_) => return Ok(None),
            };
            if !frame_pending {
                log::debug!("[MAC:Poll] pass {} ACK frame_pending=0", pass);
                continue;
            }

            log::debug!(
                "[MAC:Poll] pass {} ACK frame_pending=1, waiting for data",
                pass
            );
            if let Some(frame) = self.receive_poll_response().await {
                return Ok(Some(frame));
            }
        }

        Ok(None)
    }

    async fn mcps_data(&mut self, req: McpsDataRequest<'_>) -> Result<McpsDataConfirm, MacError> {
        let msdu_handle = req.msdu_handle;
        let ack_requested = req.tx_options.ack_tx;
        let dsn = self.next_dsn();
        let frame = build_data_frame(
            dsn,
            req.src_addr_mode,
            self.short_address,
            &self.extended_address,
            &req.dst_address,
            req.payload,
            ack_requested,
            req.tx_options.frame_pending,
        )
        .map_err(|_| MacError::FrameTooLong)?;

        if ack_requested {
            self.send_acknowledged_frame(&frame, self.max_frame_retries)
                .await?;
        } else {
            let mut packet = Packet::new();
            packet.copy_from_slice(&frame);
            self.try_send_with_csma(&mut packet).await?;
        }

        Ok(McpsDataConfirm {
            msdu_handle,
            timestamp: None,
        })
    }

    async fn mcps_data_indication(&mut self) -> Result<McpsDataIndication, MacError> {
        self.mcps_data_indication_timeout(1_000_000).await
    }

    async fn mcps_data_indication_timeout(
        &mut self,
        timeout_us: u32,
    ) -> Result<McpsDataIndication, MacError> {
        // Frames retained during ACK/poll/association waits come first.
        while let Some(entry) = self.pending_rx.pop_front() {
            if let Some(indication) = self.indication_from(entry.frame(), entry.lqi) {
                return Ok(indication);
            }
        }

        // Absolute deadline — filtered frames don't reset the clock
        let deadline =
            embassy_time::Instant::now() + embassy_time::Duration::from_micros(timeout_us as u64);

        loop {
            let now = embassy_time::Instant::now();
            if now >= deadline {
                return Err(MacError::NoData);
            }
            let remaining = deadline - now;

            let mut rx_pkt = Packet::new();
            let rx_result =
                select::select(self.radio.receive(&mut rx_pkt), Timer::after(remaining)).await;

            match rx_result {
                select::Either::Second(_) => {
                    log::debug!("[nRF RX] Timeout ({}us) — no frame", timeout_us);
                    return Err(MacError::NoData);
                }
                select::Either::First(Err(_)) => {
                    // CRC failure or radio error — discard and keep listening
                    continue;
                }
                select::Either::First(Ok(())) => {}
            }

            let lqi = crate::lqi::nrf_from_hardware(rx_pkt.lqi());
            let data = rx_pkt.as_ref();
            // ACK only AR frames addressed exactly to us, before any further
            // processing so the turnaround stays short.
            self.acknowledge_if_required(data).await;
            if let Some(indication) = self.indication_from(data, lqi) {
                log::debug!("[nRF RX] Accepted frame {} bytes, LQI {}", data.len(), lqi);
                return Ok(indication);
            }
        }
    }

    fn capabilities(&self) -> MacCapabilities {
        MacCapabilities::non_parent(102, TxPower(-20), TxPower(8)) // nRF52840: -20 to +8 dBm
    }
}

impl<T: RadioInstance, R: RngInstance> zigbee_crypto::ForwardAesProvider for NrfMac<'_, T, R> {
    fn forward_cipher(
        &mut self,
        key: &zigbee_crypto::AesKey,
    ) -> impl zigbee_crypto::Aes128Forward + '_ {
        NrfHardwareAes128 {
            engine: self.aes.engine_mut(),
            key: *key,
        }
    }
}
impl<T: RadioInstance, R: RngInstance> PlatformServices for NrfMac<'_, T, R> {
    fn monotonic_micros(&self) -> u32 {
        embassy_time::Instant::now().as_micros() as u32
    }

    async fn delay_micros(&mut self, duration_us: u32) {
        Timer::after_micros(duration_us as u64).await;
    }

    fn fill_random(&mut self, output: &mut [u8]) -> Result<(), MacError> {
        self.rng.blocking_fill_bytes(output);
        Ok(())
    }
}

impl<T: RadioInstance, R: RngInstance> NrfMac<'_, T, R> {
    /// Association exchange body for [`MacDriver::mlme_associate`]; the
    /// caller owns `macPANId` setup and rollback.
    async fn associate_with(
        &mut self,
        req: MlmeAssociateRequest,
    ) -> Result<MlmeAssociateConfirm, MacError> {
        match req.coord_address {
            MacAddress::Short(_, address) => {
                self.coord_short_address = address;
                self.coord_extended_address = [0; 8];
            }
            MacAddress::Extended(_, address) => {
                self.coord_short_address = ShortAddress::COORDINATOR;
                self.coord_extended_address = address;
            }
        }

        // Build Association Request command frame
        let frame = build_association_request(
            self.next_dsn(),
            &req.coord_address,
            &self.extended_address,
            &req.capability_info,
        );

        // IEEE 802.15.4 §6.4.1: the association request is an acknowledged
        // MAC command. A bare CCA-gated transmit neither retries a busy
        // channel nor confirms the coordinator actually heard us, so a lost
        // request was previously indistinguishable from a coordinator that
        // never answered — the join then burned all five poll attempts
        // waiting for a response to a frame that was never delivered.
        self.send_acknowledged_frame(&frame, self.max_frame_retries)
            .await?;

        // Per IEEE 802.15.4 §5.3.2.1: wait, then poll with Data Request.
        // Poll multiple times — the coordinator may need time to process.
        for poll_attempt in 0..5u8 {
            // First poll after 200ms, subsequent after 500ms
            let delay = if poll_attempt == 0 { 200 } else { 500 };
            Timer::after_millis(delay).await;

            // Send Data Request to poll for indirect Association Response.
            // CSMA-CA backoff only; the ACK window is deliberately not consumed
            // here so the coordinator's indirect Association Response is left
            // for wait_assoc_response below.
            let data_req =
                build_data_request(self.next_dsn(), &req.coord_address, &self.extended_address);
            let mut dreq_pkt = Packet::new();
            dreq_pkt.copy_from_slice(&data_req);
            let _ = self.try_send_with_csma(&mut dreq_pkt).await;

            // Wait up to 1.5s per poll for Association Response
            let timeout_us: u64 = 1_500_000;

            let result =
                select::select(Timer::after_micros(timeout_us), self.wait_assoc_response()).await;

            match result {
                select::Either::Second(Ok(confirm)) => {
                    if confirm.status == AssociationStatus::Success {
                        // Outside the per-poll timer: cancelling this capture
                        // must never discard an already received confirm.
                        self.capture_post_association_frames().await;
                    }
                    return Ok(confirm);
                }
                select::Either::Second(Err(e)) => return Err(e),
                select::Either::First(_) => {
                    // Timeout — try polling again
                    continue;
                }
            }
        }

        Err(MacError::NoAck)
    }

    /// After a successful Association Response, stay in RX to catch the
    /// Transport-Key that may follow directly. Frames for us are ACKed
    /// (without the ACK the coordinator retries and eventually gives up) and
    /// retained for the poll/indication path. Ends after 200 ms of silence or
    /// 20 frames.
    async fn capture_post_association_frames(&mut self) {
        for _ in 0..20u8 {
            let mut pkt = Packet::new();
            match select::select(Timer::after_millis(200), self.radio.receive(&mut pkt)).await {
                select::Either::Second(Ok(())) => {
                    let lqi = crate::lqi::nrf_from_hardware(pkt.lqi());
                    let data = pkt.as_ref();
                    self.handle_out_of_band(data, lqi).await;
                    if self.is_poll_response(data) {
                        log::info!("[MAC] Caught post-association frame {} bytes", data.len());
                    }
                }
                select::Either::Second(Err(_)) => {}
                select::Either::First(()) => break, // quiet: no more frames
            }
        }
    }

    /// Receive until our Association Response arrives; the caller's timer
    /// bounds the wait.
    ///
    /// Only frames addressed exactly to us with AR set are acknowledged. Data
    /// frames for us (the APS Transport-Key can overtake the confirm) are
    /// retained for the poll/indication path. Unrelated traffic on a busy
    /// channel does not consume a frame budget: only the timer, or a radio
    /// that fails back-to-back, ends the wait.
    async fn wait_assoc_response(&mut self) -> Result<MlmeAssociateConfirm, MacError> {
        const MAX_CONSECUTIVE_RADIO_ERRORS: u32 = 64;
        let mut consecutive_errors = 0u32;
        loop {
            let mut pkt = Packet::new();
            if self.radio.receive(&mut pkt).await.is_err() {
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_RADIO_ERRORS {
                    return Err(MacError::RadioError);
                }
                continue;
            }
            consecutive_errors = 0;
            let lqi = crate::lqi::nrf_from_hardware(pkt.lqi());
            let data = pkt.as_ref();
            if data.len() < 5 {
                continue;
            }
            let fc = u16::from_le_bytes([data[0], data[1]]);
            match fc & 0x07 {
                0x01 => {
                    self.handle_out_of_band(data, lqi).await;
                    if self.is_poll_response(data) {
                        log::info!(
                            "[MAC] Retained data frame {} bytes during association",
                            data.len()
                        );
                    }
                    continue;
                }
                0x03 => {}
                _ => continue,
            }

            let Some((short_addr, status_byte)) = frames::parse_association_response(data) else {
                // Some other command; ACK it if it was for us.
                self.acknowledge_if_required(data).await;
                continue;
            };
            // Another joiner's response on the same channel is not ours.
            if !matches!(
                parse_dest_address(data, fc),
                Some(MacAddress::Extended(_, address)) if address == self.extended_address
            ) {
                continue;
            }
            self.acknowledge_if_required(data).await;

            let status = match status_byte {
                0x00 => AssociationStatus::Success,
                0x01 => AssociationStatus::PanAtCapacity,
                _ => AssociationStatus::PanAccessDenied,
            };
            if status == AssociationStatus::Success {
                self.short_address = short_addr;
            }
            self.associated_pan_coord = status == AssociationStatus::Success;
            return Ok(MlmeAssociateConfirm {
                short_address: short_addr,
                status,
            });
        }
    }
}
