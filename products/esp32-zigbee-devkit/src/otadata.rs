//! ESP-IDF `otadata` encoding, validation and boot-slot selection.
//!
//! The `otadata` partition holds two redundant 4 KiB sectors, each starting
//! with one 32-byte `esp_ota_select_entry_t`:
//!
//! ```text
//! offset  size  field
//!      0     4  ota_seq      (u32, little endian)
//!      4    20  seq_label
//!     24     4  ota_state    (u32, little endian)
//!     28     4  crc          (u32, little endian) — CRC-32 of ota_seq only
//! ```
//!
//! The second stage bootloader prefers the entry with the highest valid sequence
//! number and tries slot `(ota_seq - 1) % 2`. It can fall back to another image
//! WITHOUT rewriting these entries. Metadata never identifies the running slot.
//! An entry counts as valid when its
//! CRC matches and the sequence number is neither `0` nor `0xFFFF_FFFF`
//! (the erased value), which is why a freshly erased `otadata` makes the
//! bootloader fall back to `ota_0` — exactly the state the devkit is in before
//! the first OTA.
//!
//! Writing a new entry always targets the sector that does *not* hold the
//! currently active entry, so a power failure in the middle of an activation
//! leaves the old, still valid, entry untouched.
//!
//! # Pending verification and rollback
//!
//! A freshly staged image is selected with `ESP_OTA_IMG_NEW`, exactly as
//! `esp_ota_set_boot_partition()` does when rollback is enabled. The
//! bootloader espflash ships (ESP-IDF v5.5, default configuration) is built
//! without `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`: it never moves an entry to
//! `PENDING_VERIFY`/`ABORTED` by itself. It does, however, skip entries whose
//! state is `INVALID`/`ABORTED` (`bootloader_common_ota_select_invalid()`),
//! and always prefers the highest valid sequence number. Rollback is therefore
//! driven by the application ([`OtaData::boot_action`]):
//!
//! 1. First boot of an unconfirmed (`NEW`/`PENDING_VERIFY`) entry: program a
//!    boot-attempt mark into the otherwise unused `seq_label`. This only
//!    clears bits, needs no erase and cannot destroy the entry on power loss.
//! 2. The application confirms after it has proven network operation
//!    ([`OtaData::confirmation`]): the entry is rewritten in place as
//!    `VALID`, as `esp_ota_mark_app_valid_cancel_rollback()` does.
//! 3. Booting an unconfirmed entry that already carries the mark means the
//!    previous attempt never confirmed: a `VALID` entry for the previous slot
//!    with a higher sequence number is written into the other sector, and the
//!    abandoned entry is then marked `ABORTED`.
//!
//! The mark survives the in-place `NEW` → `PENDING_VERIFY` → `ABORTED`
//! rewrites of a rollback-enabled ESP-IDF bootloader, which copies the whole
//! entry, so the same flow is correct with either bootloader.

use crate::layout::{OTA_SLOT_COUNT, SECTOR_SIZE};

/// Size of one `esp_ota_select_entry_t`.
pub const ENTRY_SIZE: usize = 32;

/// Number of redundant `otadata` sectors.
pub const SECTOR_COUNT: usize = 2;

/// `ESP_OTA_IMG_NEW` — image written, awaiting first boot.
pub const STATE_NEW: u32 = 0x0000_0000;
/// `ESP_OTA_IMG_PENDING_VERIFY` — first boot done, app must confirm.
pub const STATE_PENDING_VERIFY: u32 = 0x0000_0001;
/// `ESP_OTA_IMG_VALID` — image confirmed, boot unconditionally.
pub const STATE_VALID: u32 = 0x0000_0002;
/// `ESP_OTA_IMG_INVALID` — do not boot this image.
pub const STATE_INVALID: u32 = 0x0000_0003;
/// `ESP_OTA_IMG_ABORTED` — rolled back.
pub const STATE_ABORTED: u32 = 0x0000_0004;
/// `ESP_OTA_IMG_UNDEFINED` — erased flash.
pub const STATE_UNDEFINED: u32 = 0xFFFF_FFFF;

/// Sequence number meaning "erased / no entry".
const SEQ_ERASED: u32 = 0xFFFF_FFFF;

