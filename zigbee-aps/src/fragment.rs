//! APS fragmentation (Zigbee PRO R22 §2.2.8.4.5).
//!
//! This module holds the *receive* side: bounded reassembly sessions that
//! place every block by its block number (never by arrival order), track the
//! R22 receive window and compute the windowed acknowledgement
//! (block number + ACK bitfield, R22 §2.2.8.4.5.2). The transmit state machine
//! lives in [`crate::apsde`] because it needs the NWK data service and APS
//! security; it shares the limits defined here.
//!
//! Sessions are only compiled in with the `fragmentation` feature (implied by
//! `router`). Without it the reassembly table has no slots, so a sleepy or
//! end-device image pays no RAM for it, and fragmented frames are dropped
//! with the R22 `DEFRAG_UNSUPPORTED` semantics.

use zigbee_types::IeeeAddress;

use crate::frames::{FRAG_FIRST, FRAG_SUBSEQUENT};

/// Largest ASDU this stack transmits or reassembles with fragmentation.
///
/// The reassembled ASDU is delivered through
/// [`crate::apsde::ApsFrameBuffer`], whose capacity is this same constant, so
/// a completed transaction can never be truncated on delivery.
#[cfg(feature = "fragmentation")]
pub const APS_MAX_FRAGMENTED_ASDU: usize = 256;
/// Without the `fragmentation` feature nothing larger than one frame exists.
#[cfg(not(feature = "fragmentation"))]
pub const APS_MAX_FRAGMENTED_ASDU: usize = 128;

/// Upper bound on the number of blocks of one fragmented transaction.
///
/// R22 lets the block-count octet reach 255, but a transaction can never
/// carry more than [`APS_MAX_FRAGMENTED_ASDU`] octets here; 16 blocks of the
/// smallest block size this stack sends already cover it. Larger counts are
/// "parameters outside the bounds of this protocol" and are rejected
/// (R22 §2.2.8.4.5.1), which also keeps every block bit inside a `u16`.
pub const APS_MAX_FRAGMENT_BLOCKS: u8 = 16;

/// Largest `apsMaxWindowSize` (R22 Table 2-26: 1..=8).
pub const APS_MAX_WINDOW_SIZE: u8 = 8;

/// Clamp an AIB `apsMaxWindowSize` value into the R22 range 1..=8.
pub const fn clamp_window_size(window_size: u8) -> u8 {
    if window_size == 0 {
        1
    } else if window_size > APS_MAX_WINDOW_SIZE {
        APS_MAX_WINDOW_SIZE
    } else {
        window_size
    }
}

/// Receive-side transaction timeout.
///
/// R22 §2.2.8.4.5.2: a transaction whose receive window does not move forward
/// within `apscAckWaitDuration * (1 + apscMaxFrameRetries)` has failed.
pub const APS_FRAGMENT_RX_TIMEOUT_US: u32 =
    crate::APS_ACK_WAIT_DURATION_US * (1 + APSC_MAX_FRAME_RETRIES as u32);

/// `apscMaxFrameRetries` (R22 Table 2-23).
pub const APSC_MAX_FRAME_RETRIES: u8 = 3;

/// Windowed acknowledgement for a fragmented transaction
/// (R22 §2.2.5.2.3 / §2.2.8.4.5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentAck {
    /// Fragmentation sub-field echoed from the acknowledged frame.
    pub fragmentation: u8,
    /// Lowest block number of the acknowledged receive window.
    pub window_start: u8,
    /// Bit *n* = block `window_start + n` received. Bits beyond
    /// `apsMaxWindowSize` (and beyond the last block) are set to 1.
    pub bitfield: u8,
}

/// Result of offering one fragment to [`FragmentReassembly::insert_block`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentOutcome {
    /// Block accepted (or recognised as a retransmission); more blocks are
    /// needed. `ack` is the windowed acknowledgement R22 requires now, if any.
    /// `stored` is true when this call recorded a new block.
    Pending {
        ack: Option<FragmentAck>,
        stored: bool,
    },
    /// Every block is present; read the ASDU with
    /// [`FragmentReassembly::take_reassembled`].
    Complete { ack: FragmentAck },
    /// Outside the protocol bounds, inconsistent with the session, too large
    /// for the reassembly buffer or no session available: drop without
    /// acknowledging.
    Rejected,
}

