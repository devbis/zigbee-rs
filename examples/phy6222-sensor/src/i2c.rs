//! Pure-Rust I2C master driver for PHY6222 (DesignWare I2C IP).
//!
//! Polling-mode driver using direct register access.
//! Supports 100kHz and 400kHz modes. FIFO depth is 8 bytes,
//! reads are chunked to 7 bytes per sub-transaction.
//!
//! # Usage
//! ```rust,ignore
//! let mut i2c = I2cMaster::new(I2cConfig {
//!     dev: I2cDev::I2C0,
//!     scl_pin: 2,  // P2
//!     sda_pin: 3,  // P3
//!     speed: I2cSpeed::Fast400k,
//! });
//! let mut buf = [0u8; 6];
//! i2c.write_read(0x44, &[0x24, 0x00], &mut buf);  // SHT30 measurement
//! ```

/// I2C peripheral instance.
#[derive(Clone, Copy)]
pub enum I2cDev {
    I2C0,
    I2C1,
}

/// I2C speed mode.
#[derive(Clone, Copy)]
pub enum I2cSpeed {
    Standard100k,
    Fast400k,
}

/// I2C configuration.
pub struct I2cConfig {
    pub dev: I2cDev,
    pub scl_pin: u8,
    pub sda_pin: u8,
    pub speed: I2cSpeed,
}

/// I2C master driver.
pub struct I2cMaster {
    base: u32,
}

// Register offsets (DesignWare I2C)
const IC_CON: u32 = 0x00;
const IC_TAR: u32 = 0x04;
const IC_DATA_CMD: u32 = 0x10;
const IC_FS_SCL_HCNT: u32 = 0x1C;
const IC_FS_SCL_LCNT: u32 = 0x20;
const IC_SS_SCL_HCNT: u32 = 0x14;
const IC_SS_SCL_LCNT: u32 = 0x18;
const IC_INTR_MASK: u32 = 0x30;
const IC_RAW_INTR_STAT: u32 = 0x34;
const IC_RX_TL: u32 = 0x38;
const IC_TX_TL: u32 = 0x3C;
const IC_CLR_TX_ABRT: u32 = 0x54;
const IC_ENABLE: u32 = 0x6C;
const IC_STATUS: u32 = 0x70;
const IC_TXFLR: u32 = 0x74;
const IC_RXFLR: u32 = 0x78;

// Status bits
const STATUS_RFNE: u32 = 0x08;  // RX FIFO not empty
const STATUS_TFE: u32 = 0x04;   // TX FIFO empty
const STATUS_TFNF: u32 = 0x02;  // TX FIFO not full

// Clock gating
const AP_PCR_SW_CLK: u32 = 0x4000_0000;
const MOD_I2C0_BIT: u32 = 1 << 9;
const MOD_I2C1_BIT: u32 = 1 << 10;

// IOMUX
const AP_IOMUX_BASE: u32 = 0x4000_3800;

// AON for pull-ups
const AP_AON_BASE: u32 = 0x4000_F000;

