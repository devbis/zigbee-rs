//! Generic log-structured NV storage over raw flash.
//!
//! Platform-independent implementation that works with any flash hardware
//! via the [`FlashDriver`] trait. Each platform provides a thin adapter
//! (~20 lines) implementing read/write/erase for its specific flash controller.
//!
//! # Design
//! Uses 2 flash sectors (pages): one active, one scratch. Items are appended
//! sequentially; the latest entry for each ID wins. When the active page fills,
//! live items are compacted to the scratch page and roles swap.
//!
//! # Item format (4-byte aligned)
//! ```text
//! [magic:2][id:2][len:2][pad:2][data:len][pad to 4B]
//! ```

use crate::nv_storage::{NvError, NvItemId, NvStorage};

/// Raw flash hardware abstraction — implement per platform.
///
/// # Examples
/// - nRF52840: wraps `embassy_nrf::nvmc::Nvmc` (NVMC controller)
/// - PHY6222: wraps `phy6222_hal::flash` (SPIF controller)
/// - ESP32: wraps `esp-storage` flash partition
/// - BL702: wraps vendor `hal_flash_*` functions
pub trait FlashDriver {
    /// Read bytes from flash at the given offset.
    fn read(&self, offset: u32, buf: &mut [u8]);

    /// Write bytes to flash at the given offset.
    /// Data must be 4-byte aligned. Caller ensures no page-boundary crossing.
    fn write(&mut self, offset: u32, data: &[u8]);

    /// Erase one sector at the given offset.
    fn erase_sector(&mut self, offset: u32);

    /// Sector size in bytes (typically 4096).
    fn sector_size(&self) -> usize;
}

/// Magic bytes indicating a valid item header.
const ITEM_MAGIC: u16 = 0xA55A;

/// Header size: magic(2) + id(2) + len(2) + pad(2) = 8 bytes.
const HEADER_SIZE: usize = 8;

/// Generic log-structured NV storage backed by raw flash.
pub struct LogStructuredNv<F: FlashDriver> {
    flash: F,
    /// Flash offset of page A.
    page_a: u32,
    /// Flash offset of page B.
    page_b: u32,
    /// Which page is currently active.
    active_page: u32,
    /// Current write cursor within the active page.
    write_offset: usize,
}

impl<F: FlashDriver> LogStructuredNv<F> {
    /// Create and initialize log-structured NV storage.
    ///
    /// `page_a` and `page_b` are flash offsets for the two NV sectors.
    pub fn new(flash: F, page_a: u32, page_b: u32) -> Self {
        let mut s = Self {
            flash,
            page_a,
            page_b,
            active_page: page_a,
            write_offset: 0,
        };
        s.init();
        s
    }

    fn sector_size(&self) -> usize {
        self.flash.sector_size()
    }

    fn init(&mut self) {
        let a_valid = self.page_has_data(self.page_a);
        let b_valid = self.page_has_data(self.page_b);

        match (a_valid, b_valid) {
            (true, false) => self.active_page = self.page_a,
            (false, true) => self.active_page = self.page_b,
            (true, true) => {
                self.active_page = self.page_a;
                self.flash.erase_sector(self.page_b);
            }
            (false, false) => self.active_page = self.page_a,
        }

        self.write_offset = self.find_write_offset(self.active_page);

        log::debug!(
            "[LogNV] Active=0x{:05X}, offset={}",
            self.active_page,
            self.write_offset
        );
    }

    fn page_has_data(&self, page: u32) -> bool {
        let mut hdr = [0u8; HEADER_SIZE];
        self.flash.read(page, &mut hdr);
        u16::from_le_bytes([hdr[0], hdr[1]]) == ITEM_MAGIC
    }

    fn find_write_offset(&self, page: u32) -> usize {
        let page_size = self.sector_size();
        let mut offset = 0;
        let mut hdr = [0u8; HEADER_SIZE];

        while offset + HEADER_SIZE <= page_size {
            self.flash.read(page + offset as u32, &mut hdr);
            if u16::from_le_bytes([hdr[0], hdr[1]]) != ITEM_MAGIC {
                return offset;
            }
            let len = u16::from_le_bytes([hdr[4], hdr[5]]) as usize;
            offset += HEADER_SIZE + align4(len);
        }
        offset
    }

    fn find_latest(&self, id: NvItemId, buf: &mut [u8]) -> Option<usize> {
        let page_size = self.sector_size();
        let mut offset = 0;
        let mut hdr = [0u8; HEADER_SIZE];
        let mut found_len = None;

        while offset + HEADER_SIZE <= page_size {
            self.flash.read(self.active_page + offset as u32, &mut hdr);
            if u16::from_le_bytes([hdr[0], hdr[1]]) != ITEM_MAGIC {
                break;
            }
            let item_id = u16::from_le_bytes([hdr[2], hdr[3]]);
            let len = u16::from_le_bytes([hdr[4], hdr[5]]) as usize;

            if item_id == id as u16 {
                if len == 0 {
                    found_len = None; // deletion marker
                } else if len <= buf.len() {
                    self.flash.read(
                        self.active_page + (offset + HEADER_SIZE) as u32,
                        &mut buf[..len],
                    );
                    found_len = Some(len);
                }
            }

            offset += HEADER_SIZE + align4(len);
        }

        found_len
    }

