/* BL702 memory layout for bare-metal Rust (no bootloader) */
MEMORY
{
  FLASH : ORIGIN = 0x23000000, LENGTH = 2M
  RAM   : ORIGIN = 0x42014000, LENGTH = 112K   /* 128K SRAM - 16K cache/EM */
}

/* Discard unwinding info — we use panic-halt, not panic-unwind */
SECTIONS
{
  /DISCARD/ : { *(.eh_frame) *(.eh_frame_hdr) }
}