/// Concurrent reassembly sessions.
#[cfg(all(feature = "fragmentation", feature = "router"))]
const MAX_ENTRIES: usize = 4;
#[cfg(all(feature = "fragmentation", not(feature = "router")))]
const MAX_ENTRIES: usize = 1;
#[cfg(not(feature = "fragmentation"))]
const MAX_ENTRIES: usize = 0;

const MAX_BLOCKS: usize = APS_MAX_FRAGMENT_BLOCKS as usize;

/// A single fragment reassembly slot.
struct ReassemblyEntry {
    active: bool,
    /// Handed to the caller; kept only until the frame's durable commit so an
    /// abort can roll the final block back.
    delivered: bool,
    src_addr: u16,
    /// Originator IEEE address when known; a different device that reuses the
    /// short address never joins this session.
    src_ieee: Option<IeeeAddress>,
    aps_counter: u8,
    /// Total block count from the first block, 0 until block 0 arrives.
    total_blocks: u8,
    /// Bit *n* = block *n* stored.
    received: u16,
    /// Lowest block number of the current receive window.
    window_start: u8,
    /// Storage offset and length of each block in `data`.
    block_offset: [u16; MAX_BLOCKS],
    block_len: [u8; MAX_BLOCKS],
    /// Blocks are appended in arrival order and reordered by block number
    /// only when the ASDU is read out.
    data: [u8; APS_MAX_FRAGMENTED_ASDU],
    data_len: u16,
    /// Monotonic timestamp of the last receive-window progress.
    last_progress_us: u32,
}

impl ReassemblyEntry {
    #[allow(dead_code)]
    const fn empty() -> Self {
        Self {
            active: false,
            delivered: false,
            src_addr: 0,
            src_ieee: None,
            aps_counter: 0,
            total_blocks: 0,
            received: 0,
            window_start: 0,
            block_offset: [0; MAX_BLOCKS],
            block_len: [0; MAX_BLOCKS],
            data: [0; APS_MAX_FRAGMENTED_ASDU],
            data_len: 0,
            last_progress_us: 0,
        }
    }

    fn matches(&self, src_addr: u16, src_ieee: Option<&IeeeAddress>, aps_counter: u8) -> bool {
        self.active
            && self.src_addr == src_addr
            && self.aps_counter == aps_counter
            && match (self.src_ieee.as_ref(), src_ieee) {
                (Some(known), Some(offered)) => known == offered,
                _ => true,
            }
    }

    fn has(&self, block: u8) -> bool {
        usize::from(block) < MAX_BLOCKS && self.received & (1u16 << block) != 0
    }

    /// End (exclusive) of the current receive window.
    fn window_end(&self, window_size: u8) -> u8 {
        let end = self.window_start.saturating_add(window_size);
        if self.total_blocks != 0 {
            end.min(self.total_blocks)
        } else {
            end.min(APS_MAX_FRAGMENT_BLOCKS)
        }
    }

    fn window_complete(&self, window_size: u8) -> bool {
        // Without block 0 the total is unknown and window 0 cannot be full.
        self.total_blocks != 0
            && (self.window_start..self.window_end(window_size)).all(|block| self.has(block))
    }

    fn is_complete(&self) -> bool {
        self.total_blocks != 0 && (0..self.total_blocks).all(|block| self.has(block))
    }

