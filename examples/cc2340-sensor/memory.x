/* CC2340R5 Memory Layout
 * Flash: 512KB at 0x0000_0000
 * SRAM:  36KB  at 0x2000_0000
 *
 * Note: actual usable RAM may be less due to radio buffer reservations.
 * The CC2340R5 has 36KB SRAM (not 64KB — that's CC2340R53).
 * Adjust based on your specific variant.
 */
MEMORY
{
    FLASH : ORIGIN = 0x00000000, LENGTH = 512K
    RAM   : ORIGIN = 0x20000000, LENGTH = 36K
}
