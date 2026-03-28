//! Low-level CC2340 802.15.4 radio driver via FFI to TI's RCL library.
//!
//! Provides async TX/RX on top of the CC2340R5's IEEE 802.15.4 radio using
//! Embassy signals for interrupt-driven completion notification.
//!
//! The CC2340R5 (Texas Instruments, ARM Cortex-M0+) has a dedicated 2.4 GHz
//! radio controlled through TI's Radio Control Layer (RCL). The RCL submits
//! IEEE 802.15.4 commands to the LRF (Low-level Radio Frontend) which manages
//! the actual RF hardware via precompiled radio firmware patches.
//!
//! # Architecture
//! ```text
//! Cc2340Driver (Rust, async)
//!   ├── FFI calls → rcl_cc23x0r5.a (TI precompiled library)
//!   │     ├── RCL_init / RCL_open / RCL_close
//!   │     ├── RCL_Command_submit / RCL_Command_pend / RCL_Command_stop
//!   │     └── RCL_readRssi
//!   ├── IEEE 802.15.4 commands via RCL_CmdIeeeRxTx
//!   │     ├── rxAction — receive config (PAN filter, auto-ACK)
//!   │     └── txAction — transmit config (frame buffer, CCA)
//!   ├── TX completion: RCL callback → TX_SIGNAL
//!   └── RX completion: RCL callback → RX_SIGNAL
//! ```
//!
//! # Build requirements
//! The downstream firmware crate must link TI's precompiled libraries:
//! ```rust,ignore
//! // build.rs
//! let sdk = env::var("CC2340_SDK_DIR").unwrap();
//! println!("cargo:rustc-link-search={sdk}/source/ti/drivers/rcl/lib/ticlang/m0p");
//! println!("cargo:rustc-link-lib=static=rcl_cc23x0r5");
//! // RF firmware patches
//! println!("cargo:rustc-link-lib=static=pbe_ieee_cc23x0r5");
//! println!("cargo:rustc-link-lib=static=mce_ieee_cc23x0r5");
//! println!("cargo:rustc-link-lib=static=rfe_ieee_cc23x0r5");
//! ```

use core::sync::atomic::{AtomicBool, AtomicI8, AtomicU8, Ordering};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

// ── RCL FFI types ───────────────────────────────────────────────
// These mirror the C types from TI's RCL headers. We use opaque
// pointers where possible to avoid reproducing full struct layouts.

/// Opaque RCL client handle (pointer to RCL_Client in C)
pub type RclHandle = *mut u8;

/// RCL command status (subset of values we care about)
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RclCommandStatus {
    Idle = 0x0000,
    Active = 0x0001,
    Finished = 0x0101,
    ChannelBusy = 0x0801,
    NoAck = 0x0802,
    RxErr = 0x0803,
    Error = 0x0F00,
}

/// RCL stop type
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum RclStopType {
    Graceful = 0,
    Hard = 1,
}

/// TX power in dBm with fractional part
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RclTxPower {
    pub dbm: i8,
    pub fraction: u8,
}

// ── FFI bindings to TI RCL library ──────────────────────────────
// These map to functions in rcl_cc23x0r5.a. The library is provided
// as a pre-compiled static archive by Texas Instruments.