    fn ack(&self, fragmentation: u8, window_size: u8) -> FragmentAck {
        let mut bitfield = 0u8;
        for bit in 0..APS_MAX_WINDOW_SIZE {
            let block = u16::from(self.window_start) + u16::from(bit);
            let set = bit >= window_size
                || (self.total_blocks != 0 && block >= u16::from(self.total_blocks))
                || (block < MAX_BLOCKS as u16 && self.has(block as u8));
            if set {
                bitfield |= 1 << bit;
            }
        }
        FragmentAck {
            fragmentation,
            window_start: self.window_start,
            bitfield,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.delivered = false;
        self.received = 0;
        self.total_blocks = 0;
        self.window_start = 0;
        self.data_len = 0;
        crate::zeroize(&mut self.data);
    }
}

/// APS fragment reassembly context.
///
/// Used by `ApsLayer` to reassemble incoming fragmented data frames.
pub struct FragmentReassembly {
    entries: [ReassemblyEntry; MAX_ENTRIES],
}

impl Default for FragmentReassembly {
    fn default() -> Self {
        Self::new()
    }
}

impl FragmentReassembly {
    pub const fn new() -> Self {
        Self {
            entries: [const { ReassemblyEntry::empty() }; MAX_ENTRIES],
        }
    }

    /// Whether this build can reassemble fragmented transmissions.
    pub const fn supported() -> bool {
        MAX_ENTRIES != 0
    }

    /// Number of active (incomplete or undelivered) sessions.
    pub fn active_sessions(&self) -> usize {
        self.entries.iter().filter(|entry| entry.active).count()
    }

