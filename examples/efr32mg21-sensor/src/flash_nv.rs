//! Flash-backed NV storage for EFR32MG21.
//!
//! Uses the last 2 flash pages for persistent Zigbee state.
//! Implements FlashDriver trait for the shared LogStructuredNv engine.
//!
//! # Flash layout (EFR32MG21: 512 KB, page = 8 KB)
//! ```text
//! Page at 0x7C000: NV page A (8 KB)
//! Page at 0x7E000: NV page B (8 KB)
//! ```
//!
//! EFR32MG21 (Series 2) has 8 KB flash pages and the MSC peripheral
//! is at a different base address than Series 1.

use zigbee_runtime::log_nv::{FlashDriver, LogStructuredNv};

/// NV page A: near end of 512 KB flash, leaving room for bootloader
const NV_PAGE_A: u32 = 0x0007_C000;
/// NV page B: next 8 KB after page A
const NV_PAGE_B: u32 = 0x0007_E000;

/// EFR32MG21 MSC (Memory System Controller) register base — Series 2.
const MSC_BASE: u32 = 0x4003_0000;
/// MSC write control register.
const MSC_WRITECTRL: u32 = MSC_BASE + 0x008;
/// MSC address register.
const MSC_ADDRB: u32 = MSC_BASE + 0x010;
/// MSC write data register.
const MSC_WDATA: u32 = MSC_BASE + 0x018;
/// MSC status register.
const MSC_STATUS: u32 = MSC_BASE + 0x01C;
/// MSC command register.
const MSC_WRITECMD: u32 = MSC_BASE + 0x00C;

pub struct Efr32FlashDriver;

impl Efr32FlashDriver {
    pub fn new() -> Self { Self }

    fn wait_ready(&self) {
        for _ in 0..100_000u32 {
            let status = unsafe { core::ptr::read_volatile(MSC_STATUS as *const u32) };
            if status & 0x01 != 0 {
                core::hint::spin_loop();
            } else {
                break;
            }
        }
    }
}

impl FlashDriver for Efr32FlashDriver {
    fn read(&self, offset: u32, buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = unsafe { core::ptr::read_volatile((offset + i as u32) as *const u8) };
        }
    }

    fn write(&mut self, offset: u32, data: &[u8]) {
        unsafe {
            core::ptr::write_volatile(MSC_WRITECTRL as *mut u32, 0x01);
        }

        let mut i = 0usize;
        while i < data.len() {
            let mut word = 0xFFFF_FFFFu32;
            for j in 0..4 {
                if i + j < data.len() {
                    word &= !(0xFF << (j * 8));
                    word |= (data[i + j] as u32) << (j * 8);
                }
            }

            self.wait_ready();
            unsafe {
                core::ptr::write_volatile(MSC_ADDRB as *mut u32, offset + i as u32);
                core::ptr::write_volatile(MSC_WRITECMD as *mut u32, 0x08);
                core::ptr::write_volatile(MSC_WDATA as *mut u32, word);
                core::ptr::write_volatile(MSC_WRITECMD as *mut u32, 0x01);
            }

            i += 4;
        }

        self.wait_ready();

        unsafe {
            core::ptr::write_volatile(MSC_WRITECTRL as *mut u32, 0x00);
        }
    }

    fn erase_sector(&mut self, offset: u32) {
        unsafe {
            core::ptr::write_volatile(MSC_WRITECTRL as *mut u32, 0x01);
        }

        self.wait_ready();

        unsafe {
            core::ptr::write_volatile(MSC_ADDRB as *mut u32, offset);
            core::ptr::write_volatile(MSC_WRITECMD as *mut u32, 0x08);
            core::ptr::write_volatile(MSC_WRITECMD as *mut u32, 0x02);
        }

        self.wait_ready();

        unsafe {
            core::ptr::write_volatile(MSC_WRITECTRL as *mut u32, 0x00);
        }
    }

    fn sector_size(&self) -> usize {
        8192 // EFR32MG21 page size = 8 KB
    }
}

pub fn create_nv() -> LogStructuredNv<Efr32FlashDriver> {
    LogStructuredNv::new(Efr32FlashDriver::new(), NV_PAGE_A, NV_PAGE_B)
}
