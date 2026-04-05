/* EFR32MG1P Memory Layout
 * Flash: 256KB at 0x0000_0000
 * RAM:   32KB  at 0x2000_0000
 *
 * The EFR32MG1P maps flash at 0x00000000 and SRAM at 0x20000000.
 * First 16KB of flash is reserved for the bootloader (Gecko Bootloader).
 * Adjust ORIGIN/LENGTH based on your specific bootloader configuration.
 */
MEMORY
{
    FLASH : ORIGIN = 0x00004000, LENGTH = 240K
    RAM   : ORIGIN = 0x20000000, LENGTH = 32K
}