    /// Offer one received fragment.
    ///
    /// * `fragmentation` / `block_field` — the extended-header sub-fields:
    ///   for [`FRAG_FIRST`] the block field is the *total block count* and the
    ///   block is block 0; for [`FRAG_SUBSEQUENT`] it is the block number.
    /// * `window_size` — `apsMaxWindowSize` (clamped to 1..=8).
    /// * `now_us` — platform monotonic time.
    ///
    /// Each block is stored by block number, so arrival order is irrelevant;
    /// a repeated block (including a repeated block 0) never resets or
    /// rewrites the session.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_block(
        &mut self,
        now_us: u32,
        window_size: u8,
        src_addr: u16,
        src_ieee: Option<IeeeAddress>,
        aps_counter: u8,
        fragmentation: u8,
        block_field: u8,
        payload: &[u8],
    ) -> FragmentOutcome {
        let outcome = self.insert_block_inner(
            now_us,
            window_size,
            src_addr,
            src_ieee,
            aps_counter,
            fragmentation,
            block_field,
            payload,
        );
        if outcome == FragmentOutcome::Rejected {
            // Never leave a session holding no block behind.
            for entry in self.entries.iter_mut() {
                if entry.active && entry.received == 0 {
                    entry.reset();
                }
            }
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_block_inner(
        &mut self,
        now_us: u32,
        window_size: u8,
        src_addr: u16,
        src_ieee: Option<IeeeAddress>,
        aps_counter: u8,
        fragmentation: u8,
        block_field: u8,
        payload: &[u8],
    ) -> FragmentOutcome {
        if self.entries.is_empty() {
            return FragmentOutcome::Rejected;
        }
        let window_size = clamp_window_size(window_size);
        let (block, total) = match fragmentation {
            FRAG_FIRST => (0u8, Some(block_field)),
            FRAG_SUBSEQUENT => (block_field, None),
            _ => return FragmentOutcome::Rejected,
        };
        if matches!(total, Some(0)) || total.is_some_and(|t| t > APS_MAX_FRAGMENT_BLOCKS) {
            return FragmentOutcome::Rejected;
        }
        // A subsequent block is numbered from 1; its number must also be a
        // valid bit index.
        if (fragmentation == FRAG_SUBSEQUENT && block == 0) || block >= APS_MAX_FRAGMENT_BLOCKS {
            return FragmentOutcome::Rejected;
        }
        if payload.len() > usize::from(u8::MAX) {
            return FragmentOutcome::Rejected;
        }

        let idx = match self
            .entries
            .iter()
            .position(|entry| entry.matches(src_addr, src_ieee.as_ref(), aps_counter))
        {
            Some(idx) => idx,
            None => {
                let Some(idx) = self.entries.iter().position(|entry| !entry.active) else {
                    // R22 §2.2.8.4.5.2 permits rejecting a further concurrent
                    // transaction; never evict one that is still progressing.
                    return FragmentOutcome::Rejected;
                };
                let entry = &mut self.entries[idx];
                entry.reset();
                entry.active = true;
                entry.src_addr = src_addr;
                entry.src_ieee = src_ieee;
                entry.aps_counter = aps_counter;
                entry.last_progress_us = now_us;
                idx
            }
        };
        let entry = &mut self.entries[idx];
        if entry.delivered {
            // Already handed up; the caller's duplicate table answers repeats.
            return FragmentOutcome::Pending {
                ack: None,
                stored: false,
            };
        }
        if entry.src_ieee.is_none() {
            entry.src_ieee = src_ieee;
        }

        if let Some(total) = total {
            if entry.total_blocks == 0 {
                // Every block already stored must fit the announced total.
                if u32::from(entry.received) >> total != 0 {
                    entry.reset();
                    return FragmentOutcome::Rejected;
                }
                entry.total_blocks = total;
            } else if entry.total_blocks != total {
                return FragmentOutcome::Rejected;
            }
        }
        if entry.total_blocks != 0 && block >= entry.total_blocks {
            return FragmentOutcome::Rejected;
        }

        // Receive-window handling (R22 §2.2.8.4.5.2).
        if block < entry.window_start {
            // Belongs to a window that was already acknowledged and passed.
            return FragmentOutcome::Pending {
                ack: None,
                stored: false,
            };
        }
        if u16::from(block) >= u16::from(entry.window_start) + u16::from(window_size) {
            if !entry.window_complete(window_size) {
                // Outside the current window: no acknowledgement.
                return FragmentOutcome::Pending {
                    ack: None,
                    stored: false,
                };
            }
            let next_start = entry.window_start.saturating_add(window_size);
            if u16::from(block) >= u16::from(next_start) + u16::from(window_size) {
                return FragmentOutcome::Pending {
                    ack: None,
                    stored: false,
                };
            }
            entry.window_start = next_start;
            entry.last_progress_us = now_us;
        }

        let stored = if entry.has(block) {
            false
        } else {
            let offset = usize::from(entry.data_len);
            let end = offset + payload.len();
            if end > entry.data.len() {
                // The ASDU is larger than this device can reassemble.
                entry.reset();
                return FragmentOutcome::Rejected;
            }
            entry.data[offset..end].copy_from_slice(payload);
            entry.block_offset[usize::from(block)] = offset as u16;
            entry.block_len[usize::from(block)] = payload.len() as u8;
            entry.data_len = end as u16;
            entry.received |= 1u16 << block;
            entry.last_progress_us = now_us;
            true
        };

        if entry.is_complete() {
            let ack = entry.ack(fragmentation, window_size);
            return FragmentOutcome::Complete { ack };
        }
        // Acknowledge when every later block of the window is present: that
        // covers the last block of the window or transaction, and a
        // retransmission that fills the last gap (R22 §2.2.8.4.5.2 (1)-(3)).
        let window_end = entry.window_end(window_size);
        let ack = (block.saturating_add(1)..window_end)
            .all(|later| entry.has(later))
            .then(|| entry.ack(fragmentation, window_size));
        FragmentOutcome::Pending { ack, stored }
    }

    /// Copy a completed ASDU, in block order, into `out`.
    ///
    /// Returns the ASDU length, or `None` if no completed session matches or
    /// `out` is too small — a truncated ASDU is never produced. The session
    /// is kept (marked delivered) until [`Self::finalize_delivered`] or
    /// [`Self::rollback_block`].
    pub fn take_reassembled(
        &mut self,
        src_addr: u16,
        src_ieee: Option<IeeeAddress>,
        aps_counter: u8,
        out: &mut [u8],
    ) -> Option<usize> {
        let entry = self.entries.iter_mut().find(|entry| {
            entry.matches(src_addr, src_ieee.as_ref(), aps_counter) && !entry.delivered
        })?;
        if !entry.is_complete() || usize::from(entry.data_len) > out.len() {
            return None;
        }
        let mut written = 0usize;
        for block in 0..usize::from(entry.total_blocks) {
            let offset = usize::from(entry.block_offset[block]);
            let len = usize::from(entry.block_len[block]);
            out[written..written + len].copy_from_slice(&entry.data[offset..offset + len]);
            written += len;
        }
        entry.delivered = true;
        Some(written)
    }

    /// Free every session whose ASDU has been delivered and committed.
    pub fn finalize_delivered(&mut self) {
        for entry in self.entries.iter_mut() {
            if entry.active && entry.delivered {
                entry.reset();
            }
        }
    }

    /// Undo the reception of `block` after its frame's durable commit failed,
    /// so the sender's retransmission is accepted again.
    pub fn rollback_block(&mut self, src_addr: u16, aps_counter: u8, block: u8) {
        if usize::from(block) >= MAX_BLOCKS {
            return;
        }
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.active && entry.src_addr == src_addr && entry.aps_counter == aps_counter
        }) {
            entry.delivered = false;
            if entry.has(block) {
                entry.received &= !(1u16 << block);
                // The rolled-back block is the most recently stored one;
                // reclaim its storage so the retransmission fits again.
                let offset = entry.block_offset[usize::from(block)];
                let len = u16::from(entry.block_len[usize::from(block)]);
                if offset + len == entry.data_len {
                    entry.data_len = offset;
                }
            }
            if entry.received == 0 {
                entry.reset();
            }
        }
    }

