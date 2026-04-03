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
        log::info!("[DRV] config: ch={}", self.config.channel);
    }

    /// Transmit a raw 802.15.4 frame (synchronous).
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        log::info!("[DRV] TX {} bytes on ch{}", frame.len(), self.config.channel);
        self.driver.transmit_raw(frame)
    }

    /// Put radio into receive mode.
    pub fn start_receive(&mut self) {
        self.driver.start_receive();
    }

    /// Poll for a received frame. Returns None if nothing available yet.
    /// Returns the RAW frame (including MAC header) for proper parsing.
    pub fn poll_receive(&mut self) -> Option<Result<RxFrame, Error>> {
        // Use raw_received to get the full MAC frame including header
        match self.driver.raw_received() {
            Some(raw) => {
                let mut rx = RxFrame {
                    data: [0u8; 127],
                    len: 0,
                    lqi: 0,
                };
                // raw.data[0] = PHR (length including FCS)
                // raw.data[1..] = PSDU (MAC frame WITHOUT FCS — CRC not in buffer)
                let phr = raw.data[0] as usize;
                let mac_len = if phr >= 2 { phr - 2 } else { 0 }; // subtract FCS
                let len = mac_len.min(125);
                if len > 0 {
                    rx.data[..len].copy_from_slice(&raw.data[1..][..len]);
                }
                rx.len = len;
                // RSSI is at raw.data[phr-1] (last byte before where FCS would be)
                // Actually unclear — use 0 for now
                rx.lqi = 128; // default mid-range LQI
                Some(Ok(rx))
            }
            None => None,
        }
    }
}