/// Byte offset, inside an entry, of the boot-attempt mark (`seq_label[0..4]`).
///
/// ESP-IDF never interprets `seq_label` and leaves it erased; the CRC covers
/// only `ota_seq`. Programming this word only clears bits, so it is written
/// without an erase and is atomic with respect to the entry's validity.
pub const BOOT_MARK_OFFSET: u32 = 4;

/// Value programmed at [`BOOT_MARK_OFFSET`] on the first boot attempt.
pub const BOOT_MARK: [u8; 4] = [0x00; 4];

/// CRC-32 (IEEE, reflected) of the four little-endian `ota_seq` bytes with the
/// ESP-IDF initial value `0xFFFF_FFFF`.
///
/// This is `esp_rom_crc32_le(UINT32_MAX, &entry->ota_seq, 4)`; the initial
/// value inverts to a zero register and the result is inverted again at the
/// end, which is the same convention as `zlib.crc32(data, 0xFFFFFFFF)`.
pub const fn crc32_seq(seq: u32) -> u32 {
    let bytes = seq.to_le_bytes();
    let mut crc: u32 = 0x0000_0000;
    let mut index = 0;
    while index < bytes.len() {
        crc ^= bytes[index] as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        index += 1;
    }
    crc ^ 0xFFFF_FFFF
}

/// One decoded `otadata` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtaSelectEntry {
    /// Boot sequence number. Slot is `(ota_seq - 1) % OTA_SLOT_COUNT`.
    pub seq: u32,
    /// Free-form label; ESP-IDF leaves it erased.
    pub label: [u8; 20],
    /// Rollback state, only interpreted when the bootloader was built with
    /// rollback support.
    pub state: u32,
    /// CRC-32 of `seq`.
    pub crc: u32,
}

impl OtaSelectEntry {
    /// Build an entry with a correct CRC and an erased label.
    pub const fn new(seq: u32, state: u32) -> Self {
        Self {
            seq,
            label: [0xFF; 20],
            state,
            crc: crc32_seq(seq),
        }
    }

    /// Decode an entry from its 32 raw flash bytes.
    pub fn decode(bytes: &[u8; ENTRY_SIZE]) -> Self {
        let mut label = [0u8; 20];
        label.copy_from_slice(&bytes[4..24]);
        Self {
            seq: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            label,
            state: u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            crc: u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
        }
    }

    /// Encode the entry into its 32 raw flash bytes.
    pub fn encode(&self) -> [u8; ENTRY_SIZE] {
        let mut bytes = [0xFFu8; ENTRY_SIZE];
        bytes[0..4].copy_from_slice(&self.seq.to_le_bytes());
        bytes[4..24].copy_from_slice(&self.label);
        bytes[24..28].copy_from_slice(&self.state.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.crc.to_le_bytes());
        bytes
    }

    /// Whether the bootloader would consider this entry.
    ///
    /// Erased sectors (`seq == 0xFFFF_FFFF`), zero sequence numbers (there is
    /// no slot `(0 - 1) % 2`) and CRC mismatches are all rejected, as are the
    /// two states that explicitly forbid booting the image.
    pub fn is_valid(&self) -> bool {
        self.seq != 0
            && self.seq != SEQ_ERASED
            && self.crc == crc32_seq(self.seq)
            && self.state != STATE_INVALID
            && self.state != STATE_ABORTED
    }

    /// Slot this entry selects, if it is valid.
    pub fn slot(&self) -> Option<u8> {
        self.is_valid()
            .then(|| ((self.seq - 1) % OTA_SLOT_COUNT as u32) as u8)
    }

    /// Whether the image this entry selects still awaits application
    /// confirmation (`NEW` or `PENDING_VERIFY`).
    pub fn is_unconfirmed(&self) -> bool {
        self.state == STATE_NEW || self.state == STATE_PENDING_VERIFY
    }

    /// Whether a previous boot already started this unconfirmed image.
    pub fn boot_attempted(&self) -> bool {
        self.label[..BOOT_MARK.len()] != [0xFF; BOOT_MARK.len()]
    }

    /// The same entry with another state; sequence, label and CRC unchanged.
    pub const fn with_state(self, state: u32) -> Self {
        Self { state, ..self }
    }
}

/// Both redundant entries, in sector order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtaData {
    /// Entry decoded from each `otadata` sector.
    pub entries: [OtaSelectEntry; SECTOR_COUNT],
}

/// A prepared activation: which sector to rewrite and with what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activation {
    /// `otadata` sector index (0 or 1) to erase and rewrite.
    pub sector: u8,
    /// Byte offset of that sector inside the `otadata` partition.
    pub sector_offset: u32,
    /// Entry to program.
    pub entry: OtaSelectEntry,
}

