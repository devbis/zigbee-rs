//! TS011F flash partitions and Zigbee security/NV storage wiring.

use embedded_storage::nor_flash::{ErrorType, NorFlash, ReadNorFlash};
use tlsr8258_hal::flash::{FlashError, Tlsr8258Flash};
use zigbee_runtime::log_nv::LogStructuredNv;
use zigbee_runtime::security_journal::{SECURITY_JOURNAL_SECTOR_SIZE, SecurityStateJournal};

// ── Flash map (board setting) ──
//
// Which flash size the board has is selected by this crate's Cargo features
// (`flash-512k` default / `flash-1m`). These offsets MUST match the linker
// script chosen in build.rs for the same feature. Partitions must lie within
// the physical flash — on a 512 KiB part, addresses >= 0x80000 alias back into
// the firmware image and corrupt code when written.
#[cfg(all(feature = "flash-512k", feature = "flash-1m"))]
compile_error!("tlsr8258-ts011f: enable exactly one of `flash-512k` / `flash-1m`, not both");
#[cfg(not(any(feature = "flash-512k", feature = "flash-1m")))]
compile_error!("tlsr8258-ts011f: enable a flash size feature (`flash-512k` or `flash-1m`)");

#[cfg(all(feature = "flash-1m", not(feature = "flash-512k")))]
mod flash_map {
    pub const FLASH_CAPACITY: usize = 1024 * 1024;
    pub const NV_PARTITION_START: u32 = 0x000E_0000;
    pub const SECURITY_PARTITION_START: u32 = 0x000F_0000;
}
#[cfg(all(feature = "flash-512k", not(feature = "flash-1m")))]
mod flash_map {
    pub const FLASH_CAPACITY: usize = 512 * 1024;
    pub const NV_PARTITION_START: u32 = 0x0007_6000;
    pub const SECURITY_PARTITION_START: u32 = 0x0007_8000;
}
use flash_map::{FLASH_CAPACITY, NV_PARTITION_START, SECURITY_PARTITION_START};

// ── Security journal partition ──
const SECURITY_PARTITION_SIZE: usize = SECURITY_JOURNAL_SECTOR_SIZE * 2;
const SECURITY_SECTOR_A: u32 = 0;
const SECURITY_SECTOR_B: u32 = SECURITY_JOURNAL_SECTOR_SIZE as u32;

const _: () =
    assert!(SECURITY_PARTITION_START as usize + SECURITY_PARTITION_SIZE <= FLASH_CAPACITY);

// ── NV storage partition (energy persistence) ──
const NV_SECTOR_SIZE: u32 = 4096;
const NV_NUM_SECTORS: u32 = 2;
const NV_PARTITION_SIZE: u32 = NV_SECTOR_SIZE * NV_NUM_SECTORS;
const NV_SECTOR_A: u32 = 0;
const NV_SECTOR_B: u32 = NV_SECTOR_SIZE;

const _: () =
    assert!(NV_PARTITION_START + NV_PARTITION_SIZE <= SECURITY_PARTITION_START);

pub struct SecurityFlash {
    flash: Tlsr8258Flash,
}

impl SecurityFlash {
    const fn new() -> Self {
        Self {
            flash: Tlsr8258Flash::new(FLASH_CAPACITY),
        }
    }

    fn physical_offset(offset: u32, length: usize) -> Result<u32, FlashError> {
        (offset as usize)
            .checked_add(length)
            .filter(|end| *end <= SECURITY_PARTITION_SIZE)
            .ok_or(FlashError::AddressOverflow)?;
        SECURITY_PARTITION_START
            .checked_add(offset)
            .ok_or(FlashError::AddressOverflow)
    }
}

impl ErrorType for SecurityFlash {
    type Error = FlashError;
}

impl ReadNorFlash for SecurityFlash {
    const READ_SIZE: usize = Tlsr8258Flash::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let physical = Self::physical_offset(offset, bytes.len())?;
        self.flash.read(physical, bytes)
    }

    fn capacity(&self) -> usize {
        SECURITY_PARTITION_SIZE
    }
}

impl NorFlash for SecurityFlash {
    const WRITE_SIZE: usize = Tlsr8258Flash::WRITE_SIZE;
    const ERASE_SIZE: usize = Tlsr8258Flash::ERASE_SIZE;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if from >= to {
            return Err(FlashError::AddressOverflow);
        }
        let length = usize::try_from(to - from).map_err(|_| FlashError::AddressOverflow)?;
        let physical_from = Self::physical_offset(from, length)?;
        let physical_to = physical_from
            .checked_add(to - from)
            .ok_or(FlashError::AddressOverflow)?;
        self.flash.erase(physical_from, physical_to)
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let physical = Self::physical_offset(offset, bytes.len())?;
        self.flash.write(physical, bytes)
    }
}

pub type SecurityStore = SecurityStateJournal<SecurityFlash>;

pub const fn security_store() -> SecurityStore {
    SecurityStateJournal::new(SecurityFlash::new(), SECURITY_SECTOR_A, SECURITY_SECTOR_B)
}

// ── NV storage (energy persistence) ──

pub struct NvFlash {
    flash: Tlsr8258Flash,
}

impl NvFlash {
    const fn new() -> Self {
        Self {
            flash: Tlsr8258Flash::new(FLASH_CAPACITY),
        }
    }

    fn physical_offset(offset: u32, length: usize) -> Result<u32, FlashError> {
        (offset as usize)
            .checked_add(length)
            .filter(|end| *end <= NV_PARTITION_SIZE as usize)
            .ok_or(FlashError::AddressOverflow)?;
        NV_PARTITION_START
            .checked_add(offset)
            .ok_or(FlashError::AddressOverflow)
    }
}

impl ErrorType for NvFlash {
    type Error = FlashError;
}

impl ReadNorFlash for NvFlash {
    const READ_SIZE: usize = Tlsr8258Flash::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let physical = Self::physical_offset(offset, bytes.len())?;
        self.flash.read(physical, bytes)
    }

    fn capacity(&self) -> usize {
        NV_PARTITION_SIZE as usize
    }
}

impl NorFlash for NvFlash {
    const WRITE_SIZE: usize = Tlsr8258Flash::WRITE_SIZE;
    const ERASE_SIZE: usize = Tlsr8258Flash::ERASE_SIZE;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if from >= to {
            return Err(FlashError::AddressOverflow);
        }
        let length = usize::try_from(to - from).map_err(|_| FlashError::AddressOverflow)?;
        let physical_from = Self::physical_offset(from, length)?;
        let physical_to = physical_from
            .checked_add(to - from)
            .ok_or(FlashError::AddressOverflow)?;
        self.flash.erase(physical_from, physical_to)
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let physical = Self::physical_offset(offset, bytes.len())?;
        self.flash.write(physical, bytes)
    }
}

pub type NvStore = LogStructuredNv<NvFlash>;

pub fn nv_store() -> Result<NvStore, zigbee_runtime::nv_storage::NvError> {
    LogStructuredNv::new(NvFlash::new(), NV_SECTOR_A, NV_SECTOR_B)
}
