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

    /// Transmit a raw 802.15.4 frame (synchronous).
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        self.driver.transmit_raw(frame)
    }

    /// Put radio into receive mode.
    pub fn start_receive(&mut self) {
        self.driver.start_receive();
    }

    /// Poll for a received frame. Returns None if nothing available yet.
    pub fn poll_receive(&mut self) -> Option<Result<RxFrame, Error>> {
        match self.driver.received() {
            Some(Ok(received)) => {
                let mut rx = RxFrame {
                    data: [0u8; 127],
                    len: 0,
                    lqi: received.lqi,
                };
                let frame_data = &received.frame.payload;
                let len = frame_data.len().min(127);
                rx.data[..len].copy_from_slice(&frame_data[..len]);
                rx.len = len;
                Some(Ok(rx))
            }
            Some(Err(e)) => Some(Err(e)),
            None => None,
        }
    }
}
