//! TLSR8258 GPIO interrupt configuration.
//!
//! Provides edge-triggered interrupt setup on individual GPIO pins.
//! The actual IRQ dispatch is handled by `tlsr8258-rt`'s vector table.
//!
//! Register reference (from `platform/chip_8258/register.h`):
//! - `reg_gpio_pol(i)` = `REG_ADDR8(0x584 + (group << 3))` — per-pin polarity
//! - `reg_gpio_irq_wakeup_en(i)` = `REG_ADDR8(0x587 + (group << 3))` — per-pin IRQ enable
//! - `reg_gpio_wakeup_irq` = `REG_ADDR8(0x5b5)` — `FLD_GPIO_CORE_INTERRUPT_EN` (BIT(3))
//! - `reg_irq_mask` = `REG_ADDR32(0x640)` — `FLD_IRQ_GPIO_EN` (BIT(18))
//! - `reg_irq_src` = `REG_ADDR32(0x648)` — write-1-to-clear

#[cfg(target_arch = "tc32")]
use super::mmio::{r8, w8, w32};

use super::gpio::Pin;

/// Interrupt edge polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptEdge {
    /// Trigger on rising edge (low → high).
    Rising,
    /// Trigger on falling edge (high → low).
    Falling,
}

// Register constants from platform/chip_8258/register.h
// base = REG_ADDR8(0x800000)
const REG_GPIO_WAKEUP_IRQ: u32 = super::mmio::REG_BASE + 0x5b5;
const FLD_GPIO_CORE_INTERRUPT_EN: u8 = 1 << 3;
const REG_IRQ_MASK: u32 = super::mmio::REG_BASE + 0x640;
const FLD_IRQ_GPIO_EN: u32 = 1 << 18;
const REG_IRQ_SRC: u32 = super::mmio::REG_BASE + 0x648;

/// Configure a pin for edge-triggered GPIO interrupt and enable it.
///
/// This mirrors the vendor `gpio_set_interrupt()` function:
/// 1. Sets the polarity (rising/falling) in `reg_gpio_pol`
/// 2. Clears any pending IRQ source for GPIO
/// 3. Enables `FLD_GPIO_CORE_INTERRUPT_EN` in `reg_gpio_wakeup_irq`
/// 4. Enables `FLD_IRQ_GPIO_EN` in `reg_irq_mask`
///
/// After calling this, the pin will generate an interrupt on the selected
/// edge. The interrupt handler must call [`clear_irq_src`] after checking
/// which pin caused the interrupt.
#[cfg(target_arch = "tc32")]
pub fn set_interrupt(pin: Pin, edge: InterruptEdge) {
    let (port, bit) = pin.port_and_bit();
    let group = port.index();
    let mask = 1u8 << bit;

    // 1. Set polarity in reg_gpio_pol
    let pol_addr = super::mmio::REG_BASE + 0x584 + ((group as u32) << 3);
    unsafe {
        let val = r8(pol_addr);
        w8(
            pol_addr,
            if matches!(edge, InterruptEdge::Falling) {
                val | mask
            } else {
                val & !mask
            },
        );
    }

    // 2. Clear any pending IRQ source for GPIO (write-1-to-clear)
    unsafe {
        w32(REG_IRQ_SRC, FLD_IRQ_GPIO_EN);
    }

    // 3. Enable FLD_GPIO_CORE_INTERRUPT_EN in reg_gpio_wakeup_irq
    unsafe {
        let val = r8(REG_GPIO_WAKEUP_IRQ);
        w8(REG_GPIO_WAKEUP_IRQ, val | FLD_GPIO_CORE_INTERRUPT_EN);
    }

    // 4. Enable FLD_IRQ_GPIO_EN in reg_irq_mask
    unsafe {
        let val = r8(REG_IRQ_MASK);
        w8(REG_IRQ_MASK, val | (FLD_IRQ_GPIO_EN as u8));
    }
}

/// Enable or disable a pin's contribution to the core GPIO IRQ.
///
/// This is the per-pin enable bit in `reg_gpio_irq_wakeup_en`.
/// The global `FLD_IRQ_GPIO_EN` in `reg_irq_mask` must also be set
/// for any GPIO interrupt to fire — use [`set_interrupt`] to do both
/// at once, or call this to selectively mask individual pins.
#[cfg(target_arch = "tc32")]
pub fn enable_pin_irq(pin: Pin, enable: bool) {
    let (port, bit) = pin.port_and_bit();
    let group = port.index();
    let addr = super::mmio::REG_BASE + 0x587 + ((group as u32) << 3);
    let mask = 1u8 << bit;
    unsafe {
        let val = r8(addr);
        w8(addr, if enable { val | mask } else { val & !mask });
    }
}

/// Disable a pin's interrupt and clear its pending source.
///
/// Clears the per-pin enable in `reg_gpio_irq_wakeup_en` and
/// clears the global GPIO IRQ source.
#[cfg(target_arch = "tc32")]
pub fn disable_pin_irq(pin: Pin) {
    enable_pin_irq(pin, false);
    clear_irq_src();
}

/// Clear the GPIO IRQ source bits (write-1-to-clear on `reg_irq_src`).
///
/// Call this from the IRQ handler after reading which pin caused the interrupt.
#[cfg(target_arch = "tc32")]
pub fn clear_irq_src() {
    unsafe {
        w32(REG_IRQ_SRC, FLD_IRQ_GPIO_EN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_addresses() {
        // reg_gpio_pol(Port::B) = 0x584 + (1 << 3) = 0x58C
        assert_eq!(crate::mmio::REG_BASE + 0x584 + (1 << 3), 0x80058C);
        // reg_gpio_irq_wakeup_en(Port::B) = 0x587 + (1 << 3) = 0x58F
        assert_eq!(crate::mmio::REG_BASE + 0x587 + (1 << 3), 0x80058F);
        // reg_gpio_wakeup_irq = 0x5B5
        assert_eq!(REG_GPIO_WAKEUP_IRQ, 0x8005B5);
        // reg_irq_mask = 0x640
        assert_eq!(REG_IRQ_MASK, 0x800640);
        // reg_irq_src = 0x648
        assert_eq!(REG_IRQ_SRC, 0x800648);
    }

    #[test]
    fn fld_constants() {
        assert_eq!(FLD_GPIO_CORE_INTERRUPT_EN, 0x08);
        assert_eq!(FLD_IRQ_GPIO_EN, 1 << 18);
    }

    #[test]
    fn interrupt_edge_variants() {
        assert!(matches!(InterruptEdge::Rising, InterruptEdge::Rising));
        assert!(!matches!(InterruptEdge::Rising, InterruptEdge::Falling));
        assert!(matches!(InterruptEdge::Falling, InterruptEdge::Falling));
    }
}
