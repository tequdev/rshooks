//! SetHook fee estimation: `docs/DESIGN.md` §6.1 — `bytes * 5000` drops.

/// Drops of XAH per byte of hook binary, per SetHook's fee schedule.
pub const DROPS_PER_BYTE: u64 = 5000;

/// Drops per whole XAH (1 XAH = 1,000,000 drops).
pub const DROPS_PER_XAH: u64 = 1_000_000;

/// A fee estimate for a hook binary of a given size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeEstimate {
    /// The size of the binary, in bytes.
    pub bytes: u64,
    /// The estimated SetHook fee, in drops.
    pub drops: u64,
}

impl FeeEstimate {
    /// The estimated fee, in whole XAH plus remainder drops, as a decimal
    /// string (e.g. `"1.234500"`).
    #[must_use]
    pub fn xah_string(&self) -> String {
        let whole = self.drops / DROPS_PER_XAH;
        let frac = self.drops % DROPS_PER_XAH;
        format!("{whole}.{frac:06}")
    }
}

/// Estimates the SetHook fee for a binary of the given size.
#[must_use]
pub fn estimate_fee(size_bytes: usize) -> FeeEstimate {
    let bytes = size_bytes as u64;
    FeeEstimate {
        bytes,
        drops: bytes.saturating_mul(DROPS_PER_BYTE),
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn estimate_fee_is_bytes_times_5000() {
        let fee = estimate_fee(1234);
        assert_eq!(fee.bytes, 1234);
        assert_eq!(fee.drops, 1234 * 5000);
    }

    #[test]
    fn xah_string_zero_pads_the_fraction() {
        // 1 byte = 5000 drops = "0.005000".
        let fee = estimate_fee(1);
        assert_eq!(fee.xah_string(), "0.005000");
    }

    #[test]
    fn xah_string_exact_multiple_of_a_million_drops() {
        // 200 bytes = 1,000,000 drops = exactly 1 XAH, fraction is 0.
        let fee = estimate_fee(200);
        assert_eq!(fee.drops % DROPS_PER_XAH, 0);
        assert_eq!(fee.xah_string(), "1.000000");
    }

    #[test]
    fn estimate_fee_saturates_on_overflow() {
        let fee = estimate_fee(usize::MAX);
        assert_eq!(fee.drops, u64::MAX);
    }
}
