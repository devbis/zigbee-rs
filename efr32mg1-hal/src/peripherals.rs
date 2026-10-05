//! Exclusive ownership tokens for singleton EFR32MG1 peripherals.

use core::sync::atomic::{AtomicBool, Ordering};

/// Exclusive ownership of the CRYPTO accelerator.
#[derive(Debug)]
pub struct Crypto {
    _private: (),
}

impl Crypto {
    const fn new() -> Self {
        Self { _private: () }
    }
}

/// Uniquely owned chip peripherals with stateful safe driver constructors.
pub struct Peripherals {
    /// CRYPTO AES-128 accelerator (see [`crate::crypto`]).
    pub crypto: Crypto,
    /// Internal MSC flash (whole chip). Products split it into bounded
    /// persistence partitions; see [`crate::flash::Efr32mg1Flash`].
    pub flash: crate::flash::Efr32mg1Flash,
}

static TAKEN: AtomicBool = AtomicBool::new(false);

impl Peripherals {
    /// Acquire the singleton peripheral tokens once.
    pub fn take() -> Option<Self> {
        TAKEN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self {
                crypto: Crypto::new(),
                // SAFETY: `TAKEN` guarantees this is the only flash handle
                // minted by safe code.
                flash: unsafe { crate::flash::Efr32mg1Flash::new() },
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn peripherals_can_only_be_taken_once() {
        assert!(Peripherals::take().is_some());
        assert!(Peripherals::take().is_none());
    }

    #[test]
    fn crypto_token_is_zero_sized() {
        assert_eq!(size_of::<Crypto>(), 0);
    }

    #[test]
    fn flash_handle_is_zero_sized() {
        assert_eq!(size_of::<crate::flash::Efr32mg1Flash>(), 0);
    }
}
