//! Low-level ESP32 802.15.4 radio driver wrapper.
//!
//! Provides synchronous TX and polling-based RX on top of
//! `esp-radio::ieee802154`. Uses direct radio API calls without
//! depending on interrupt signal wiring.

use esp_radio::ieee802154::{Config, Error, Ieee802154};

/// Received frame data (copied out of radio buffer).
pub struct RxFrame {
    pub data: [u8; 127],
    pub len: usize,
    pub lqi: u8,
}

/// Wrapper around the ESP32 ieee802154 radio peripheral.
pub struct Ieee802154Driver<'a> {
    driver: Ieee802154<'a>,
    config: Config,
}

impl<'a> Ieee802154Driver<'a> {
    pub fn new(mut ieee802154: Ieee802154<'a>, config: Config) -> Self {
        ieee802154.set_config(config);
        Self {
            driver: ieee802154,
            config,
        }
    }

    /// Update radio configuration (channel, PAN ID, short address, etc.)
    pub fn update_config(&mut self, update_fn: impl FnOnce(&mut Config)) {
        update_fn(&mut self.config);
        self.driver.set_config(self.config);
    }

    /// Transmit a raw 802.15.4 frame and wait for TX completion.
    /// The ESP32 802.15.4 hardware interprets buffer[0] as total OTA length
    /// INCLUDING FCS. Since hardware auto-generates FCS, we add +2 to the
    /// length so the full frame data is transmitted.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        // Append 2 dummy bytes — transmit_raw sets PHR = frame.len(),
        // but hardware expects PHR including FCS. The extra bytes are
        // overwritten by hardware-generated CRC on air.
        let mut padded = [0u8; 129];
        padded[..frame.len()].copy_from_slice(frame);
        self.driver.transmit_raw(&padded[..frame.len() + 2])?;
        // Wait for TX to complete before returning.
        // Too short: beacon request gets aborted by subsequent start_receive.
        // Too long: miss ACK from coordinator (ACK arrives ~192µs after our frame).
        // 2ms is a good compromise — TX completes, and ACK wait starts promptly.
        let start = esp_hal::time::Instant::now();
        while start.elapsed() < esp_hal::time::Duration::from_millis(2) {
            core::hint::spin_loop();
        }
        Ok(())
    }

    /// Put radio into receive mode.
    pub fn start_receive(&mut self) {
        self.driver.start_receive();
    }

    /// Poll for a received frame. Returns None if nothing available yet.
    pub fn poll_receive(&mut self) -> Option<Result<RxFrame, Error>> {
        // Try raw_received first for full frame access
        if let Some(raw) = self.driver.raw_received() {
            let mut rx = RxFrame {
                data: [0u8; 127],
                len: 0,
                lqi: 128,
            };
            let phr = raw.data[0] as usize;
            let mac_len = if phr >= 2 { phr - 2 } else { phr };
            let len = mac_len.min(125);
            if len > 0 {
                rx.data[..len].copy_from_slice(&raw.data[1..][..len]);
            }
            rx.len = len;
            return Some(Ok(rx));
        }
        None
    }
}
