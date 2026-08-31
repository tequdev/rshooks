//! Loop-free equality and ordering checks for protocol-sized byte arrays.
//!
//! These concrete functions use literal indices and straight-line XOR/OR (or
//! compare-and-branch) code so Hooks do not rely on compiler-generated
//! comparison loops.

/// Computes `u64((a's word at literal indices) ^ (b's word at literal
/// indices))` for one word-sized chunk, where the word width is `u64`,
/// `u32`, `u16`, or `u8`. Every index is a source-level literal, so this is
/// panic-free (statically proven in-bounds) and loop-free (straight-line).
macro_rules! word_diff {
    ($a:ident, $b:ident, u64, [$($i:literal),+ $(,)?]) => {
        u64::from_ne_bytes([$($a[$i]),+]) ^ u64::from_ne_bytes([$($b[$i]),+])
    };
    ($a:ident, $b:ident, $ty:ident, [$($i:literal),+ $(,)?]) => {
        ($ty::from_ne_bytes([$($a[$i]),+]) ^ $ty::from_ne_bytes([$($b[$i]),+])) as u64
    };
}

/// Generates a loop-free, panic-free equality function for a `$n`-byte
/// buffer, comparing it as a fixed sequence of word-sized chunks (see the
/// [module docs](self)). Every index in every chunk is a literal, so the
/// body is straight-line code (no loop, no bounds-check panic path)
/// regardless of optimization level.
macro_rules! impl_buf_eq {
    ($name:ident, $n:literal, [ $( $ty:ident [ $($i:literal),+ $(,)? ] ),+ $(,)? ]) => {
        #[doc = concat!(
            "Loop-free, panic-free equality check for two ", stringify!($n),
            "-byte buffers. See the [module docs](self) for why this exists ",
            "instead of `a == b`."
        )]
        #[inline(always)]
        #[must_use]
        pub fn $name(a: &[u8; $n], b: &[u8; $n]) -> bool {
            let mut acc: u64 = 0;
            $( acc |= word_diff!(a, b, $ty, [$($i),+]); )+
            acc == 0
        }
    };
}

impl_buf_eq!(buf_eq_8, 8, [u64[0, 1, 2, 3, 4, 5, 6, 7]]);
impl_buf_eq!(
    buf_eq_20,
    20,
    [
        u64[0, 1, 2, 3, 4, 5, 6, 7],
        u64[8, 9, 10, 11, 12, 13, 14, 15],
        u32[16, 17, 18, 19],
    ]
);
impl_buf_eq!(
    buf_eq_32,
    32,
    [
        u64[0, 1, 2, 3, 4, 5, 6, 7],
        u64[8, 9, 10, 11, 12, 13, 14, 15],
        u64[16, 17, 18, 19, 20, 21, 22, 23],
        u64[24, 25, 26, 27, 28, 29, 30, 31],
    ]
);
impl_buf_eq!(
    buf_eq_33,
    33,
    [
        u64[0, 1, 2, 3, 4, 5, 6, 7],
        u64[8, 9, 10, 11, 12, 13, 14, 15],
        u64[16, 17, 18, 19, 20, 21, 22, 23],
        u64[24, 25, 26, 27, 28, 29, 30, 31],
        u8[32],
    ]
);
impl_buf_eq!(
    buf_eq_34,
    34,
    [
        u64[0, 1, 2, 3, 4, 5, 6, 7],
        u64[8, 9, 10, 11, 12, 13, 14, 15],
        u64[16, 17, 18, 19, 20, 21, 22, 23],
        u64[24, 25, 26, 27, 28, 29, 30, 31],
        u16[32, 33],
    ]
);
impl_buf_eq!(
    buf_eq_40,
    40,
    [
        u64[0, 1, 2, 3, 4, 5, 6, 7],
        u64[8, 9, 10, 11, 12, 13, 14, 15],
        u64[16, 17, 18, 19, 20, 21, 22, 23],
        u64[24, 25, 26, 27, 28, 29, 30, 31],
        u64[32, 33, 34, 35, 36, 37, 38, 39],
    ]
);
impl_buf_eq!(
    buf_eq_48,
    48,
    [
        u64[0, 1, 2, 3, 4, 5, 6, 7],
        u64[8, 9, 10, 11, 12, 13, 14, 15],
        u64[16, 17, 18, 19, 20, 21, 22, 23],
        u64[24, 25, 26, 27, 28, 29, 30, 31],
        u64[32, 33, 34, 35, 36, 37, 38, 39],
        u64[40, 41, 42, 43, 44, 45, 46, 47],
    ]
);
impl_buf_eq!(
    buf_eq_64,
    64,
    [
        u64[0, 1, 2, 3, 4, 5, 6, 7],
        u64[8, 9, 10, 11, 12, 13, 14, 15],
        u64[16, 17, 18, 19, 20, 21, 22, 23],
        u64[24, 25, 26, 27, 28, 29, 30, 31],
        u64[32, 33, 34, 35, 36, 37, 38, 39],
        u64[40, 41, 42, 43, 44, 45, 46, 47],
        u64[48, 49, 50, 51, 52, 53, 54, 55],
        u64[56, 57, 58, 59, 60, 61, 62, 63],
    ]
);

