//! SetHook fee estimation: `docs/DESIGN.md` §6.1 — `bytes * 5000` drops.

/// Drops of XAH per byte of hook binary, per SetHook's fee schedule.
pub const DROPS_PER_BYTE: u64 = 5000;

/// Drops per whole XAH (1 XAH = 1,000,000 drops).
pub const DROPS_PER_XAH: u64 = 1_000_000;

/// Estimates the SetHook fee, in drops, for a binary of the given size.
#[must_use]
pub fn estimate_fee(size_bytes: usize) -> u64 {
    (size_bytes as u64).saturating_mul(DROPS_PER_BYTE)
}

/// Formats a drop amount as whole XAH plus remainder drops, as a decimal
/// string (e.g. `"1.234500"`).
#[must_use]
pub fn drops_to_xah_string(drops: u64) -> String {
    let whole = drops / DROPS_PER_XAH;
    let frac = drops % DROPS_PER_XAH;
    format!("{whole}.{frac:06}")
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
        assert_eq!(estimate_fee(1234), 1234 * 5000);
    }

    #[test]
    fn xah_string_zero_pads_the_fraction() {
        // 1 byte = 5000 drops = "0.005000".
        assert_eq!(drops_to_xah_string(estimate_fee(1)), "0.005000");
    }

    #[test]
    fn xah_string_exact_multiple_of_a_million_drops() {
        // 200 bytes = 1,000,000 drops = exactly 1 XAH, fraction is 0.
        let drops = estimate_fee(200);
        assert_eq!(drops % DROPS_PER_XAH, 0);
        assert_eq!(drops_to_xah_string(drops), "1.000000");
    }

    // `usize::MAX * 5000` only overflows `u64` on a 64-bit host; on a
    // 32-bit host the product is exact and nothing saturates.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn estimate_fee_saturates_on_overflow() {
        assert_eq!(estimate_fee(usize::MAX), u64::MAX);
    }
}