unsafe extern "C" {
    // RCL core API
    pub fn RCL_init() -> ();
    pub fn RCL_open(client: *mut u8, config: *const u8) -> RclHandle;
    pub fn RCL_close(handle: RclHandle) -> ();
    pub fn RCL_Command_submit(handle: RclHandle, cmd: *mut u8) -> u16;
    pub fn RCL_Command_pend(cmd: *mut u8) -> u16;
    pub fn RCL_Command_stop(cmd: *mut u8, stop_type: u32) -> u16;
    pub fn RCL_readRssi(handle: RclHandle) -> i8;

    // MAC platform functions from TI's Zigbee platform shim
    // These implement the radio operations at a higher level than raw RCL.
    pub fn mac_ti23xx_radio_init(enable: u8) -> ();
    pub fn mac_ti23xx_set_channel(page: u8, channel_num: u8) -> i32;
    pub fn mac_ti23xx_24_set_tx_power(tx_power_dbm: u8) -> ();
    pub fn mac_ti23xx_trans_set_rx_on_off(enable: u32) -> ();
    pub fn mac_ti23xx_set_ieee_addr(addr: *const u8) -> ();
    pub fn mac_ti23xx_send_packet(mhr_len: u8, buf: u8, wait_type: u8) -> ();
    pub fn mac_ti23xx_perform_cca(rssi: *mut i8) -> i32;
    pub fn mac_ti23xx_set_promiscuous_mode(mode: u8) -> ();
    pub fn mac_ti23xx_src_match_add_short_addr(index: u8, short_addr: u16) -> u8;
    pub fn mac_ti23xx_src_match_delete_short_addr(index: u8) -> u8;
    pub fn mac_ti23xx_src_match_tbl_drop() -> ();
    pub fn mac_ti23xx_trans_rec_pkt(buf: *mut u8) -> ();
    pub fn mac_ti23xx_get_radio_data_status(status_type: u8) -> u8;
    pub fn mac_ti23xx_clear_radio_data_status(status_type: u8) -> ();
    pub fn mac_ti23xx_abort_tx() -> ();
    pub fn mac_ti23xx_enable_rx() -> ();
    pub fn mac_ti23xx_get_sync_rssi() -> i8;
    pub fn mac_ti23xx_set_cca_rssi_threshold(rssi: i8) -> ();
}

// ── Async signals for interrupt-driven TX/RX ────────────────────

/// Signal raised by TX completion callback
static TX_SIGNAL: Signal<CriticalSectionRawMutex, TxResult> = Signal::new();

/// Signal raised by RX completion callback
static RX_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Whether a received frame is waiting in the RX buffer
static RX_PENDING: AtomicBool = AtomicBool::new(false);

/// Last TX status
static LAST_TX_STATUS: AtomicU8 = AtomicU8::new(0);

/// Last RSSI value from received packet
static LAST_RX_RSSI: AtomicI8 = AtomicI8::new(-128);

/// Last LQI value from received packet
static LAST_RX_LQI: AtomicU8 = AtomicU8::new(0);

// ── RX buffer ───────────────────────────────────────────────────

/// Maximum IEEE 802.15.4 frame size
const MAX_FRAME_LEN: usize = 127;

/// Static RX frame buffer — filled by interrupt, consumed by async reader
static mut RX_BUF: [u8; MAX_FRAME_LEN + 2] = [0u8; MAX_FRAME_LEN + 2];
static RX_BUF_LEN: AtomicU8 = AtomicU8::new(0);

// ── TX result ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum TxResult {
    Success,
    ChannelBusy,
    NoAck,
    Error,
}

// ── Callback functions (called from C interrupt context) ────────

/// TX done callback — called from RCL/MAC interrupt context.
/// Wakes the async TX signal.
#[unsafe(no_mangle)]
pub extern "C" fn cc2340_tx_done_callback(status: u8) {
    let result = match status {
        0 => TxResult::Success,
        1 => TxResult::ChannelBusy,
        2 => TxResult::NoAck,
        _ => TxResult::Error,
    };
    LAST_TX_STATUS.store(status, Ordering::Release);
    TX_SIGNAL.signal(result);
}

/// RX done callback — called from RCL/MAC interrupt context.
/// Copies the received frame into the static buffer and wakes async reader.
#[unsafe(no_mangle)]
pub extern "C" fn cc2340_rx_done_callback(data: *const u8, len: u8, rssi: i8, lqi: u8) {
    if len == 0 || len > MAX_FRAME_LEN as u8 || data.is_null() {
        return;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            data,
            core::ptr::addr_of_mut!(RX_BUF) as *mut u8,
            len as usize,
        );
    }
    RX_BUF_LEN.store(len, Ordering::Release);
    LAST_RX_RSSI.store(rssi, Ordering::Release);
    LAST_RX_LQI.store(lqi, Ordering::Release);
    RX_PENDING.store(true, Ordering::Release);
    RX_SIGNAL.signal(());
}

// ── Driver configuration ────────────────────────────────────────

/// Radio configuration for the CC2340 driver.
#[derive(Debug, Clone)]
pub struct RadioConfig {
    pub channel: u8,
    pub pan_id: u16,
    pub short_addr: u16,
    pub ieee_addr: [u8; 8],
    pub tx_power_dbm: i8,
    pub rx_on_when_idle: bool,
    pub promiscuous: bool,
    pub auto_ack: bool,
}

