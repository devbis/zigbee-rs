/* Telink TLSR8258 memory layout */
/* NOTE: tc32 ISA uses different memory map than standard ARM/RISC-V.
   For cargo check with thumbv6m stand-in, use ARM-compatible addresses.
   Real tc32 builds use the Telink linker script. */
MEMORY
{
    FLASH : ORIGIN = 0x00000000, LENGTH = 512K
    RAM   : ORIGIN = 0x00840000, LENGTH = 64K
}