impl I2cMaster {
    /// Initialize I2C master with the given configuration.
    pub fn new(config: I2cConfig) -> Self {
        let base = match config.dev {
            I2cDev::I2C0 => 0x4000_5000,
            I2cDev::I2C1 => 0x4000_5800,
        };

        // Enable clock gate
        let clk_bit = match config.dev {
            I2cDev::I2C0 => MOD_I2C0_BIT,
            I2cDev::I2C1 => MOD_I2C1_BIT,
        };
        let sw_clk = reg_read(AP_PCR_SW_CLK);
        reg_write(AP_PCR_SW_CLK, sw_clk | clk_bit);

        // Also enable IOMUX clock (bit 7)
        reg_write(AP_PCR_SW_CLK, reg_read(AP_PCR_SW_CLK) | (1 << 7));

        // Configure pin mux
        let fmux_scl = match config.dev {
            I2cDev::I2C0 => 0u8, // FMUX_IIC0_SCL
            I2cDev::I2C1 => 2,   // FMUX_IIC1_SCL
        };
        let fmux_sda = fmux_scl + 1;
        set_pin_fmux(config.scl_pin, fmux_scl);
        set_pin_fmux(config.sda_pin, fmux_sda);

        // Enable strong pull-ups on both pins
        set_pin_pull(config.scl_pin, 0x01); // strong pull-up
        set_pin_pull(config.sda_pin, 0x01);

        // Disable I2C before configuration
        reg_write(base + IC_ENABLE, 0);

        // IC_CON: master mode, restart enable, slave disable
        let speed_bits: u32 = match config.speed {
            I2cSpeed::Standard100k => 1 << 1,
            I2cSpeed::Fast400k => 2 << 1,
        };
        reg_write(base + IC_CON, 0x61 | speed_bits);

        // Set SCL timing (assuming PCLK = 16 MHz)
        match config.speed {
            I2cSpeed::Standard100k => {
                reg_write(base + IC_SS_SCL_HCNT, 72);
                reg_write(base + IC_SS_SCL_LCNT, 88);
            }
            I2cSpeed::Fast400k => {
                reg_write(base + IC_FS_SCL_HCNT, 14);
                reg_write(base + IC_FS_SCL_LCNT, 24);
            }
        }

        // Disable all interrupts, set FIFO thresholds
        reg_write(base + IC_INTR_MASK, 0);
        reg_write(base + IC_RX_TL, 0);
        reg_write(base + IC_TX_TL, 1);

        // Enable I2C
        reg_write(base + IC_ENABLE, 1);

        log::info!("[I2C] Initialized at 0x{:08X}", base);

        Self { base }
    }

    /// Set target slave address (7-bit).
    fn set_target(&self, addr: u8) {
        reg_write(self.base + IC_ENABLE, 0);
        reg_write(self.base + IC_TAR, addr as u32);
        reg_write(self.base + IC_ENABLE, 1);
    }

    /// Wait for TX FIFO to be not full, with timeout.
    fn wait_tx_ready(&self) -> bool {
        for _ in 0..10_000u32 {
            if reg_read(self.base + IC_STATUS) & STATUS_TFNF != 0 {
                return true;
            }
            cortex_m::asm::nop();
        }
        false
    }

    /// Wait for RX FIFO to have data, with timeout.
    fn wait_rx_ready(&self) -> bool {
        for _ in 0..50_000u32 {
            if reg_read(self.base + IC_STATUS) & STATUS_RFNE != 0 {
                return true;
            }
            cortex_m::asm::nop();
        }
        false
    }

    /// Wait for all TX data to be sent.
    fn wait_tx_empty(&self) -> bool {
        for _ in 0..50_000u32 {
            if reg_read(self.base + IC_STATUS) & STATUS_TFE != 0 {
                return true;
            }
            cortex_m::asm::nop();
        }
        false
    }

    /// Check and clear TX abort.
    fn check_abort(&self) -> bool {
        let raw = reg_read(self.base + IC_RAW_INTR_STAT);
        if raw & 0x40 != 0 {
            // TX_ABRT — clear it
            let _ = reg_read(self.base + IC_CLR_TX_ABRT);
            return true;
        }
        false
    }

    /// Write bytes then read bytes (write-read transaction).
    ///
    /// Sends `write_data` to register address, then reads `read_buf.len()` bytes.
    /// Uses repeated start between write and read phases.
    pub fn write_read(&self, addr: u8, write_data: &[u8], read_buf: &mut [u8]) -> Result<(), ()> {
        self.set_target(addr);

        // Write phase
        for &b in write_data {
            if !self.wait_tx_ready() { return Err(()); }
            reg_write(self.base + IC_DATA_CMD, b as u32);
            if self.check_abort() { return Err(()); }
        }

        // Wait for write to complete
        if !self.wait_tx_empty() { return Err(()); }

        // Read phase — chunked to 7 bytes (FIFO depth 8, leave room)
        let mut pos = 0;
        while pos < read_buf.len() {
            let chunk = (read_buf.len() - pos).min(7);

            // Issue read commands
            for _ in 0..chunk {
                if !self.wait_tx_ready() { return Err(()); }
                reg_write(self.base + IC_DATA_CMD, 0x100); // READ_CMD
            }

            // Collect read data
            for i in 0..chunk {
                if !self.wait_rx_ready() { return Err(()); }
                read_buf[pos + i] = (reg_read(self.base + IC_DATA_CMD) & 0xFF) as u8;
            }

            pos += chunk;
        }

        Ok(())
    }

