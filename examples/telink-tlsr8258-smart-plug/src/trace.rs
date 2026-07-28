/// RAM trace buffer for on-device debugging via SWS.
///
/// After building, find the address with:
///   llvm-nm <elf> | grep TRACE
///
/// Read via SWS:
///   TlsrPgm -p tcp://host:55555 ds <addr> 64
///
/// Layout: 16 x u32 = 64 bytes.
/// Magic should be 0x54524143 ("TRAC") when the device has been running.
pub const TRACE_MAGIC: u32 = 0x5452_4143;
pub const TRACE_WORDS: usize = 16;

#[unsafe(no_mangle)]
pub static mut TRACE: [u32; TRACE_WORDS] = [0u32; TRACE_WORDS];

// ── Field indices ───────────────────────────────────────────────
pub const IX_MAGIC: usize = 0;
pub const IX_SEQ: usize = 1;
pub const IX_COMMISSION_OK: usize = 2;
pub const IX_RECEIVE_CNT: usize = 3;
pub const IX_RECEIVE_OK: usize = 4;
pub const IX_RECEIVE_NODATA: usize = 5;
pub const IX_RECEIVE_ERR: usize = 6;
pub const IX_NWK_PARSE_FAIL: usize = 7;
pub const IX_NWK_KEY_MISSING: usize = 8;
pub const IX_NWK_DECRYPT_OK: usize = 9;
pub const IX_NWK_DECRYPT_FAIL: usize = 10;
pub const IX_ZDO_OK: usize = 11;
pub const IX_ZDO_FAIL: usize = 12;
pub const IX_LAST_ERR: usize = 13;
pub const IX_LAST_SRC: usize = 14;
pub const IX_LAST_EP: usize = 15;

#[inline(always)]
pub fn trace_init() {
    unsafe {
        let p = core::ptr::addr_of_mut!(TRACE);
        for i in 0..TRACE_WORDS {
            (*p)[i] = 0;
        }
        (*p)[IX_MAGIC] = TRACE_MAGIC;
    }
}

#[inline(always)]
pub fn trace_inc(idx: usize) {
    unsafe {
        if idx < TRACE_WORDS {
            let p = core::ptr::addr_of_mut!(TRACE);
            (*p)[idx] = (*p)[idx].wrapping_add(1);
        }
    }
}

#[inline(always)]
pub fn trace_set(idx: usize, val: u32) {
    unsafe {
        if idx < TRACE_WORDS {
            let p = core::ptr::addr_of_mut!(TRACE);
            (*p)[idx] = val;
        }
    }
}

#[inline(always)]
pub fn trace_seq() {
    unsafe {
        let p = core::ptr::addr_of_mut!(TRACE);
        (*p)[IX_SEQ] = (*p)[IX_SEQ].wrapping_add(1);
    }
}
