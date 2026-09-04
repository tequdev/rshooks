//! Compile-time encoder backing the `XFL!` macro: turns a Rust numeric
//! literal's *text* into the 64-bit Xahau XFL bit pattern
//! `rshooks::xfl::XFL::from_raw_bits` expects.
//!
//! Parsed with pure integer/string arithmetic throughout -- **never**
//! `str::parse::<f64>()`. Round-tripping a decimal literal through `f64`
//! would silently reintroduce the binary-floating-point rounding error XFL
//! itself exists to avoid (e.g. `0.1` is not exactly representable in
//! `f64`), defeating the entire point of a bit-exact compile-time literal.
//!
//! See `rshooks::XFL!`'s doc comment for the full bit-layout writeup,
//! worked examples, and the `compile_fail` cases; this module only
//! concerns itself with the proc-macro plumbing and the pure text -> bits
//! algorithm.

use proc_macro::{Span, TokenStream, TokenTree};

use crate::err;

/// Unbiased exponent lower bound (stored field `1`, the minimum after the
/// 97 bias) -- mirrors `rshooks::xfl`'s module doc comment bit layout.
const MIN_UNBIASED_EXPONENT: i64 = -96;
/// Unbiased exponent upper bound (stored field `177`).
const MAX_UNBIASED_EXPONENT: i64 = 80;
/// Exponent field bias: stored = unbiased + 97.
const EXPONENT_BIAS: i64 = 97;
/// Bit offset of the exponent field.
const EXPONENT_SHIFT: u32 = 54;
/// Bit offset of the sign bit (`1` = positive, `0` = negative).
const SIGN_SHIFT: u32 = 62;
/// XFL's mantissa is always normalized to exactly this many significant
/// decimal digits.
const MANTISSA_DIGITS: usize = 16;

/// `XFL!`'s `#[proc_macro]` entry point. Walks the input for an optional
/// leading `-` `Punct` followed by exactly one numeric `Literal`, then
/// hands the literal's raw text off to [`encode`] -- the pure,
/// independently testable part of this module.
pub(crate) fn expand(input: TokenStream) -> TokenStream {
    const USAGE: &str = "XFL! expects a single numeric literal, e.g. XFL!(0.1)";

    let mut iter = input.into_iter();
    let mut negative = false;
    let mut next = iter.next();

    if let Some(TokenTree::Punct(p)) = &next
        && p.as_char() == '-'
    {
        negative = true;
        next = iter.next();
    }

    let literal = match next {
        Some(TokenTree::Literal(lit)) => lit,
        Some(other) => return err(other.span(), USAGE),
        None => return err(Span::call_site(), USAGE),
    };

    if let Some(extra) = iter.next() {
        return err(extra.span(), &format!("{USAGE} (unexpected extra tokens)"));
    }

    let span = literal.span();
    let bits = match encode(&literal.to_string(), negative) {
        Ok(bits) => bits,
        Err(msg) => return err(span, &msg),
    };

    // `bits` never sets bit 63 (see `encode`'s doc comment), so it always
    // fits a plain, unsuffixed positive `i64` literal.
    let src = format!("::rshooks::xfl::XFL::from_raw_bits({bits}i64)");
    src.parse::<TokenStream>()
        .unwrap_or_else(|_| err(span, "rshooks-macros: internal XFL! expansion failed"))
}