    /// Mark a completed entry as inactive so the slot can be reused.
    pub fn complete_entry(&mut self, src_addr: u16, aps_counter: u8) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.active && entry.src_addr == src_addr && entry.aps_counter == aps_counter
        }) {
            entry.reset();
        }
    }

    /// Expire transactions whose receive window has not moved for
    /// [`APS_FRAGMENT_RX_TIMEOUT_US`] (R22 §2.2.8.4.5.2), using the platform
    /// monotonic clock.
    pub fn expire(&mut self, now_us: u32) {
        for entry in self.entries.iter_mut() {
            if entry.active
                && !entry.delivered
                && now_us.wrapping_sub(entry.last_progress_us) >= APS_FRAGMENT_RX_TIMEOUT_US
            {
                log::debug!(
                    "[APS frag] Expiring stale reassembly: src=0x{:04X} counter={} ({}/{} blocks)",
                    entry.src_addr,
                    entry.aps_counter,
                    entry.received.count_ones(),
                    entry.total_blocks,
                );
                entry.reset();
            }
        }
    }

    /// Compatibility shim for the former tick-counted ageing.
    ///
    /// Expiry is now time-based: [`crate::ApsLayer::age_dup_table`] expires
    /// sessions against the platform monotonic clock, so this call does
    /// nothing and is kept only so existing maintenance loops still compile.
    pub fn age_entries(&mut self) {}
}

#[cfg(all(test, feature = "fragmentation"))]
mod tests {
    use super::*;

    const SRC: u16 = 0x1234;
    const IEEE: IeeeAddress = [0xAB; 8];

    fn insert(
        rx: &mut FragmentReassembly,
        now: u32,
        window: u8,
        frag: u8,
        field: u8,
        payload: &[u8],
    ) -> FragmentOutcome {
        rx.insert_block(now, window, SRC, Some(IEEE), 7, frag, field, payload)
    }

