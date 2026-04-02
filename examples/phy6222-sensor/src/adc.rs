//! Pure-Rust ADC driver for PHY6222 battery voltage measurement.
//!
//! Uses a single ADC channel to measure battery voltage via an analog pin.
//! The ADC is enabled only during measurement and disabled after to save power.
//!
//! # Usage
//! ```rust,ignore
//! let mv = read_battery_mv(AdcChannel::P11);
//! let pct = mv_to_percent(mv);
//! ```

/// ADC channel (maps to analog-capable GPIO pins).
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum AdcChannel {
    P11 = 2,  // ADC_CH1N_P11
    P23 = 3,  // ADC_CH1P_P23
    P24 = 4,  // ADC_CH2N_P24
    P14 = 5,  // ADC_CH2P_P14
    P15 = 6,  // ADC_CH3N_P15
    P20 = 7,  // ADC_CH3P_P20
}

// PCRM registers (at AP_PCRM_BASE = AP_AON_BASE + 0x3C = 0x4000_F03C)
const PCRM_BASE: u32 = 0x4000_F03C;
const PCRM_CLKSEL: u32 = PCRM_BASE + 0x00;      // 0x4000_F03C
const PCRM_CLKHF_CTL0: u32 = PCRM_BASE + 0x04;  // 0x4000_F040
const PCRM_CLKHF_CTL1: u32 = PCRM_BASE + 0x08;  // 0x4000_F044
const PCRM_ANA_CTL: u32 = PCRM_BASE + 0x0C;      // 0x4000_F048
const PCRM_ADC_CTL0: u32 = PCRM_BASE + 0x30;     // 0x4000_F06C
const PCRM_ADC_CTL4: u32 = PCRM_BASE + 0x40;     // 0x4000_F07C

// AON register for ADC channel routing
const AON_PMCTL2_1: u32 = 0x4000_F000 + 0x20;    // 0x4000_F020

// ADCC (ADC controller) base
const ADCC_BASE: u32 = 0x4005_0000;
const ADCC_ENABLE: u32 = ADCC_BASE + 0x00;
const ADCC_INTR_MASK: u32 = ADCC_BASE + 0x34;
const ADCC_INTR_CLEAR: u32 = ADCC_BASE + 0x38;
const ADCC_INTR_STATUS: u32 = ADCC_BASE + 0x3C;

// ADC sample buffer base (per-channel at stride 0x80)
const ADC_CH_BASE: u32 = 0x4005_0400;

// Clock gating
const AP_PCR_SW_CLK: u32 = 0x4000_0000;
const MOD_ADCC_BIT: u32 = 1 << 17;

// Number of samples to average
const ADC_SAMPLE_COUNT: usize = 10;

/// Read battery voltage in millivolts from the given ADC channel.
///
/// Enables ADC, takes samples, converts to mV, then disables ADC.
pub fn read_battery_mv(channel: AdcChannel) -> u32 {
    let ch = channel as u32;

    // 1. Enable clocks
    let clksel = reg_read(PCRM_CLKSEL);
    reg_write(PCRM_CLKSEL, clksel | (1 << 6));         // 1.28MHz enable

    let ctl0 = reg_read(PCRM_CLKHF_CTL0);
    reg_write(PCRM_CLKHF_CTL0, ctl0 | (1 << 18));      // XTAL out to digital

    let ctl1 = reg_read(PCRM_CLKHF_CTL1);
    reg_write(PCRM_CLKHF_CTL1, ctl1 | (1 << 7));       // DLL enable
    let ctl1 = reg_read(PCRM_CLKHF_CTL1);
    reg_write(PCRM_CLKHF_CTL1, ctl1 | (1 << 13));      // ADC clock enable

    // 2. Enable ADCC module clock
    let sw_clk = reg_read(AP_PCR_SW_CLK);
    reg_write(AP_PCR_SW_CLK, sw_clk | MOD_ADCC_BIT);

    // 3. Enable ADC + analog LDO
    let ana = reg_read(PCRM_ANA_CTL);
    reg_write(PCRM_ANA_CTL, ana | (1 << 3) | (1 << 0)); // ADC enable + analog LDO

    // Brief delay for ADC to stabilize
    for _ in 0..5000u32 { cortex_m::asm::nop(); }

    // 4. Configure ADC channel
    // Set PMCTL2_1 for channel routing
    let pmctl = reg_read(AON_PMCTL2_1);
    // Clear old channel config and set new one
    // Each channel needs specific bits — simplified single-channel config
    reg_write(AON_PMCTL2_1, pmctl | (1 << (ch + 8))); // Enable analog IO for channel

    // Configure ADC_CTL4: auto mode, single conversion
    let ctl4 = reg_read(PCRM_ADC_CTL4);
    reg_write(PCRM_ADC_CTL4, (ctl4 & !0x1F) | 0x01);  // Enable, auto mode

    // Set channel in ADC_CTL0
    reg_write(PCRM_ADC_CTL0, (1 << ch)); // Select channel

    // 5. Clear interrupts and enable
    reg_write(ADCC_INTR_CLEAR, 0x1FF);
    reg_write(ADCC_INTR_MASK, 1 << ch);
    reg_write(ADCC_ENABLE, 1 << ch);

    // 6. Wait for conversion complete (poll interrupt status)
    for _ in 0..100_000u32 {
        let status = reg_read(ADCC_INTR_STATUS);
        if status & (1 << ch) != 0 {
            break;
        }
        cortex_m::asm::nop();
    }

    // 7. Read samples from channel buffer
    let ch_buf_base = ADC_CH_BASE + ch * 0x80;
    let mut sum: u32 = 0;
    let mut count: u32 = 0;

    // Skip first 2 samples (settling), average the rest
    for i in 2..(2 + ADC_SAMPLE_COUNT) {
        let sample = reg_read(ch_buf_base + (i as u32) * 4);
        let raw = sample & 0xFFF; // 12-bit ADC
        if raw > 0 {
            sum += raw;
            count += 1;
        }
    }

    // 8. Disable ADC (power save)
    reg_write(ADCC_ENABLE, 0);
    reg_write(ADCC_INTR_CLEAR, 0x1FF);
    reg_write(AON_PMCTL2_1, pmctl); // Restore original PMCTL2_1

    let ana2 = reg_read(PCRM_ANA_CTL);
    reg_write(PCRM_ANA_CTL, ana2 & !((1 << 3) | (1 << 0))); // Disable ADC + analog LDO

    let sw_clk2 = reg_read(AP_PCR_SW_CLK);
    reg_write(AP_PCR_SW_CLK, sw_clk2 & !MOD_ADCC_BIT);

    // 9. Convert to millivolts
    // PHY6222 SDK uses scaling factor: mv = (adc_sum * 1904) >> 16
    // For P15 channel: mv = (adc_sum * 1710) >> 16
    if count == 0 {
        return 0;
    }

    let avg = sum / count;
    let scale = match channel {
        AdcChannel::P15 => 1710u32,
        _ => 1904u32,
    };

    (avg * scale) >> 4  // Adjusted for per-sample (not sum) scaling
}

/// Convert millivolts to battery percentage (0-100).
///
/// Linear mapping: 2000mV = 0%, 3000mV = 100%.
pub fn mv_to_percent(mv: u32) -> u8 {
    if mv >= 3000 { 100 }
    else if mv <= 2000 { 0 }
    else { ((mv - 2000) / 10) as u8 }
}

fn reg_write(addr: u32, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) };
}

fn reg_read(addr: u32) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