/// Parses a Rust numeric literal's `to_string()` text (e.g. `"0.1"`,
/// `"1_000"`, `"2.6E6"`) plus a separately-tracked sign into an XFL raw bit
/// pattern, or a human-readable error message on failure.
///
/// Pure string/integer arithmetic from end to end: `text` is split by hand
/// into an integer part, an optional fractional part, and an optional
/// exponent part, the two digit runs are concatenated into one
/// significant-digit string, then normalized -- leading/trailing zeros
/// stripped (bumping the exponent to compensate), checked against the
/// 16-significant-digit limit, and padded back out to exactly 16 digits --
/// before the exponent is range-checked and the bits assembled. Every
/// exponent adjustment uses saturating arithmetic, so a pathological input
/// like `1e99999999999999999999` is rejected as "too large" rather than
/// panicking or wrapping.
// Every `bytes[i]`/`&bytes[a..b]` below is reached only after a `i <
// bytes.len()` (or equivalent, already-checked-range) guard immediately
// before it -- always in bounds by construction, exactly like
// `base58::decode`'s identical allow.
#[allow(clippy::indexing_slicing)]
fn encode(text: &str, negative: bool) -> Result<u64, String> {
    const USAGE: &str = "XFL! expects a single numeric literal, e.g. XFL!(0.1)";

    let lower = text.to_ascii_lowercase();
    if lower.starts_with("0x") || lower.starts_with("0o") || lower.starts_with("0b") {
        return Err(format!(
            "{USAGE} (hexadecimal/octal/binary literals are not accepted -- \
             XFL! parses plain decimal text)"
        ));
    }
    match text.as_bytes().first() {
        Some(b) if b.is_ascii_digit() => {}
        _ => {
            return Err(format!(
                "{USAGE} (found a string, char, or byte literal -- XFL! only \
                 accepts a plain decimal number)"
            ));
        }
    }

    // Hand-scan the underscore-stripped text into (integer digits)
    // ['.' (fractional digits)] [('e'|'E') ['+'|'-'] exponent digits]
    // [suffix]. Underscores are stripped unconditionally before scanning
    // rather than validated for placement -- lossless for every literal
    // rustc will hand it.
    let stripped: String = text.chars().filter(|&c| c != '_').collect();
    let bytes = stripped.as_bytes();
    let mut i = 0usize;

    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let int_part = &stripped[int_start..i];
    if int_part.is_empty() {
        return Err(format!("{USAGE} (expected a decimal literal)"));
    }

    let mut frac_part = "";
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        frac_part = &stripped[frac_start..i];
    }

    let mut exp_value: i64 = 0;
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        let mut exp_negative = false;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            exp_negative = bytes[i] == b'-';
            i += 1;
        }
        let exp_digits_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_digits_start {
            return Err(format!("{USAGE} (malformed exponent)"));
        }
        // Saturating accumulation: an absurdly long exponent must be
        // rejected by the range check below, never panic or wrap.
        let mut acc: i64 = 0;
        for &b in &bytes[exp_digits_start..i] {
            let digit = i64::from(b - b'0');
            acc = acc.saturating_mul(10).saturating_add(digit);
        }
        exp_value = if exp_negative {
            acc.saturating_neg()
        } else {
            acc
        };
    }

    // Anything left over is a suffix (`1i64`, `1.0f64`, ...): rejected
    // outright, since XFL! always produces its own `i64` expansion.
    if i < bytes.len() {
        return Err(
            "XFL! does not accept a numeric literal suffix -- write the plain \
             decimal value, e.g. XFL!(1.5), not XFL!(1.5f64)"
                .to_string(),
        );
    }

    // Concatenate the integer and fractional digit runs into one
    // significant-digit string, adjusting the exponent by the fractional
    // digit count (each fractional digit divides by one further power of
    // ten).
    let mut digits: Vec<u8> = int_part.bytes().chain(frac_part.bytes()).collect();
    let frac_len = i64::try_from(frac_part.len()).unwrap_or(i64::MAX);
    let mut exponent = exp_value.saturating_sub(frac_len);

    // Strip leading zeros -- purely cosmetic, no exponent change.
    let Some(first_nonzero) = digits.iter().position(|&d| d != b'0') else {
        // Every digit is zero: canonical XFL zero, sign ignored -- matches
        // the reference vectors (`XFL!(0)`, `XFL!(0.0)`, `XFL!(-0)`, and
        // `XFL!(-0.0)` all encode to raw bits 0).
        return Ok(0);
    };
    digits.drain(..first_nonzero);

    // Strip trailing zeros, bumping the exponent once per digit removed so
    // `digits * 10^exponent` still equals the original value -- lets
    // `1.50`, `1_000`, and `2600000` normalize to their true
    // significant-digit count instead of false-tripping the limit below.
    while digits.last() == Some(&b'0') {
        digits.pop();
        exponent = exponent.saturating_add(1);
    }

    if digits.len() > MANTISSA_DIGITS {
        return Err(format!(
            "XFL holds at most {MANTISSA_DIGITS} significant decimal digits; \
             this literal has {} -- round it before using XFL!",
            digits.len()
        ));
    }

    // Pad back out to exactly 16 significant digits, dropping the exponent
    // by one per padding digit -- the mirror of the trailing-zero strip
    // above. `pad` is at most `MANTISSA_DIGITS` (16), so the `i64`
    // conversion below is always exact.
    let pad = MANTISSA_DIGITS - digits.len();
    digits.extend(core::iter::repeat_n(b'0', pad));
    exponent = exponent.saturating_sub(i64::try_from(pad).unwrap_or(i64::MAX));

    if exponent < MIN_UNBIASED_EXPONENT {
        return Err(format!(
            "XFL! value is too small to represent (the smallest positive \
             magnitude is 1e-81); this literal's effective exponent is {exponent}"
        ));
    }
    if exponent > MAX_UNBIASED_EXPONENT {
        return Err(format!(
            "XFL! value is too large to represent (the largest magnitude is \
             just under 1e96); this literal's effective exponent is {exponent}"
        ));
    }

    // `digits` is now exactly 16 ASCII digit bytes (`0`..=`9`), so this
    // accumulation can never exceed 9_999_999_999_999_999 -- well within
    // `u64`, no overflow possible.
    let mut mantissa: u64 = 0;
    for &d in &digits {
        mantissa = mantissa * 10 + u64::from(d - b'0');
    }

    // `exponent` was just range-checked into -96..=80, so `stored_exponent`
    // lands in 1..=177 -- always non-negative, so the cast below is exact.
    let stored_exponent = (exponent + EXPONENT_BIAS) as u64;
    let sign_bit: u64 = if negative { 0 } else { 1 };

    Ok((sign_bit << SIGN_SHIFT) | (stored_exponent << EXPONENT_SHIFT) | mantissa)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference vectors from the Xahau XFL reference implementation --
    /// every one of these must hold bit-for-bit.
    ///
    /// `XFL!(-1)`'s expected value here (`1_478_180_677_777_522_688`) is
    /// derived from xahaud's own source
    /// (`hook_float::make_float`/`set_sign`/`invert_sign` in
    /// `src/xrpld/app/hook/HookAPI.h`): negating an XFL is a pure XOR of bit
    /// 62 (`invert_sign`), leaving the exponent and mantissa fields
    /// bit-for-bit identical to the positive value -- so `XFL!(-1)` must
    /// equal `XFL!(1)` with only bit 62 cleared, i.e.
    /// `6_089_866_696_204_910_592 - (1u64 << 62)`.
    #[test]
    fn reference_vectors() {
        assert_eq!(encode("0", false), Ok(0));
        assert_eq!(encode("0.0", false), Ok(0));
        assert_eq!(encode("0", true), Ok(0));
        assert_eq!(encode("0.0", true), Ok(0));
        assert_eq!(encode("1", false), Ok(6_089_866_696_204_910_592));
        assert_eq!(encode("0.1", false), Ok(6_071_852_297_695_428_608));
        assert_eq!(encode("123456789", false), Ok(6_234_216_452_170_766_464));
        assert_eq!(encode("1", true), Ok(1_478_180_677_777_522_688));
        assert_eq!(
            encode("0.003333333333333333", false),
            Ok(6_038_156_834_009_797_973)
        );
        assert_eq!(encode("2600000", false), Ok(6_199_553_087_261_802_496));
    }

    #[test]
    fn trailing_zeros_normalize_to_the_same_value() {
        assert_eq!(encode("1.50", false), encode("1.5", false));
        assert_eq!(encode("1000", false), encode("1e3", false));
        assert_eq!(encode("2600000", false), encode("2.6e6", false));
        assert_eq!(encode("1.", false), encode("1", false));
        // 19 characters of fractional digits, only 3 of them significant
        // (`1`, `2`, `3`) once the seventeen trailing zeros are stripped --
        // must not false-trip the 16-significant-digit limit.
        assert_eq!(
            encode("1.2300000000000000000", false),
            encode("1.23", false)
        );
        assert!(encode("1.2300000000000000000", false).is_ok());
    }

    #[test]
    fn zero_significand_wins_over_exponent_overflow() {
        // The significand is all zeros, so the value is exactly zero
        // regardless of what the exponent says -- even an exponent so
        // large its digit run saturates the `i64` accumulator during
        // parsing. Zero must short-circuit before the (irrelevant, and
        // otherwise out-of-range) exponent is ever range-checked.
        assert_eq!(encode("0e99999999999999999999", false), Ok(0));
        assert_eq!(encode("0e99999999999999999999", true), Ok(0));
        assert_eq!(encode("0.000e-99999999999999999999", false), Ok(0));
    }

    /// Inputs whose *text* is well-formed enough to reach [`encode`] but is
    /// not a valid numeric literal by rustc's own grammar (a leading `.`
    /// with no integer part, or an exponent marker with no digits after
    /// it) -- these can never actually appear as `XFL!(..)`'s input in real
    /// source, since rustc's tokenizer rejects them before macro expansion
    /// ever runs. Covered here at the `encode()` level purely defensively:
    /// `encode` is a plain `&str -> Result` function and must not panic on
    /// any input, tokenizer artifact or not.
    #[test]
    fn malformed_exponent_and_leading_dot_text_is_rejected() {
        assert!(encode("1e", false).is_err());
        assert!(encode("1e+", false).is_err());
        assert!(encode("1e-", false).is_err());
        assert!(encode(".5", false).is_err());
    }

    #[test]
    fn underscores_are_ignored() {
        assert_eq!(encode("1_000", false), encode("1000", false));
        assert_eq!(encode("1_000.5_5", false), encode("1000.55", false));
        assert_eq!(encode("1e1_0", false), encode("1e10", false));
    }

    #[test]
    fn exponent_signs() {
        assert_eq!(encode("1e-5", false), encode("0.00001", false));
        assert_eq!(encode("1e+3", false), encode("1000", false));
        assert_eq!(encode("2.6E6", false), encode("2600000", false));
    }

    #[test]
    fn boundary_exponents() {
        // Smallest positive magnitude: 1e-81 is representable, 9.99e-82 is
        // not (its normalized exponent lands one below -96).
        assert!(encode("1e-81", false).is_ok());
        assert!(encode("9.99e-82", false).is_err());
        // Largest magnitude: an all-nines 16-digit mantissa at exponent 80
        // is representable, 1e96 is not (normalized exponent 81).
        assert!(encode("9.999999999999999e95", false).is_ok());
        assert!(encode("1e96", false).is_err());
    }

    #[test]
    fn too_many_significant_digits_is_rejected() {
        assert!(encode("12345678901234567", false).is_err()); // 17 digits
        assert!(encode("1.2345678901234567", false).is_err()); // 17 digits
        // 17 digits, but the last is a stripped trailing zero -> 16 sig
        // digits after normalization, so this one is fine.
        assert!(encode("12345678901234560", false).is_ok());
    }

    #[test]
    fn exponent_overflow_does_not_panic() {
        assert!(encode("1e99999999999999999999", false).is_err());
        assert!(encode("1e-99999999999999999999", false).is_err());
    }

    #[test]
    fn rejects_non_decimal_literal_kinds() {
        assert!(encode("\"0.1\"", false).is_err());
        assert!(encode("'a'", false).is_err());
        assert!(encode("b'a'", false).is_err());
        assert!(encode("0x1A", false).is_err());
        assert!(encode("0o17", false).is_err());
        assert!(encode("0b101", false).is_err());
    }

    #[test]
    fn rejects_suffixed_literals() {
        assert!(encode("1i64", false).is_err());
        assert!(encode("1.0f64", false).is_err());
        assert!(encode("1u32", false).is_err());
    }
}