    #[test]
    fn blocks_are_placed_by_number_not_arrival_order() {
        let mut rx = FragmentReassembly::new();
        // Block 2 (last), then block 1, then block 0.
        assert_eq!(
            insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 2, b"CC"),
            FragmentOutcome::Pending {
                ack: None,
                stored: true
            }
        );
        assert!(matches!(
            insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 1, b"BBB"),
            FragmentOutcome::Pending { stored: true, .. }
        ));
        let FragmentOutcome::Complete { ack } = insert(&mut rx, 0, 8, FRAG_FIRST, 3, b"A") else {
            panic!("block 0 completes the transaction");
        };
        assert_eq!(ack.window_start, 0);
        assert_eq!(ack.bitfield, 0xFF);
        let mut out = [0u8; APS_MAX_FRAGMENTED_ASDU];
        let len = rx.take_reassembled(SRC, Some(IEEE), 7, &mut out).unwrap();
        assert_eq!(&out[..len], b"ABBBCC");
    }

    #[test]
    fn repeated_first_block_does_not_reset_the_session() {
        let mut rx = FragmentReassembly::new();
        insert(&mut rx, 0, 8, FRAG_FIRST, 3, b"A");
        insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 1, b"B");
        // A retransmitted block 0 (lost ACK) and a conflicting total.
        assert!(matches!(
            insert(&mut rx, 0, 8, FRAG_FIRST, 3, b"A"),
            FragmentOutcome::Pending { stored: false, .. }
        ));
        assert_eq!(
            insert(&mut rx, 0, 8, FRAG_FIRST, 4, b"A"),
            FragmentOutcome::Rejected
        );
        assert!(matches!(
            insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 2, b"C"),
            FragmentOutcome::Complete { .. }
        ));
        let mut out = [0u8; 8];
        let len = rx.take_reassembled(SRC, Some(IEEE), 7, &mut out).unwrap();
        assert_eq!(&out[..len], b"ABC");
    }

    #[test]
    fn out_of_bounds_block_numbers_are_rejected_without_overflow() {
        let mut rx = FragmentReassembly::new();
        for field in [APS_MAX_FRAGMENT_BLOCKS, 200, 255] {
            // Block numbers are 1..total-1 and bit indices of a u16 mask.
            assert_eq!(
                insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, field, b"x"),
                FragmentOutcome::Rejected
            );
        }
        for field in [APS_MAX_FRAGMENT_BLOCKS + 1, 200, 255] {
            assert_eq!(
                insert(&mut rx, 0, 8, FRAG_FIRST, field, b"x"),
                FragmentOutcome::Rejected
            );
        }
        assert_eq!(rx.active_sessions(), 0, "a rejected block opens no session");
        assert_eq!(
            insert(&mut rx, 0, 8, FRAG_FIRST, 0, b"x"),
            FragmentOutcome::Rejected
        );
        assert_eq!(
            insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 0, b"x"),
            FragmentOutcome::Rejected
        );
        // A block at or beyond the announced total is rejected.
        insert(&mut rx, 0, 8, FRAG_FIRST, 2, b"x");
        assert_eq!(
            insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 2, b"x"),
            FragmentOutcome::Rejected
        );
        assert_eq!(rx.active_sessions(), 1);
    }

    #[test]
    fn an_asdu_larger_than_the_buffer_is_rejected_not_truncated() {
        let mut rx = FragmentReassembly::new();
        let block = [0x55u8; 100];
        insert(&mut rx, 0, 8, FRAG_FIRST, 3, &block);
        insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 1, &block);
        assert_eq!(
            insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 2, &block),
            FragmentOutcome::Rejected
        );
        assert_eq!(rx.active_sessions(), 0, "the session is abandoned");
    }

    #[test]
    fn take_reassembled_refuses_a_short_output_buffer() {
        let mut rx = FragmentReassembly::new();
        insert(&mut rx, 0, 8, FRAG_FIRST, 2, b"AAAA");
        insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 1, b"BBBB");
        let mut small = [0u8; 7];
        assert_eq!(rx.take_reassembled(SRC, Some(IEEE), 7, &mut small), None);
    }

    #[test]
    fn window_acks_follow_r22_with_window_size_two() {
        let mut rx = FragmentReassembly::new();
        // 5 blocks, window 2: [0,1] [2,3] [4]
        assert_eq!(
            insert(&mut rx, 0, 2, FRAG_FIRST, 5, b"0"),
            FragmentOutcome::Pending {
                ack: None,
                stored: true
            }
        );
        let FragmentOutcome::Pending { ack: Some(ack), .. } =
            insert(&mut rx, 0, 2, FRAG_SUBSEQUENT, 1, b"1")
        else {
            panic!("last block of the window is acknowledged");
        };
        assert_eq!(ack.window_start, 0);
        assert_eq!(ack.bitfield, 0xFF, "unused bits beyond the window are 1");
        // Block 3 before 2: outside nothing, inside window [2,3] → ACK with gap.
        let FragmentOutcome::Pending { ack: Some(ack), .. } =
            insert(&mut rx, 0, 2, FRAG_SUBSEQUENT, 3, b"3")
        else {
            panic!("last block of window 2 is acknowledged");
        };
        assert_eq!(ack.window_start, 2);
        assert_eq!(ack.bitfield, 0b1111_1110, "block 2 is missing");
        // Block 4 is outside the incomplete window: no ACK, not stored.
        assert_eq!(
            insert(&mut rx, 0, 2, FRAG_SUBSEQUENT, 4, b"4"),
            FragmentOutcome::Pending {
                ack: None,
                stored: false
            }
        );
        // Retransmitted block 2 fills the gap → ACK.
        let FragmentOutcome::Pending { ack: Some(ack), .. } =
            insert(&mut rx, 0, 2, FRAG_SUBSEQUENT, 2, b"2")
        else {
            panic!("gap fill is acknowledged");
        };
        assert_eq!(ack.bitfield, 0xFF);
        let FragmentOutcome::Complete { ack } = insert(&mut rx, 0, 2, FRAG_SUBSEQUENT, 4, b"4")
        else {
            panic!("final block completes");
        };
        assert_eq!(ack.window_start, 4);
        assert_eq!(ack.bitfield, 0xFF);
        let mut out = [0u8; 8];
        let len = rx.take_reassembled(SRC, Some(IEEE), 7, &mut out).unwrap();
        assert_eq!(&out[..len], b"01234");
    }

    #[test]
    fn a_different_device_reusing_the_short_address_gets_its_own_session() {
        let mut rx = FragmentReassembly::new();
        insert(&mut rx, 0, 8, FRAG_FIRST, 2, b"A");
        let other = rx.insert_block(0, 8, SRC, Some([0xCD; 8]), 7, FRAG_SUBSEQUENT, 1, b"Z");
        // Single-slot ED builds reject it; router builds open a second session.
        if MAX_ENTRIES == 1 {
            assert_eq!(other, FragmentOutcome::Rejected);
        } else {
            assert!(matches!(
                other,
                FragmentOutcome::Pending { stored: true, .. }
            ));
        }
        assert!(matches!(
            insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 1, b"B"),
            FragmentOutcome::Complete { .. }
        ));
    }

    #[test]
    fn sessions_expire_by_elapsed_time() {
        let mut rx = FragmentReassembly::new();
        insert(&mut rx, 1_000, 8, FRAG_FIRST, 2, b"A");
        rx.expire(1_000 + APS_FRAGMENT_RX_TIMEOUT_US - 1);
        assert_eq!(rx.active_sessions(), 1);
        // Calling the old tick API any number of times changes nothing.
        for _ in 0..100 {
            rx.age_entries();
        }
        assert_eq!(rx.active_sessions(), 1);
        rx.expire(1_000 + APS_FRAGMENT_RX_TIMEOUT_US);
        assert_eq!(rx.active_sessions(), 0);
    }

    #[test]
    fn rollback_reopens_the_final_block() {
        let mut rx = FragmentReassembly::new();
        insert(&mut rx, 0, 8, FRAG_FIRST, 2, b"A");
        insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 1, b"B");
        let mut out = [0u8; 4];
        assert!(rx.take_reassembled(SRC, Some(IEEE), 7, &mut out).is_some());
        rx.rollback_block(SRC, 7, 1);
        assert!(matches!(
            insert(&mut rx, 0, 8, FRAG_SUBSEQUENT, 1, b"B"),
            FragmentOutcome::Complete { .. }
        ));
        assert!(rx.take_reassembled(SRC, Some(IEEE), 7, &mut out).is_some());
        rx.finalize_delivered();
        assert_eq!(rx.active_sessions(), 0);
    }
}
