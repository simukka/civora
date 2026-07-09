//! Wall-clock epochs for voting windows.
//!
//! An epoch is just Unix time floored to a fixed window: `epoch = unix_secs /
//! epoch_secs`. There is no epoch-sync protocol — every peer derives the same
//! number from its own clock, and a proposal's `activation_epoch` is the first
//! epoch at which its voting window has closed. The divisor is a parameter
//! rather than a constant so tests can shrink the window without a trait or a
//! mock clock; that arithmetic *is* the seam. Certificate verification is
//! clock-free, so a divisor mismatch across peers only skews UX timing, never
//! validity.

/// Default voting-window length, in seconds. Clients may override this with a
/// dev knob, but it must match across instances or countdowns disagree.
pub const EPOCH_SECS: u64 = 30;

/// The epoch containing `unix_secs` for a window of `epoch_secs` seconds.
///
/// A zero divisor is treated as 1 so a misconfigured window can never panic on
/// division; every second is then its own epoch.
pub fn epoch_at(unix_secs: u64, epoch_secs: u64) -> u64 {
    unix_secs / epoch_secs.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_boundaries_at_default_window() {
        assert_eq!(epoch_at(0, EPOCH_SECS), 0);
        assert_eq!(epoch_at(29, EPOCH_SECS), 0);
        assert_eq!(epoch_at(30, EPOCH_SECS), 1);
        assert_eq!(epoch_at(59, EPOCH_SECS), 1);
        assert_eq!(epoch_at(60, EPOCH_SECS), 2);
    }

    #[test]
    fn custom_window_divides_directly() {
        assert_eq!(epoch_at(0, 2), 0);
        assert_eq!(epoch_at(1, 2), 0);
        assert_eq!(epoch_at(2, 2), 1);
        assert_eq!(epoch_at(7, 2), 3);
    }

    #[test]
    fn zero_divisor_falls_back_to_one_second_epochs() {
        assert_eq!(epoch_at(0, 0), 0);
        assert_eq!(epoch_at(5, 0), 5);
        assert_eq!(epoch_at(u64::MAX, 0), u64::MAX);
    }
}