/// What the application must do with `otadata` early in each boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootAction {
    /// The running image is confirmed (or OTA was never used).
    None,
    /// First boot of an unconfirmed image: program [`BOOT_MARK`] into the
    /// entry in `sector`, then run pending verification.
    MarkBootAttempt {
        /// Sector holding the running, unconfirmed entry.
        sector: u8,
    },
    /// The unconfirmed image already had a boot attempt and was never
    /// confirmed: program `select` (the previous slot, `VALID`), then
    /// `abandon` (the unconfirmed entry, `ABORTED`), then reset.
    RollBack {
        /// New entry selecting the previous slot.
        select: Activation,
        /// In-place rewrite marking the unconfirmed entry aborted.
        abandon: Activation,
    },
    /// The bootloader could not start the unconfirmed preferred image and
    /// fell back to the running slot. Record that slot as `VALID` so the
    /// metadata matches what actually runs.
    AdoptRunning(Activation),
}

/// Reasons an activation entry cannot be produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaDataError {
    /// The requested slot does not exist.
    UnknownSlot,
    /// The sequence counter cannot be advanced without wrapping into the
    /// erased/invalid value.
    SequenceExhausted,
}

impl OtaData {
    /// Decode both sectors. `sectors[i]` are the first 32 bytes of sector `i`.
    pub fn decode(sectors: [&[u8; ENTRY_SIZE]; SECTOR_COUNT]) -> Self {
        Self {
            entries: [
                OtaSelectEntry::decode(sectors[0]),
                OtaSelectEntry::decode(sectors[1]),
            ],
        }
    }

    /// Highest valid sequence number, if any entry is valid.
    pub fn max_seq(&self) -> Option<u32> {
        self.entries
            .iter()
            .filter(|entry| entry.is_valid())
            .map(|entry| entry.seq)
            .max()
    }

    /// Index of the sector the bootloader would use, if any.
    pub fn active_sector(&self) -> Option<u8> {
        let max = self.max_seq()?;
        self.entries
            .iter()
            .position(|entry| entry.is_valid() && entry.seq == max)
            .map(|index| index as u8)
    }

    /// Preferred slot, if `otadata` selects one. NOT the executing slot:
    /// bootloader image-validation failure can cause fallback without a rewrite.
    pub fn active_slot(&self) -> Option<u8> {
        let max = self.max_seq()?;
        Some(((max - 1) % OTA_SLOT_COUNT as u32) as u8)
    }

    /// Sector index and entry the bootloader would use, if any.
    pub fn active_entry(&self) -> Option<(u8, OtaSelectEntry)> {
        let sector = self.active_sector()?;
        Some((sector, self.entries[sector as usize]))
    }

    /// Build the `NEW` entry that makes the bootloader select a freshly
    /// staged image in `slot`; the application must later confirm it.
    ///
    /// The entry is placed in the sector that is currently *not* active (an
    /// invalid one first, otherwise the one with the older sequence number),
    /// which keeps the currently bootable entry intact while the new one is
    /// erased and programmed.
    pub fn activation_for(&self, slot: u8) -> Result<Activation, OtaDataError> {
        self.selection_for(slot, STATE_NEW)
    }

    /// Whether `running_slot` executes an image that still awaits
    /// confirmation. No new image may be staged over the rollback target
    /// while this holds.
    pub fn running_unconfirmed(&self, running_slot: u8) -> bool {
        matches!(
            self.active_entry(),
            Some((_, entry)) if entry.slot() == Some(running_slot) && entry.is_unconfirmed()
        )
    }

    /// In-place rewrite that confirms the running image, if it is pending.
    pub fn confirmation(&self, running_slot: u8) -> Option<Activation> {
        if !self.running_unconfirmed(running_slot) {
            return None;
        }
        let (sector, entry) = self.active_entry()?;
        Some(Self::rewrite(sector, entry.with_state(STATE_VALID)))
    }

