//! Low-level ESP32-C6 802.15.4 radio driver wrapper.
//!
//! Provides async TX/RX on top of `esp-radio::ieee802154` using Embassy
//! signals for interrupt-driven completion notification.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_radio::ieee802154::{Config, Error, Ieee802154};

static TX_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static RX_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Received frame data (copied out of radio buffer).
pub struct RxFrame {
    pub data: [u8; 127],
    pub len: usize,
    pub lqi: u8,
}

/// Async wrapper around the ESP32-C6 ieee802154 radio peripheral.
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

    /// Update radio configuration (channel, promiscuous mode, etc.)
    pub fn update_config(&mut self, update_fn: impl FnOnce(&mut Config)) {
        update_fn(&mut self.config);
        self.driver.set_config(self.config);
    }

    /// Transmit a raw 802.15.4 frame. Blocks until TX complete interrupt.
    pub async fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        TX_SIGNAL.reset();
        self.driver.transmit_raw(frame)?;
        TX_SIGNAL.wait().await;
        Ok(())
    }

    /// Receive the next 802.15.4 frame. Blocks until RX interrupt fires.
    pub async fn receive(&mut self) -> Result<RxFrame, Error> {
        RX_SIGNAL.reset();
        self.driver.start_receive();

        let received = loop {
            if let Some(frame) = self.driver.received() {
                break frame?;
            }
            RX_SIGNAL.wait().await;
        };

        // Serialize frame back to raw bytes for our owned buffer
        let mut rx = RxFrame {
            data: [0u8; 127],
            len: 0,
            lqi: received.lqi,
        };

        // Use the frame payload as raw data
        let frame_data = &received.frame.payload;
        let len = frame_data.len().min(127);
        rx.data[..len].copy_from_slice(&frame_data[..len]);
        rx.len = len;

        Ok(rx)
    }
}

// Interrupt callbacks — called from the radio ISR
fn _rx_callback() {
    RX_SIGNAL.signal(());
}

fn _tx_callback() {
    TX_SIGNAL.signal(());
}