impl Default for RadioConfig {
    fn default() -> Self {
        Self {
            channel: 11,
            pan_id: 0xFFFF,
            short_addr: 0xFFFF,
            ieee_addr: [0u8; 8],
            tx_power_dbm: 0,
            rx_on_when_idle: false,
            promiscuous: false,
            auto_ack: true,
        }
    }
}

/// Radio error type
#[derive(Debug, Clone, Copy)]
pub enum RadioError {
    TxFailed,
    ChannelBusy,
    NoAck,
    Timeout,
    HardwareError,
}

// ── Received frame ──────────────────────────────────────────────

/// A received IEEE 802.15.4 frame with metadata.
pub struct RxFrame {
    pub data: [u8; MAX_FRAME_LEN],
    pub len: usize,
    pub rssi: i8,
    pub lqi: u8,
}

// ── CC2340 Driver ───────────────────────────────────────────────

/// Low-level CC2340 802.15.4 radio driver.
///
/// Wraps TI's RCL library via FFI, providing async TX/RX through
/// Embassy signals. The RCL manages the radio hardware including
/// channel programming, auto-ACK, frame filtering, and CCA.
pub struct Cc2340Driver {
    config: RadioConfig,
    initialized: bool,
}

impl Cc2340Driver {
    /// Create a new CC2340 driver with the given configuration.
    pub fn new(config: RadioConfig) -> Self {
        Self {
            config,
            initialized: false,
        }
    }

    /// Initialize the radio hardware via RCL.
    pub fn init(&mut self) {
        if self.initialized {
            return;
        }
        unsafe {
            mac_ti23xx_radio_init(1);
        }
        self.apply_config();
        self.initialized = true;
        log::info!(
            "[CC2340 DRV] Radio initialized on channel {}",
            self.config.channel
        );
    }

    /// Deinitialize the radio hardware.
    pub fn deinit(&mut self) {
        if !self.initialized {
            return;
        }
        unsafe {
            mac_ti23xx_radio_init(0);
        }
        self.initialized = false;
    }

    /// Apply current configuration to the radio hardware.
    fn apply_config(&self) {
        unsafe {
            mac_ti23xx_set_channel(0, self.config.channel);
            mac_ti23xx_24_set_tx_power(self.config.tx_power_dbm as u8);
            mac_ti23xx_set_ieee_addr(self.config.ieee_addr.as_ptr());
            mac_ti23xx_trans_set_rx_on_off(if self.config.rx_on_when_idle { 1 } else { 0 });
            mac_ti23xx_set_promiscuous_mode(if self.config.promiscuous { 1 } else { 0 });
        }
    }

    /// Update radio configuration with a closure.
    pub fn update_config<F: FnOnce(&mut RadioConfig)>(&mut self, f: F) {
        f(&mut self.config);
        if self.initialized {
            self.apply_config();
        }
    }

    /// Set the 802.15.4 channel (11–26).
    pub fn set_channel(&mut self, channel: u8) {
        self.config.channel = channel;
        if self.initialized {
            unsafe {
                mac_ti23xx_set_channel(0, channel);
            }
        }
    }

    /// Set PAN ID. Updated via the full config apply since TI's API
    /// uses update_rx_panconfig which is a higher-level function.
    pub fn set_pan_id(&mut self, pan_id: u16) {
        self.config.pan_id = pan_id;
    }

    /// Set short address.
    pub fn set_short_addr(&mut self, addr: u16) {
        self.config.short_addr = addr;
    }

    /// Set IEEE (extended) address.
    pub fn set_ieee_addr(&mut self, addr: &[u8; 8]) {
        self.config.ieee_addr = *addr;
        if self.initialized {
            unsafe {
                mac_ti23xx_set_ieee_addr(addr.as_ptr());
            }
        }
    }

    /// Set TX power in dBm.
    pub fn set_tx_power(&mut self, dbm: i8) {
        self.config.tx_power_dbm = dbm;
        if self.initialized {
            unsafe {
                mac_ti23xx_24_set_tx_power(dbm as u8);
            }
        }
    }

    /// Enable or disable RX when idle.
    pub fn set_rx_on_when_idle(&mut self, on: bool) {
        self.config.rx_on_when_idle = on;
        if self.initialized {
            unsafe {
                mac_ti23xx_trans_set_rx_on_off(if on { 1 } else { 0 });
            }
        }
    }

