//! Integration coverage for the `XFL!` compile-time literal macro: every
//! published reference vector, `const`/negative/exponent/underscore usage,
//! and that the macro (`rshooks::XFL`) and the type it targets
//! (`rshooks::xfl::XFL`) coexist under `rshooks::prelude::*` without a
//! name clash -- an integration test, not an in-crate module, so the glob
//! import is exercised the way a hook crate actually sees it.
use rshooks::prelude::*;

#[test]
fn reference_vectors() {
    assert_eq!(XFL!(0).raw_bits(), 0);
    assert_eq!(XFL!(0.0).raw_bits(), 0);
    assert_eq!(XFL!(-0).raw_bits(), 0);
    assert_eq!(XFL!(-0.0).raw_bits(), 0);
    assert_eq!(XFL!(1).raw_bits(), 6_089_866_696_204_910_592);
    assert_eq!(XFL!(0.1).raw_bits(), 6_071_852_297_695_428_608);
    assert_eq!(XFL!(123456789).raw_bits(), 6_234_216_452_170_766_464);
    assert_eq!(XFL!(-1).raw_bits(), 1_478_180_677_777_522_688);
    assert_eq!(
        XFL!(0.003333333333333333).raw_bits(),
        6_038_156_834_009_797_973
    );
    assert_eq!(XFL!(2600000).raw_bits(), 6_199_553_087_261_802_496);
}

#[test]
fn const_and_static_usage() {
    const RATE: rshooks::xfl::XFL = XFL!(0.003333333333333333);
    static DELAY: rshooks::xfl::XFL = XFL!(2600000);
    assert_eq!(RATE.raw_bits(), 6_038_156_834_009_797_973);
    assert_eq!(DELAY.raw_bits(), 6_199_553_087_261_802_496);
}

#[test]
fn exponent_and_underscore_forms_match_their_plain_equivalent() {
    assert_eq!(XFL!(2.6e6).raw_bits(), XFL!(2600000).raw_bits());
    assert_eq!(XFL!(1e-5).raw_bits(), XFL!(0.00001).raw_bits());
    assert_eq!(XFL!(1_000_000).raw_bits(), XFL!(1000000).raw_bits());
    assert_eq!(XFL!(1_000.5).raw_bits(), XFL!(1000.5).raw_bits());
}

#[test]
fn negation_flips_only_the_sign_bit() {
    // Bit 62 is the sign bit (`1` = positive, see `rshooks::xfl`); negating
    // a literal must leave the exponent and mantissa bit-for-bit identical.
    let positive = XFL!(2600000).raw_bits();
    let negative = XFL!(-2600000).raw_bits();
    assert_eq!(negative, positive ^ (1i64 << 62));
}

#[test]
fn trailing_zeros_do_not_affect_the_encoding() {
    assert_eq!(XFL!(1.50).raw_bits(), XFL!(1.5).raw_bits());
    assert_eq!(XFL!(1.).raw_bits(), XFL!(1).raw_bits());
    // 19 fractional digits, only 3 significant once trailing zeros are
    // stripped -- must not false-trip the 16-significant-digit limit.
    assert_eq!(
        XFL!(1.2300000000000000000).raw_bits(),
        XFL!(1.23).raw_bits()
    );
}

#[test]
fn zero_significand_wins_over_exponent_overflow() {
    // An all-zero significand is exactly zero regardless of the exponent --
    // even one long enough to saturate the parser's `i64` accumulator. Zero
    // must short-circuit before that (otherwise out-of-range) exponent is
    // ever range-checked.
    assert_eq!(XFL!(0e99999999999999999999).raw_bits(), 0);
}