/// Loop-free, panic-free 160-bit big-endian ordering of two 20-byte buffers
/// (e.g. two [`crate::types::AccountId`]s).
///
/// Compares as three big-endian words (`u64` bytes 0..7, `u64` bytes 8..15,
/// `u32` bytes 16..19) from literal indices — straight-line, no loop, no
/// bounds-check panic path. Byte-lexicographic order on a 20-byte buffer is
/// exactly numeric 160-bit big-endian order, i.e. XRPL/Xahau's "high"/"low"
/// account ordering used to canonicalize a pair of accounts (e.g. picking
/// the low/high account of a `RippleState` trustline keylet).
#[inline(always)]
#[must_use]
pub fn buf_cmp_20(a: &[u8; 20], b: &[u8; 20]) -> core::cmp::Ordering {
    let a_hi = u64::from_be_bytes([a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]]);
    let b_hi = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    if a_hi != b_hi {
        return if a_hi < b_hi {
            core::cmp::Ordering::Less
        } else {
            core::cmp::Ordering::Greater
        };
    }

    let a_mid = u64::from_be_bytes([a[8], a[9], a[10], a[11], a[12], a[13], a[14], a[15]]);
    let b_mid = u64::from_be_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]);
    if a_mid != b_mid {
        return if a_mid < b_mid {
            core::cmp::Ordering::Less
        } else {
            core::cmp::Ordering::Greater
        };
    }

    let a_lo = u32::from_be_bytes([a[16], a[17], a[18], a[19]]);
    let b_lo = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
    a_lo.cmp(&b_lo)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8
    use super::*;

    /// Generic equal/unequal-at-every-position check, shared by the
    /// per-size tests below, cross-checked against `a == b` as the
    /// correctness oracle (array `==` is what this module exists to
    /// *avoid* inside a hook, but it's fine as a host-side reference).
    fn check_eq_and_all_single_byte_diffs<const N: usize>(eq: fn(&[u8; N], &[u8; N]) -> bool) {
        let a = [0x5Au8; N];
        let b = a;
        assert!(eq(&a, &b), "identical {N}-byte buffers must compare equal");
        assert_eq!(eq(&a, &b), a == b, "buf_eq_{N} must agree with a == b");

        for i in 0..N {
            let mut diff = a;
            diff[i] ^= 0xFF;
            assert!(
                !eq(&a, &diff),
                "single-byte difference at index {i} ({N} bytes) should be detected"
            );
            assert_eq!(
                eq(&a, &diff),
                a == diff,
                "buf_eq_{N} must agree with a == b at index {i}"
            );
        }
    }

    #[test]
    fn buf_eq_8_matches_slice_eq() {
        check_eq_and_all_single_byte_diffs(buf_eq_8);
    }

    #[test]
    fn buf_eq_20_matches_slice_eq() {
        check_eq_and_all_single_byte_diffs(buf_eq_20);
    }

    #[test]
    fn buf_eq_32_matches_slice_eq() {
        check_eq_and_all_single_byte_diffs(buf_eq_32);
    }

    #[test]
    fn buf_eq_33_matches_slice_eq() {
        check_eq_and_all_single_byte_diffs(buf_eq_33);
    }

    #[test]
    fn buf_eq_34_matches_slice_eq() {
        check_eq_and_all_single_byte_diffs(buf_eq_34);
    }

    #[test]
    fn buf_eq_40_matches_slice_eq() {
        check_eq_and_all_single_byte_diffs(buf_eq_40);
    }

    #[test]
    fn buf_eq_48_matches_slice_eq() {
        check_eq_and_all_single_byte_diffs(buf_eq_48);
    }

    #[test]
    fn buf_eq_64_matches_slice_eq() {
        check_eq_and_all_single_byte_diffs(buf_eq_64);
    }

    #[test]
    fn buf_eq_20_detects_difference_at_every_position() {
        let a = [0u8; 20];
        for i in 0..20 {
            let mut b = a;
            if let Some(byte) = b.get_mut(i) {
                *byte = 1;
            }
            assert!(
                !buf_eq_20(&a, &b),
                "difference at index {i} should be detected"
            );
        }
    }

    #[test]
    fn buf_eq_8_basic() {
        assert!(buf_eq_8(
            &[1, 2, 3, 4, 5, 6, 7, 8],
            &[1, 2, 3, 4, 5, 6, 7, 8]
        ));
        assert!(!buf_eq_8(
            &[1, 2, 3, 4, 5, 6, 7, 8],
            &[1, 2, 3, 4, 5, 6, 7, 9]
        ));
    }

    #[test]
    fn buf_eq_32_basic() {
        let a = [0xABu8; 32];
        let mut b = a;
        assert!(buf_eq_32(&a, &b));
        b[31] = 0xAC;
        assert!(!buf_eq_32(&a, &b));
    }

    #[test]
    fn buf_eq_33_basic() {
        let a = [0x11u8; 33];
        let mut b = a;
        assert!(buf_eq_33(&a, &b));
        b[0] = 0x12;
        assert!(!buf_eq_33(&a, &b));
    }

    #[test]
    fn buf_eq_34_basic() {
        let a = [0x22u8; 34];
        let mut b = a;
        assert!(buf_eq_34(&a, &b));
        b[17] = 0x23;
        assert!(!buf_eq_34(&a, &b));
    }

    #[test]
    fn buf_eq_40_basic() {
        let a = [0x44u8; 40];
        let mut b = a;
        assert!(buf_eq_40(&a, &b));
        b[39] = 0x45;
        assert!(!buf_eq_40(&a, &b));
    }

    #[test]
    fn buf_eq_48_basic() {
        let a = [0x33u8; 48];
        let mut b = a;
        assert!(buf_eq_48(&a, &b));
        b[47] = 0x34;
        assert!(!buf_eq_48(&a, &b));
    }

    #[test]
    fn buf_eq_64_basic() {
        let a = [0x55u8; 64];
        let mut b = a;
        assert!(buf_eq_64(&a, &b));
        b[63] = 0x56;
        assert!(!buf_eq_64(&a, &b));
    }

    #[test]
    fn buf_cmp_20_equal_buffers() {
        let a = [0x5Au8; 20];
        let b = a;
        assert_eq!(buf_cmp_20(&a, &b), core::cmp::Ordering::Equal);
        assert_eq!(buf_cmp_20(&a, &b), a.cmp(&b));
    }

    #[test]
    fn buf_cmp_20_agrees_with_array_ord_at_every_position() {
        let a = [0x5Au8; 20];
        for i in 0..20 {
            // a < b: bump byte i up (avoid overflow).
            let mut lo = a;
            lo[i] = 0x5B;
            assert_eq!(
                buf_cmp_20(&a, &lo),
                a.cmp(&lo),
                "buf_cmp_20 must agree with [u8; 20]::cmp at index {i} (a<b)"
            );
            assert_eq!(buf_cmp_20(&a, &lo), core::cmp::Ordering::Less);

            // a > b: bump byte i down.
            let mut hi = a;
            hi[i] = 0x59;
            assert_eq!(
                buf_cmp_20(&a, &hi),
                a.cmp(&hi),
                "buf_cmp_20 must agree with [u8; 20]::cmp at index {i} (a>b)"
            );
            assert_eq!(buf_cmp_20(&a, &hi), core::cmp::Ordering::Greater);
        }
    }

    /// Guards against a reordering of `buf_cmp_20`'s three word comparisons:
    /// the leading and trailing words are made to disagree (a's first word
    /// smaller, a's last word larger, and vice versa), so only the *first*
    /// differing word (index order 0 then 8 then 16) may decide the result.
    #[test]
    fn buf_cmp_20_first_differing_word_wins_even_when_later_words_disagree() {
        let mut a = [0x5Au8; 20];
        let mut b = [0x5Au8; 20];

        // a's leading word is smaller, a's trailing word is larger.
        a[0] = 0x10;
        b[0] = 0x20;
        a[19] = 0xFF;
        b[19] = 0x00;
        assert_eq!(buf_cmp_20(&a, &b), core::cmp::Ordering::Less);
        assert_eq!(buf_cmp_20(&a, &b), a.cmp(&b));

        // Mirror image: a's leading word is larger, a's trailing word is
        // smaller.
        let mut c = [0x5Au8; 20];
        let mut d = [0x5Au8; 20];
        c[0] = 0x20;
        d[0] = 0x10;
        c[19] = 0x00;
        d[19] = 0xFF;
        assert_eq!(buf_cmp_20(&c, &d), core::cmp::Ordering::Greater);
        assert_eq!(buf_cmp_20(&c, &d), c.cmp(&d));
    }
}