    /// Decide the boot-time `otadata` action for the executing slot.
    pub fn boot_action(&self, running_slot: u8) -> Result<BootAction, OtaDataError> {
        if running_slot >= OTA_SLOT_COUNT {
            return Err(OtaDataError::UnknownSlot);
        }
        let Some((sector, entry)) = self.active_entry() else {
            return Ok(BootAction::None);
        };
        if !entry.is_unconfirmed() {
            return Ok(BootAction::None);
        }
        if entry.slot() != Some(running_slot) {
            return Ok(BootAction::AdoptRunning(
                self.selection_for(running_slot, STATE_VALID)?,
            ));
        }
        if !entry.boot_attempted() {
            return Ok(BootAction::MarkBootAttempt { sector });
        }
        self.rollback_from(running_slot)
            .map(|(select, abandon)| BootAction::RollBack { select, abandon })
    }

    /// Entries that abandon the unconfirmed running image in favour of the
    /// previous slot. `None` when the running image is not unconfirmed.
    pub fn rollback(&self, running_slot: u8) -> Result<Option<BootAction>, OtaDataError> {
        if !self.running_unconfirmed(running_slot) {
            return Ok(None);
        }
        self.rollback_from(running_slot)
            .map(|(select, abandon)| Some(BootAction::RollBack { select, abandon }))
    }

    fn rollback_from(&self, running_slot: u8) -> Result<(Activation, Activation), OtaDataError> {
        let (sector, entry) = self.active_entry().ok_or(OtaDataError::UnknownSlot)?;
        let previous = (running_slot + 1) % OTA_SLOT_COUNT;
        let select = self.selection_for(previous, STATE_VALID)?;
        debug_assert_ne!(select.sector, sector);
        Ok((
            select,
            Self::rewrite(sector, entry.with_state(STATE_ABORTED)),
        ))
    }

    fn rewrite(sector: u8, entry: OtaSelectEntry) -> Activation {
        Activation {
            sector,
            sector_offset: sector as u32 * SECTOR_SIZE,
            entry,
        }
    }

    fn selection_for(&self, slot: u8, state: u32) -> Result<Activation, OtaDataError> {
        if slot >= OTA_SLOT_COUNT {
            return Err(OtaDataError::UnknownSlot);
        }

        let count = OTA_SLOT_COUNT as u32;
        let mut seq = self.max_seq().unwrap_or(0) + 1;
        while (seq - 1) % count != slot as u32 {
            seq = seq.checked_add(1).ok_or(OtaDataError::SequenceExhausted)?;
        }
        if seq == SEQ_ERASED {
            return Err(OtaDataError::SequenceExhausted);
        }

        Ok(Self::rewrite(
            self.spare_sector(),
            OtaSelectEntry::new(seq, state),
        ))
    }