    /// Perform a Clear Channel Assessment.
    /// Returns the RSSI value if the channel is clear, or an error if busy.
    pub fn perform_cca(&self) -> Result<i8, RadioError> {
        let mut rssi: i8 = 0;
        let ret = unsafe { mac_ti23xx_perform_cca(&mut rssi) };
        if ret == 0 {
            Ok(rssi)
        } else {
            Err(RadioError::ChannelBusy)
        }
    }

    /// Read current RSSI from the radio.
    pub fn read_rssi(&self) -> i8 {
        unsafe { mac_ti23xx_get_sync_rssi() }
    }

    /// Transmit a raw IEEE 802.15.4 frame.
    ///
    /// The frame should be a complete MAC frame (FC + Seq + Addressing + Payload).
    /// CRC is appended by hardware.
    ///
    /// This is async — waits for the TX complete interrupt via Embassy signal.
    pub async fn transmit(&self, frame: &[u8]) -> Result<(), RadioError> {
        if frame.is_empty() || frame.len() > MAX_FRAME_LEN {
            return Err(RadioError::TxFailed);
        }

        // Reset TX signal before starting
        TX_SIGNAL.reset();

        // Copy frame to a static TX buffer for the C library
        static mut TX_BUF: [u8; MAX_FRAME_LEN + 1] = [0u8; MAX_FRAME_LEN + 1];
        unsafe {
            TX_BUF[0] = frame.len() as u8;
            TX_BUF[1..=frame.len()].copy_from_slice(frame);
        }

        // Submit TX via the MAC platform shim
        // The mac_ti23xx_send_packet handles CSMA-CA and auto-ACK internally
        // mhr_len is used by TI's stack for header/payload split
        let mhr_len = Self::compute_mhr_len(frame);
        unsafe {
            mac_ti23xx_send_packet(mhr_len, 0, 0);
        }

        // Wait for TX completion signal from interrupt
        let result = TX_SIGNAL.wait().await;

        match result {
            TxResult::Success => Ok(()),
            TxResult::ChannelBusy => Err(RadioError::ChannelBusy),
            TxResult::NoAck => Err(RadioError::NoAck),
            TxResult::Error => Err(RadioError::TxFailed),
        }
    }

    /// Receive the next incoming IEEE 802.15.4 frame.
    ///
    /// Blocks until a frame arrives via the RX interrupt signal.
    pub async fn receive(&self) -> Result<RxFrame, RadioError> {
        // If a frame is already pending, consume it immediately
        let was_pending = RX_PENDING.load(Ordering::Acquire);
        RX_PENDING.store(false, Ordering::Release);
        if was_pending {
            return self.consume_rx_frame();
        }

        // Enable RX if not already on
        unsafe {
            mac_ti23xx_enable_rx();
        }

        // Wait for RX signal
        RX_SIGNAL.reset();
        RX_SIGNAL.wait().await;
        RX_PENDING.store(false, Ordering::Release);

        self.consume_rx_frame()
    }

    /// Consume the current RX buffer into an RxFrame.
    fn consume_rx_frame(&self) -> Result<RxFrame, RadioError> {
        let len = RX_BUF_LEN.load(Ordering::Acquire) as usize;
        if len == 0 {
            return Err(RadioError::Timeout);
        }

        let mut frame = RxFrame {
            data: [0u8; MAX_FRAME_LEN],
            len,
            rssi: LAST_RX_RSSI.load(Ordering::Acquire),
            lqi: LAST_RX_LQI.load(Ordering::Acquire),
        };

        unsafe {
            let src = core::ptr::addr_of!(RX_BUF) as *const u8;
            core::ptr::copy_nonoverlapping(src, frame.data.as_mut_ptr(), len);
        }

        RX_BUF_LEN.store(0, Ordering::Release);
        Ok(frame)
    }

    /// Compute MAC header length from frame control field.
    fn compute_mhr_len(frame: &[u8]) -> u8 {
        if frame.len() < 2 {
            return 0;
        }
        let fc = u16::from_le_bytes([frame[0], frame[1]]);
        // MHR = FC(2) + Seq(1) + addressing fields
        let addr_len = super::addressing_size(fc);
        (3 + addr_len) as u8
    }

    /// Cancel any in-progress TX operation.
    pub fn abort_tx(&self) {
        unsafe {
            mac_ti23xx_abort_tx();
        }
    }
}
