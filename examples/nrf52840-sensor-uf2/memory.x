MEMORY
{
  /* nRF52840 with Adafruit UF2 bootloader (nice!nano, ProMicro, etc.)
   *
   * The UF2 bootloader occupies 0x00000–0x25FFF (MBR + SoftDevice region).
   * Application must start at 0x26000.
   * Bootloader settings & bootloader itself sit at the end of flash.
   * Usable app region: 0x26000 – ~0xED000 ≈ 808 KB.
   *
   * RAM is fully available (no SoftDevice running).
   */
  FLASH : ORIGIN = 0x00026000, LENGTH = 808K
  RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