    fn append_item(&mut self, id: NvItemId, data: &[u8]) -> Result<(), NvError> {
        let aligned_len = align4(data.len());
        let total = HEADER_SIZE + aligned_len;

        if self.write_offset + total > self.sector_size() {
            self.compact()?;
            if self.write_offset + total > self.sector_size() {
                return Err(NvError::Full);
            }
        }

        let mut write_buf = [0xFFu8; 128 + HEADER_SIZE];
        write_buf[0] = (ITEM_MAGIC & 0xFF) as u8;
        write_buf[1] = (ITEM_MAGIC >> 8) as u8;
        write_buf[2] = (id as u16 & 0xFF) as u8;
        write_buf[3] = (id as u16 >> 8) as u8;
        write_buf[4] = (data.len() as u16 & 0xFF) as u8;
        write_buf[5] = (data.len() as u16 >> 8) as u8;
        write_buf[6] = 0x00;
        write_buf[7] = 0x00;
        if !data.is_empty() {
            write_buf[HEADER_SIZE..HEADER_SIZE + data.len()].copy_from_slice(data);
        }

        self.flash.write(
            self.active_page + self.write_offset as u32,
            &write_buf[..total],
        );
        self.write_offset += total;
        Ok(())
    }

    fn scratch_page(&self) -> u32 {
        if self.active_page == self.page_a {
            self.page_b
        } else {
            self.page_a
        }
    }
}

impl<F: FlashDriver> NvStorage for LogStructuredNv<F> {
    fn read(&self, id: NvItemId, buf: &mut [u8]) -> Result<usize, NvError> {
        self.find_latest(id, buf).ok_or(NvError::NotFound)
    }

    fn write(&mut self, id: NvItemId, data: &[u8]) -> Result<(), NvError> {
        self.append_item(id, data)
    }

    fn delete(&mut self, id: NvItemId) -> Result<(), NvError> {
        self.append_item(id, &[])
    }

    fn exists(&self, id: NvItemId) -> bool {
        let mut buf = [0u8; 128];
        self.find_latest(id, &mut buf).is_some()
    }

    fn item_length(&self, id: NvItemId) -> Result<usize, NvError> {
        let mut buf = [0u8; 128];
        self.find_latest(id, &mut buf).ok_or(NvError::NotFound)
    }

    fn compact(&mut self) -> Result<(), NvError> {
        let scratch = self.scratch_page();
        self.flash.erase_sector(scratch);

        // Collect unique item IDs
        let page_size = self.sector_size();
        let mut seen_ids: heapless::Vec<u16, 32> = heapless::Vec::new();
        let mut offset = 0;
        let mut hdr = [0u8; HEADER_SIZE];

        while offset + HEADER_SIZE <= page_size {
            self.flash.read(self.active_page + offset as u32, &mut hdr);
            if u16::from_le_bytes([hdr[0], hdr[1]]) != ITEM_MAGIC {
                break;
            }
            let item_id = u16::from_le_bytes([hdr[2], hdr[3]]);
            if !seen_ids.contains(&item_id) {
                let _ = seen_ids.push(item_id);
            }
            let len = u16::from_le_bytes([hdr[4], hdr[5]]) as usize;
            offset += HEADER_SIZE + align4(len);
        }

        // Copy latest of each item to scratch
        let old_active = self.active_page;
        self.active_page = scratch;
        self.write_offset = 0;

        let mut data_buf = [0u8; 128];
        for &item_id in seen_ids.iter() {
            self.active_page = old_active;
            if let Some(nv_id) = raw_to_nv_item_id(item_id)
                && let Some(len) = self.find_latest(nv_id, &mut data_buf)
            {
                self.active_page = scratch;
                let _ = self.append_item(nv_id, &data_buf[..len]);
                continue;
            }
            self.active_page = scratch;
        }

        self.active_page = scratch;
        self.flash.erase_sector(old_active);

        log::debug!(
            "[LogNV] Compacted: {} items, offset={}",
            seen_ids.len(),
            self.write_offset
        );

        Ok(())
    }
}

const fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn raw_to_nv_item_id(raw: u16) -> Option<NvItemId> {
    match raw {
        0x0001 => Some(NvItemId::NwkPanId),
        0x0002 => Some(NvItemId::NwkChannel),
        0x0003 => Some(NvItemId::NwkShortAddress),
        0x0004 => Some(NvItemId::NwkExtendedPanId),
        0x0005 => Some(NvItemId::NwkIeeeAddress),
        0x0006 => Some(NvItemId::NwkKey),
        0x0007 => Some(NvItemId::NwkKeySeqNum),
        0x0008 => Some(NvItemId::NwkFrameCounter),
        0x0009 => Some(NvItemId::NwkDepth),
        0x000A => Some(NvItemId::NwkParentAddress),
        0x000B => Some(NvItemId::NwkUpdateId),
        0x0020 => Some(NvItemId::ApsTrustCenterAddress),
        0x0021 => Some(NvItemId::ApsLinkKey),
        0x0022 => Some(NvItemId::ApsBindingTable),
        0x0023 => Some(NvItemId::ApsGroupTable),
        0x0040 => Some(NvItemId::BdbNodeIsOnNetwork),
        0x0041 => Some(NvItemId::BdbCommissioningMode),
        0x0042 => Some(NvItemId::BdbPrimaryChannelSet),
        0x0043 => Some(NvItemId::BdbSecondaryChannelSet),
        0x0044 => Some(NvItemId::BdbCommissioningGroupId),
        0x0100 => Some(NvItemId::AppEndpoint1),
        0x0101 => Some(NvItemId::AppEndpoint2),
        0x0102 => Some(NvItemId::AppEndpoint3),
        _ if raw >= 0x0200 => Some(NvItemId::AppCustomBase),
        _ => None,
    }
}