    /// The sector that may be overwritten without losing the active entry.
    fn spare_sector(&self) -> u8 {
        match (self.entries[0].is_valid(), self.entries[1].is_valid()) {
            (false, _) => 0,
            (true, false) => 1,
            // Both valid: overwrite the older one.
            (true, true) => {
                if self.entries[0].seq <= self.entries[1].seq {
                    0
                } else {
                    1
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ERASED: [u8; ENTRY_SIZE] = [0xFF; ENTRY_SIZE];

    fn entry_bytes(seq: u32, state: u32) -> [u8; ENTRY_SIZE] {
        OtaSelectEntry::new(seq, state).encode()
    }

    #[test]
    fn crc_matches_esp_idf_vectors() {
        // Captured from `bootloader_common_ota_select_crc()` on device.
        assert_eq!(crc32_seq(1), 0x4743_989A);
        assert_eq!(crc32_seq(2), 0x55F6_3774);
        assert_eq!(crc32_seq(3), 0xED4A_5011);
    }

    #[test]
    fn entry_round_trips_through_flash_bytes() {
        let entry = OtaSelectEntry::new(7, STATE_VALID);
        let bytes = entry.encode();
        assert_eq!(&bytes[0..4], &7u32.to_le_bytes());
        assert_eq!(&bytes[4..24], &[0xFFu8; 20]);
        assert_eq!(&bytes[24..28], &STATE_VALID.to_le_bytes());
        assert_eq!(&bytes[28..32], &crc32_seq(7).to_le_bytes());
        assert_eq!(OtaSelectEntry::decode(&bytes), entry);
    }

    #[test]
    fn erased_and_corrupt_entries_are_invalid() {
        assert!(!OtaSelectEntry::decode(&ERASED).is_valid());

        let mut corrupt = entry_bytes(2, STATE_VALID);
        corrupt[28] ^= 0x01;
        assert!(!OtaSelectEntry::decode(&corrupt).is_valid());

        let zero_seq = OtaSelectEntry::new(0, STATE_VALID);
        assert!(!zero_seq.is_valid());
        assert_eq!(zero_seq.slot(), None);

        let aborted = OtaSelectEntry::new(3, STATE_ABORTED);
        assert!(!aborted.is_valid());
        let invalid = OtaSelectEntry::new(3, STATE_INVALID);
        assert!(!invalid.is_valid());
    }

    #[test]
    fn empty_otadata_has_no_preference_and_can_select_either_slot() {
        let data = OtaData::decode([&ERASED, &ERASED]);
        assert_eq!(data.max_seq(), None);
        assert_eq!(data.active_slot(), None);
        assert_eq!(data.active_sector(), None);
        assert_eq!(data.activation_for(0).unwrap().entry.slot(), Some(0));

        let activation = data.activation_for(1).unwrap();
        assert_eq!(activation.sector, 0);
        assert_eq!(activation.sector_offset, 0);
        assert_eq!(activation.entry.seq, 2);
        assert_eq!(activation.entry.slot(), Some(1));
        assert_eq!(activation.entry.state, STATE_NEW);
    }

    #[test]
    fn highest_sequence_number_wins_across_sectors() {
        let low = entry_bytes(4, STATE_VALID);
        let high = entry_bytes(5, STATE_VALID);

        let data = OtaData::decode([&low, &high]);
        assert_eq!(data.max_seq(), Some(5));
        assert_eq!(data.active_sector(), Some(1));
        assert_eq!(data.active_slot(), Some(0));

        let flipped = OtaData::decode([&high, &low]);
        assert_eq!(flipped.active_sector(), Some(0));
        assert_eq!(flipped.active_slot(), Some(0));
    }

    #[test]
    fn one_corrupt_sector_falls_back_to_the_other() {
        let mut corrupt = entry_bytes(9, STATE_VALID);
        corrupt[0] ^= 0xFF;
        let good = entry_bytes(4, STATE_VALID);

        let data = OtaData::decode([&corrupt, &good]);
        assert_eq!(data.max_seq(), Some(4));
        assert_eq!(data.active_slot(), Some(1));

        // The corrupt sector is the one that gets rewritten.
        let activation = data.activation_for(0).unwrap();
        assert_eq!(activation.sector, 0);
        assert_eq!(activation.entry.seq, 5);
        assert_eq!(activation.entry.slot(), Some(0));
    }

    #[test]
    fn activation_is_monotonic_and_never_touches_the_active_sector() {
        // Sector 0 holds seq 5 (slot 0), sector 1 holds seq 4 (slot 1).
        let newest = entry_bytes(5, STATE_VALID);
        let oldest = entry_bytes(4, STATE_VALID);
        let data = OtaData::decode([&newest, &oldest]);
        assert_eq!(data.active_sector(), Some(0));

        let activation = data.activation_for(1).unwrap();
        assert_eq!(activation.sector, 1, "must rewrite the stale sector");
        assert_eq!(activation.sector_offset, SECTOR_SIZE);
        assert!(activation.entry.seq > data.max_seq().unwrap());
        assert_eq!(activation.entry.slot(), Some(1));

        // Applying it selects the new slot.
        let applied = OtaData::decode([&newest, &activation.entry.encode()]);
        assert_eq!(applied.active_slot(), Some(1));
        assert_eq!(applied.active_sector(), Some(1));
    }

    #[test]
    fn re_activating_the_same_slot_skips_a_sequence_number() {
        let current = entry_bytes(5, STATE_VALID); // slot 0
        let data = OtaData::decode([&current, &ERASED]);
        assert_eq!(data.active_slot(), Some(0));

        // Staging into slot 0 again needs seq 7, because seq 6 maps to slot 1.
        let activation = data.activation_for(0).unwrap();
        assert_eq!(activation.entry.seq, 7);
        assert_eq!(activation.entry.slot(), Some(0));
        assert_eq!(activation.sector, 1);
    }

    #[test]
    fn sequence_exhaustion_is_reported() {
        // seq 0xFFFFFFFE selects slot (0xFFFFFFFE - 1) % 2 == 1 and is the last
        // usable value, because 0xFFFFFFFF is the erased marker.
        let last = OtaSelectEntry::new(0xFFFF_FFFE, STATE_VALID).encode();
        let data = OtaData::decode([&last, &ERASED]);
        assert_eq!(data.active_slot(), Some(1));
        assert_eq!(data.activation_for(0), Err(OtaDataError::SequenceExhausted));
        assert_eq!(data.activation_for(2), Err(OtaDataError::UnknownSlot));

        // One step earlier the counter still has room.
        let earlier = OtaSelectEntry::new(0xFFFF_FFFD, STATE_VALID).encode();
        let data = OtaData::decode([&earlier, &ERASED]);
        assert_eq!(data.activation_for(1).unwrap().entry.seq, 0xFFFF_FFFE);
    }

    #[test]
    fn power_loss_before_the_crc_lands_keeps_the_old_entry() {
        let good = entry_bytes(5, STATE_VALID);
        // Torn write: sequence programmed, CRC still erased.
        let mut torn = entry_bytes(6, STATE_VALID);
        torn[28..32].copy_from_slice(&[0xFF; 4]);

        let data = OtaData::decode([&good, &torn]);
        assert_eq!(data.max_seq(), Some(5));
        assert_eq!(data.active_slot(), Some(0));
    }

    fn marked(entry: OtaSelectEntry) -> [u8; ENTRY_SIZE] {
        let mut bytes = entry.encode();
        let at = BOOT_MARK_OFFSET as usize;
        bytes[at..at + BOOT_MARK.len()].copy_from_slice(&BOOT_MARK);
        bytes
    }

    #[test]
    fn boot_mark_only_clears_bits_and_keeps_the_entry_valid() {
        let entry = OtaSelectEntry::new(6, STATE_NEW);
        assert!(!entry.boot_attempted());
        let bytes = marked(entry);
        for (before, after) in entry.encode().iter().zip(bytes.iter()) {
            assert_eq!(after & !before, 0, "the mark must not need an erase");
        }
        let decoded = OtaSelectEntry::decode(&bytes);
        assert!(decoded.boot_attempted());
        assert!(decoded.is_valid());
        assert_eq!(decoded.slot(), entry.slot());
    }

    #[test]
    fn confirmed_or_unused_otadata_needs_no_boot_action() {
        for running in 0..OTA_SLOT_COUNT {
            let empty = OtaData::decode([&ERASED, &ERASED]);
            assert_eq!(empty.boot_action(running), Ok(BootAction::None));
            assert_eq!(empty.confirmation(running), None);

            // Firmware older than pending verification wrote VALID entries.
            let valid = entry_bytes(running as u32 + 1, STATE_VALID);
            let data = OtaData::decode([&valid, &ERASED]);
            assert_eq!(data.boot_action(running), Ok(BootAction::None));
            assert_eq!(data.confirmation(running), None);
            assert!(!data.running_unconfirmed(running));
        }
        let data = OtaData::decode([&ERASED, &ERASED]);
        assert_eq!(data.boot_action(2), Err(OtaDataError::UnknownSlot));
    }

    #[test]
    fn first_boot_of_a_new_image_marks_the_attempt() {
        // Sector 0: previous image (slot 0, seq 1). Sector 1: staged slot 1.
        let previous = entry_bytes(1, STATE_VALID);
        let data = OtaData::decode([&previous, &ERASED]);
        let staged = data.activation_for(1).unwrap();
        assert_eq!(staged.sector, 1);
        assert_eq!(staged.entry.state, STATE_NEW);

        let data = OtaData::decode([&previous, &staged.entry.encode()]);
        assert!(data.running_unconfirmed(1));
        assert_eq!(
            data.boot_action(1),
            Ok(BootAction::MarkBootAttempt { sector: 1 })
        );

        // A rollback-enabled IDF bootloader has already turned NEW into
        // PENDING_VERIFY before the first boot; the action is the same.
        let pending = staged.entry.with_state(STATE_PENDING_VERIFY).encode();
        let data = OtaData::decode([&previous, &pending]);
        assert_eq!(
            data.boot_action(1),
            Ok(BootAction::MarkBootAttempt { sector: 1 })
        );
    }

    #[test]
    fn confirmation_rewrites_the_running_entry_in_place_as_valid() {
        let previous = entry_bytes(1, STATE_VALID);
        let new = OtaSelectEntry::new(2, STATE_NEW);
        let data = OtaData::decode([&previous, &marked(new)]);
        let confirm = data.confirmation(1).expect("pending image confirms");
        assert_eq!(confirm.sector, 1);
        assert_eq!(confirm.sector_offset, SECTOR_SIZE);
        assert_eq!(confirm.entry.seq, 2);
        assert_eq!(confirm.entry.state, STATE_VALID);
        assert!(confirm.entry.boot_attempted(), "label is preserved");

        let confirmed = OtaData::decode([&previous, &confirm.entry.encode()]);
        assert_eq!(confirmed.boot_action(1), Ok(BootAction::None));
        assert_eq!(
            confirmed.confirmation(1),
            None,
            "confirmation is idempotent"
        );
        assert_eq!(confirmed.active_slot(), Some(1));

        // Never confirm on behalf of an image that is not running.
        assert_eq!(data.confirmation(0), None);
    }

    #[test]
    fn unconfirmed_second_boot_rolls_back_to_the_previous_slot() {
        for (previous_slot, previous_seq) in [(0u8, 1u32), (1, 2), (0, 7)] {
            let new_slot = 1 - previous_slot;
            let previous = entry_bytes(previous_seq, STATE_VALID);
            let staged = OtaData::decode([&previous, &ERASED])
                .activation_for(new_slot)
                .unwrap();
            for state in [STATE_NEW, STATE_PENDING_VERIFY] {
                let attempted = marked(staged.entry.with_state(state));
                let data = OtaData::decode([&previous, &attempted]);
                let Ok(BootAction::RollBack { select, abandon }) = data.boot_action(new_slot)
                else {
                    panic!("expected rollback");
                };
                assert_eq!(select.sector, 0, "the unconfirmed entry is untouched");
                assert_eq!(select.entry.state, STATE_VALID);
                assert_eq!(select.entry.slot(), Some(previous_slot));
                assert!(select.entry.seq > staged.entry.seq);
                assert_eq!(abandon.sector, 1);
                assert_eq!(abandon.entry.seq, staged.entry.seq);
                assert_eq!(abandon.entry.state, STATE_ABORTED);

                // Power loss after `select` already rolls back.
                let half = OtaData::decode([&select.entry.encode(), &attempted]);
                assert_eq!(half.active_slot(), Some(previous_slot));
                assert_eq!(half.boot_action(previous_slot), Ok(BootAction::None));

                let done = OtaData::decode([&select.entry.encode(), &abandon.entry.encode()]);
                assert_eq!(done.active_slot(), Some(previous_slot));
                assert_eq!(done.active_sector(), Some(0));

                // Power loss while `select`'s sector is erased: the
                // unconfirmed entry remains, so the next boot retries.
                let torn = OtaData::decode([&ERASED, &attempted]);
                assert!(matches!(
                    torn.boot_action(new_slot),
                    Ok(BootAction::RollBack { .. })
                ));
            }
        }
    }

    #[test]
    fn deadline_rollback_only_applies_to_the_running_unconfirmed_image() {
        let previous = entry_bytes(1, STATE_VALID);
        let new = OtaSelectEntry::new(2, STATE_NEW);
        let data = OtaData::decode([&previous, &marked(new)]);
        assert!(matches!(
            data.rollback(1),
            Ok(Some(BootAction::RollBack { .. }))
        ));
        assert_eq!(data.rollback(0), Ok(None));

        let confirmed = OtaData::decode([&previous, &new.with_state(STATE_VALID).encode()]);
        assert_eq!(confirmed.rollback(1), Ok(None));
    }

    #[test]
    fn bootloader_fallback_from_an_unconfirmed_image_adopts_the_running_slot() {
        // The new slot-1 image failed bootloader validation; slot 0 runs.
        let previous = entry_bytes(1, STATE_VALID);
        let new = entry_bytes(2, STATE_NEW);
        let data = OtaData::decode([&previous, &new]);
        let Ok(BootAction::AdoptRunning(adopt)) = data.boot_action(0) else {
            panic!("expected the running slot to be adopted");
        };
        assert_eq!(adopt.sector, 0);
        assert_eq!(adopt.entry.slot(), Some(0));
        assert_eq!(adopt.entry.state, STATE_VALID);
        assert_eq!(adopt.entry.seq, 3);
        assert!(!data.running_unconfirmed(0));

        // A VALID preference that failed to load is left alone.
        let valid_new = entry_bytes(2, STATE_VALID);
        let data = OtaData::decode([&previous, &valid_new]);
        assert_eq!(data.boot_action(0), Ok(BootAction::None));
    }
}