    /// Write bytes only.
    pub fn write(&self, addr: u8, data: &[u8]) -> Result<(), ()> {
        self.set_target(addr);

        for &b in data {
            if !self.wait_tx_ready() { return Err(()); }
            reg_write(self.base + IC_DATA_CMD, b as u32);
            if self.check_abort() { return Err(()); }
        }

        if !self.wait_tx_empty() { return Err(()); }
        Ok(())
    }

    /// Read bytes only (no register write first).
    pub fn read(&self, addr: u8, buf: &mut [u8]) -> Result<(), ()> {
        self.set_target(addr);

        let mut pos = 0;
        while pos < buf.len() {
            let chunk = (buf.len() - pos).min(7);
            for _ in 0..chunk {
                if !self.wait_tx_ready() { return Err(()); }
                reg_write(self.base + IC_DATA_CMD, 0x100);
            }
            for i in 0..chunk {
                if !self.wait_rx_ready() { return Err(()); }
                buf[pos + i] = (reg_read(self.base + IC_DATA_CMD) & 0xFF) as u8;
            }
            pos += chunk;
        }

        Ok(())
    }
}

// ── Pin mux and pull-up helpers ─────────────────────────────────

/// GPIO pin index mapping (PHY6222 has non-contiguous pin numbers).
/// Returns the GPIO bit position for the given pin enum value.
fn pin_to_gpio_index(pin: u8) -> u8 {
    // Pin enum values from gpio.h match the GPIO bit positions
    // P0=0, P1=1, ..., P34=22
    pin
}

/// Set IOMUX function for a pin.
fn set_pin_fmux(pin: u8, fmux: u8) {
    let reg_idx = (pin >> 2) as u32;
    let bit_idx = (pin & 3) as u32;
    let shift = bit_idx * 8;
    let mask = 0x3F << shift;

    // Write mux value to gpio_sel register
    let sel_addr = AP_IOMUX_BASE + 0x08 + reg_idx * 4; // gpio_sel[n] offset
    let old = reg_read(sel_addr);
    reg_write(sel_addr, (old & !mask) | ((fmux as u32) << shift));

    // Enable full mux for this pin
    let mux_en_addr = AP_IOMUX_BASE + 0x00; // full_mux0_en
    let mux_en = reg_read(mux_en_addr);
    reg_write(mux_en_addr, mux_en | (1 << pin));
}

/// Set pull-up/down for a pin.
/// pull: 0=floating, 1=strong pull-up, 2=weak pull-up, 3=pull-down
fn set_pin_pull(pin: u8, pull: u8) {
    // Pull configuration is in AON IOCTL registers
    // Each pin has a 2-bit field at specific positions
    let (reg_offset, bit_h, bit_l) = match pin {
        0  => (0x08, 2, 1),    // IOCTL[0]
        1  => (0x08, 5, 4),
        2  => (0x08, 8, 7),
        3  => (0x08, 11, 10),
        4  => (0x08, 23, 22),  // P7
        5  => (0x08, 29, 28),  // P9
        6  => (0x0C, 2, 1),    // P10, IOCTL[1]
        7  => (0x0C, 5, 4),    // P11
        8  => (0x0C, 14, 13),  // P14
        9  => (0x0C, 17, 16),  // P15
        10 => (0x0C, 20, 19),  // P16
        11 => (0x0C, 23, 22),  // P17
        12 => (0x0C, 26, 25),  // P18
        13 => (0x10, 2, 1),    // P20, IOCTL[2]
        14 => (0x10, 11, 10),  // P23
        15 => (0x10, 14, 13),  // P24
        16 => (0x10, 17, 16),  // P25
        17 => (0x10, 20, 19),  // P26
        18 => (0x10, 23, 22),  // P27
        19 => (0x14, 5, 4),    // P31, PMCTL0
        20 => (0x14, 8, 7),    // P32
        21 => (0x14, 11, 10),  // P33
        22 => (0x14, 14, 13),  // P34
        _ => return,
    };

    let addr = AP_AON_BASE + reg_offset as u32;
    let mask = ((1u32 << (bit_h - bit_l + 1)) - 1) << bit_l;
    let old = reg_read(addr);
    reg_write(addr, (old & !mask) | ((pull as u32) << bit_l));
}

// ── Register access ─────────────────────────────────────────────

fn reg_write(addr: u32, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) };
}

fn reg_read(addr: u32) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
