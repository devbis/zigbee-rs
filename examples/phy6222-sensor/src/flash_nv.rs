//! Flash-backed NV storage for PHY6222/6252.
//!
//! Uses the last 2 flash sectors (8 KB) for persistent Zigbee state.
//! Implements FlashDriver trait for the shared LogStructuredNv engine.
//!
//! # Flash layout (PHY6222: 512 KB, sector = 4 KB)
//! ```text
//! Sector at 0x7E000: NV page A
//! Sector at 0x7F000: NV page B
//! ```

use zigbee_runtime::log_nv::{FlashDriver, LogStructuredNv};

const NV_PAGE_A: u32 = 0x0007_E000;
const NV_PAGE_B: u32 = 0x0007_F000;

pub struct Phy6222FlashDriver;

impl Phy6222FlashDriver {
    pub fn new() -> Self { Self }
}

impl FlashDriver for Phy6222FlashDriver {
    fn read(&self, offset: u32, buf: &mut [u8]) {
        phy6222_hal::flash::read(offset, buf);
    }

    fn write(&mut self, offset: u32, data: &[u8]) {
        phy6222_hal::flash::write(offset, data);
    }

    fn erase_sector(&mut self, offset: u32) {
        phy6222_hal::flash::erase_sector(offset);
    }

    fn sector_size(&self) -> usize {
        4096
    }
}

pub fn create_nv() -> LogStructuredNv<Phy6222FlashDriver> {
    LogStructuredNv::new(Phy6222FlashDriver::new(), NV_PAGE_A, NV_PAGE_B)
}
