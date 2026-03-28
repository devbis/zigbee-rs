/* Telink B91 memory layout */
MEMORY
{
    /* B91: 512KB Flash, 256KB SRAM */
    FLASH : ORIGIN = 0x20000000, LENGTH = 512K
    RAM   : ORIGIN = 0x00000000, LENGTH = 256K
}
