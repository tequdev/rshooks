//! `float_*`/`slot_float` semantics (P2-B —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "float_*", stage plan §7).
//!
//! Pure functions over the XFL `i64` bit pattern; no [`crate::world::World`]
//! / [`crate::invocation::InvocationContext`] coupling (float ops need no
//! world state — design §4). Every function is a line-for-line Rust port of
//! xahaud's real implementation (`Xahau/xahaud`, branch `dev`); where it
//! disagrees with the `hook-api` skill's prose summary, this module follows
//! the C++ source and says so in a comment.
//!
//! Source of truth:
//! - `src/xrpld/app/hook/detail/HookAPI.cpp` (`HookAPI::float_*`) and
//!   `src/xrpld/app/hook/HookAPI.h` (`namespace hook_float` — bit
//!   accessors, `normalize_xfl`, `make_float`) — the `float_*` bodies.
//! - `src/xrpld/app/hook/detail/applyHook.cpp` (`RETURN_IF_INVALID_FLOAT`)
//!   — the operand-validation macro every wasm-facing `float_*` wrapper
//!   applies before calling into `HookAPI`.
//! - `include/xrpl/protocol/IOUAmount.h`/`.cpp` (`IOUAmount::normalize`,
//!   `operator+=`, `operator<`, `mulRatio`) — `float_sum`/`float_compare`/
//!   `float_mulratio` all construct an `IOUAmount` internally and inherit
//!   its arithmetic.
//! - `include/xrpl/basics/Number.h`/`.cpp` (`Number::normalize`,
//!   `operator+=`) — `IOUAmount` delegates to `Number` whenever
//!   `STNumberSwitchover` is on, which it always is on the hook execution
//!   path (`LocalValue<bool> r{true}` in `Number.cpp`, never toggled).
//!   `Number` is the one place real rounding happens (round-to-nearest-even
//!   via 16 packed guard digits); `float_multiply`/`float_int`/`float_sto`
//!   use their own bespoke truncating integer math instead, ported
//!   separately below.
//!
//! # Non-obvious quirk kept byte-for-byte on purpose
//!
//! [`hook_float_is_negative`] mirrors `hook_float::is_negative` exactly,
//! including for canonical zero (`0`): the raw bit-62 check reports it as
//! "negative" (`(0 >> 62) & 1 == 0`). Every real call site in `HookAPI.cpp`
//! except two short-circuits on `float1 == 0` before calling `is_negative`,
//! so the quirk is inert there. The two that don't short-circuit:
//! - `float_compare`: harmless — the mantissa is also `0` for a zero
//!   operand, so `0 * -1 == 0 * 1 == 0` either way.
//! - `float_sto`'s native (`is_xrp`) branch: not harmless — encoding
//!   canonical-zero XFL as a native amount produces 8 all-zero bytes (no
//!   `0b0100_0000` "non-negative" flag), unlike every other native-zero
//!   encoder in this codebase (e.g. `txn::codec::encode_native_amount_const
//!   (0)`, which always sets that flag). This module reproduces that exact
//!   byte sequence — see [`float_sto`]'s test
//!   `native_zero_encodes_as_all_zero_bytes_not_positive_zero`.
//!
//! Every other call site uses [`get_mantissa`]/[`get_exponent`] (which fold
//! the zero case in themselves) or calls [`hook_float_is_negative`] only
//! after already establishing the operand is non-zero.

// Value-range-bounded arithmetic port of xahaud's XFL engine
// (`hook_float::*`, `ripple::Number`/`IOUAmount`): every `+`/`-`/`*`/`/`
// mirrors one line of the cited C++ source, operating on quantities bounded
// by XFL's own fixed limits (16-digit mantissas, `-96..=80` exponents), same
// as the C++. Switching to `checked_*`/`wrapping_*` would break that
// line-for-line correspondence without adding safety: every place overflow
// is reachable already has an explicit range check next to it (e.g.
// `normalize_xfl`'s `adjust > 18`/`-adjust > 18` guards before the risky
// multiply/divide). A blanket module-level allow (matching
// `rshooks_macros`'s own `lib.rs`) beats scattering `#[allow]` on 80+
// individual arithmetic lines.
#![allow(clippy::arithmetic_side_effects)]

use std::vec::Vec;

use rshooks_core::{
    CANT_RETURN_NEGATIVE, DIVISION_BY_ZERO, INVALID_ARGUMENT, INVALID_FLOAT, TOO_BIG, XFL_OVERFLOW,
};

// ---------------------------------------------------------------------
// XFL bit-pattern constants and accessors (`hook_float::` in HookAPI.h).
// ---------------------------------------------------------------------

/// `hook_float::minMantissa`.
const MIN_MANTISSA: u64 = 1_000_000_000_000_000;
/// `hook_float::maxMantissa`.
const MAX_MANTISSA: u64 = 9_999_999_999_999_999;
/// `hook_float::minExponent`.
const MIN_EXPONENT: i32 = -96;
/// `hook_float::maxExponent`.
const MAX_EXPONENT: i32 = 80;
/// Bias applied to the stored 8-bit exponent field.
const EXPONENT_BIAS: i32 = 97;
/// Bit offset of the exponent field.
const EXPONENT_SHIFT: u32 = 54;
/// Bit offset of the sign bit (`1` = positive, `0` = negative).
const SIGN_SHIFT: u32 = 62;
/// Mask for the 54-bit mantissa field (bits 0..=53).
const MANTISSA_MASK: u64 = (1u64 << EXPONENT_SHIFT) - 1;

/// `hook_float::power_of_ten` (indices `0..=18`, `10^18` fits `u64`).
const POWERS_OF_TEN: [u64; 19] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
    10_000_000_000,
    100_000_000_000,
    1_000_000_000_000,
    10_000_000_000_000,
    100_000_000_000_000,
    1_000_000_000_000_000,
    10_000_000_000_000_000,
    100_000_000_000_000_000,
    1_000_000_000_000_000_000,
];

/// `hook_float::get_mantissa` — folds in the `float1 == 0` shortcut the C++
/// function itself applies (not call-site-dependent, unlike
/// [`hook_float_is_negative`] — see the module doc comment).
fn get_mantissa(bits: i64) -> u64 {
    if bits == 0 {
        0
    } else {
        (bits as u64) & MANTISSA_MASK
    }
}

/// `hook_float::get_exponent` — see [`get_mantissa`]'s doc comment.
fn get_exponent(bits: i64) -> i32 {
    if bits == 0 {
        0
    } else {
        (((bits as u64) >> EXPONENT_SHIFT) & 0xFF) as i32 - EXPONENT_BIAS
    }
}

/// `hook_float::is_negative`, verbatim (including the `bits == 0`
/// quirk) — see the module doc comment for which call sites this is safe
/// to use directly on a not-yet-zero-checked operand.
fn hook_float_is_negative(bits: i64) -> bool {
    ((bits as u64) >> SIGN_SHIFT) & 1 == 0
}

/// `hook_float::invert_sign` — flips bit 62.
fn invert_sign(bits: i64) -> i64 {
    (bits as u64 ^ (1u64 << SIGN_SHIFT)) as i64
}

/// `RETURN_IF_INVALID_FLOAT` (applyHook.cpp): `bits < 0` is never a valid
/// float (that channel is reserved for error codes); `bits == 0` (canonical
/// zero) is always valid; otherwise both fields must be in range.
fn is_valid(bits: i64) -> bool {
    if bits < 0 {
        return false;
    }
    if bits == 0 {
        return true;
    }
    let mantissa = (bits as u64) & MANTISSA_MASK;
    let exponent = (((bits as u64) >> EXPONENT_SHIFT) & 0xFF) as i32 - EXPONENT_BIAS;
    (MIN_MANTISSA..=MAX_MANTISSA).contains(&mantissa)
        && (MIN_EXPONENT..=MAX_EXPONENT).contains(&exponent)
}

/// `hook_float::make_float(uint64_t mantissa, int32_t exponent, bool neg)`.
/// Internal-only error codes (`MANTISSA_OVERSIZED`/`UNDERSIZED`/
/// `EXPONENT_OVERSIZED`/`UNDERSIZED`) are real `rshooks_core` constants and
/// real `rshooks::error::HookError` variants, so propagating them verbatim
/// (rather than remapping) is safe wherever the C++ itself doesn't remap.
fn make_float(mantissa: u64, exponent: i32, neg: bool) -> Result<i64, i64> {
    if mantissa == 0 {
        return Ok(0);
    }
    if mantissa > MAX_MANTISSA {
        return Err(rshooks_core::MANTISSA_OVERSIZED);
    }
    if mantissa < MIN_MANTISSA {
        return Err(rshooks_core::MANTISSA_UNDERSIZED);
    }
    if exponent > MAX_EXPONENT {
        return Err(rshooks_core::EXPONENT_OVERSIZED);
    }
    if exponent < MIN_EXPONENT {
        return Err(rshooks_core::EXPONENT_UNDERSIZED);
    }
    let stored_exponent = (exponent + EXPONENT_BIAS) as u64;
    let mut out: u64 = mantissa | (stored_exponent << EXPONENT_SHIFT);
    // `set_sign`: the fields above always leave bit 62 clear (= "negative"
    // by `is_negative`'s convention), so only a *positive* result needs the
    // bit flipped.
    if !neg {
        out ^= 1u64 << SIGN_SHIFT;
    }
    Ok(out as i64)
}

// ---------------------------------------------------------------------
// `hook_float::normalize_xfl<uint64_t>` — used by `float_multiply`,
// `float_divide` (defensively — see `float_divide`'s doc comment).
// ---------------------------------------------------------------------

/// Outcome of [`normalize_xfl`]: mirrors the C++ function's three possible
/// results (success, underflow-to-canonical-zero, `XFL_OVERFLOW`).
enum Normalized {
    Value(i64),
    Overflow,
}

/// `hook_float::normalize_xfl(uint64_t& man, int32_t& exp, bool neg)`. `man`
/// is always a non-negative magnitude here (the unsigned instantiation —
/// `float_set`'s signed-mantissa instantiation is inlined directly into
/// [`float_set`] instead, since it is the only caller of that variant).
#[allow(clippy::indexing_slicing)] // `POWERS_OF_TEN[adjust]`/`[shrink]` are reached only after `adjust <= 18`/`shrink <= 18` guards immediately above — `POWERS_OF_TEN` has 19 entries (indices 0..=18)
fn normalize_xfl(mut man: u64, mut exp: i32, neg: bool) -> Normalized {
    if man == 0 {
        return Normalized::Value(0);
    }
    // `int32_t mo = log10(man);` — C++ truncates toward zero; for `man >= 1`
    // (guaranteed here) that coincides with `floor`. `f64::log10` here is
    // deliberate, boundary imprecision included — the min/max-mantissa
    // correction branches below exist to self-heal a mis-estimated `mo`
    // ("even after adjustment the mantissa can be outside the range by one
    // place", per the C++ comment this ports).
    let mo = (man as f64).log10() as i32;
    let adjust = 15 - mo;
    if adjust > 0 {
        if adjust > 18 {
            return Normalized::Value(0); // defensive, unreachable for real inputs
        }
        man = man.saturating_mul(POWERS_OF_TEN[adjust as usize]);
        exp -= adjust;
    } else if adjust < 0 {
        let shrink = -adjust;
        if shrink > 18 {
            return Normalized::Overflow; // defensive, unreachable for real inputs
        }
        man /= POWERS_OF_TEN[shrink as usize];
        exp -= adjust;
    }
    if man == 0 {
        return Normalized::Value(0);
    }
    if man < MIN_MANTISSA {
        if man == MIN_MANTISSA - 1 {
            man += 1;
        } else {
            man *= 10;
            exp -= 1;
        }
    }
    if man > MAX_MANTISSA {
        if man == MAX_MANTISSA + 1 {
            man -= 1;
        } else {
            man /= 10;
            exp += 1;
        }
    }
    if exp < MIN_EXPONENT {
        return Normalized::Value(0);
    }
    if exp > MAX_EXPONENT {
        return Normalized::Overflow;
    }
    match make_float(man, exp, neg) {
        Ok(bits) => Normalized::Value(bits),
        Err(_) => Normalized::Overflow, // unreachable: man/exp already clamped in range above
    }
}

// ---------------------------------------------------------------------
// `ripple::Number` emulation — round-to-nearest-even via 16 packed guard
// digits (`Number::Guard`, `Number::normalize`, `Number::operator+=`).
// Used by `float_sum`, `float_compare` (via [`iou_new`]/[`number_lt`]),
// and `float_mulratio` (via [`iou_new`]) — every place xahaud itself
// constructs a `ripple::IOUAmount`/`ripple::Number`.
// ---------------------------------------------------------------------

/// `Number::minExponent`/`maxExponent` — deliberately much wider than XFL's
/// own `-96..=80`; `IOUAmount::normalize` narrows to that range itself
/// *after* delegating to `Number` (see [`iou_new`]).
const NUMBER_MIN_EXPONENT: i32 = -32768;
const NUMBER_MAX_EXPONENT: i32 = 32768;

/// `Number::Guard` — a 16-decimal-digit shift register recording every
/// digit shifted off during normalization, used only to decide the
/// round-to-nearest-even correction. Ported field-for-field from
/// `Number.cpp`; `xbit_`/`sbit_` are plain `bool` here (the C++ bitfields
/// are a size optimization this port doesn't need).
struct Guard {
    digits: u64,
    xbit: bool,
    negative: bool,
}

impl Guard {
    fn new() -> Self {
        Guard {
            digits: 0,
            xbit: false,
            negative: false,
        }
    }

    fn set_negative(&mut self) {
        self.negative = true;
    }

    fn push(&mut self, d: u32) {
        self.xbit |= (self.digits & 0xF) != 0;
        self.digits >>= 4;
        self.digits |= (u64::from(d) & 0xF) << 60;
    }

    fn pop(&mut self) -> u64 {
        let d = (self.digits & 0xF000_0000_0000_0000) >> 60;
        self.digits <<= 4;
        d
    }

    /// `Number::Guard::round` under the default (and only, in this
    /// codebase — `Number::mode_` is never changed on the hook-execution
    /// path) `to_nearest` mode: `1` rounds up, `-1` rounds down/truncates,
    /// `0` is an exact tie (caller breaks it by rounding to even).
    fn round(&self) -> i32 {
        match self.digits.cmp(&0x5000_0000_0000_0000) {
            core::cmp::Ordering::Greater => 1,
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => {
                if self.xbit {
                    1
                } else {
                    0
                }
            }
        }
    }
}

/// A `ripple::Number`-shaped value: `Zero` (the sentinel `Number{}` —
/// mantissa `0`, exponent `i32::MIN`, matching `numeric_limits<int>::
/// lowest()`) or a normalized `(mantissa, exponent)` pair.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NumberValue {
    Zero,
    Normal(i64, i32),
}

/// `Number::normalize()`, applied to a raw `(mantissa, exponent)` pair
/// (mirrors the `Number(rep, int)` constructor, which calls this
/// unconditionally). `None` is `Number::normalize`'s
/// `throw std::overflow_error`.
fn number_normalize(mantissa: i64, exponent: i32) -> Option<NumberValue> {
    if mantissa == 0 {
        return Some(NumberValue::Zero);
    }
    let negative = mantissa < 0;
    let mut m: u64 = mantissa.unsigned_abs();
    let mut exp = exponent;
    while m < MIN_MANTISSA && exp > NUMBER_MIN_EXPONENT {
        m *= 10;
        exp -= 1;
    }
    let mut g = Guard::new();
    if negative {
        g.set_negative();
    }
    while m > MAX_MANTISSA {
        if exp >= NUMBER_MAX_EXPONENT {
            return None;
        }
        g.push((m % 10) as u32);
        m /= 10;
        exp += 1;
    }
    if exp < NUMBER_MIN_EXPONENT || m < MIN_MANTISSA {
        return Some(NumberValue::Zero);
    }
    let r = g.round();
    if r == 1 || (r == 0 && (m & 1) == 1) {
        m += 1;
        if m > MAX_MANTISSA {
            m /= 10;
            exp += 1;
        }
    }
    if exp > NUMBER_MAX_EXPONENT {
        return None;
    }
    let signed = if negative { -(m as i64) } else { m as i64 };
    Some(NumberValue::Normal(signed, exp))
}

/// `Number::operator+=`, including its own `y == 0`/`x == 0`/`x == -y`
/// shortcuts. `None` is `Number::addition overflow`.
fn number_add(x: NumberValue, y: NumberValue) -> Option<NumberValue> {
    let (xm0, xe0) = match x {
        NumberValue::Zero => return Some(y),
        NumberValue::Normal(m, e) => (m, e),
    };
    let (ym0, ye0) = match y {
        NumberValue::Zero => return Some(x),
        NumberValue::Normal(m, e) => (m, e),
    };
    if xe0 == ye0 && xm0 == -ym0 {
        return Some(NumberValue::Zero);
    }

    let (mut xm, xn): (u64, i64) = if xm0 < 0 {
        (xm0.unsigned_abs(), -1)
    } else {
        (xm0 as u64, 1)
    };
    let mut xe = xe0;
    let (mut ym, yn): (u64, i64) = if ym0 < 0 {
        (ym0.unsigned_abs(), -1)
    } else {
        (ym0 as u64, 1)
    };
    let mut ye = ye0;

    let mut g = Guard::new();
    if xe < ye {
        if xn == -1 {
            g.set_negative();
        }
        loop {
            g.push((xm % 10) as u32);
            xm /= 10;
            xe += 1;
            if xe >= ye {
                break;
            }
        }
    } else if xe > ye {
        if yn == -1 {
            g.set_negative();
        }
        loop {
            g.push((ym % 10) as u32);
            ym /= 10;
            ye += 1;
            if ye >= xe {
                break;
            }
        }
    }

    if xn == yn {
        xm += ym;
        if xm > MAX_MANTISSA {
            g.push((xm % 10) as u32);
            xm /= 10;
            xe += 1;
        }
        let r = g.round();
        if r == 1 || (r == 0 && (xm & 1) == 1) {
            xm += 1;
            if xm > MAX_MANTISSA {
                xm /= 10;
                xe += 1;
            }
        }
        if xe > NUMBER_MAX_EXPONENT {
            return None;
        }
    } else {
        let mut xn = xn;
        if xm > ym {
            xm -= ym;
        } else {
            xm = ym - xm;
            xe = ye;
            xn = yn;
        }
        while xm < MIN_MANTISSA {
            xm *= 10;
            xm = xm.saturating_sub(g.pop());
            xe -= 1;
        }
        let r = g.round();
        if r == 1 || (r == 0 && (xm & 1) == 1) {
            xm -= 1;
            if xm < MIN_MANTISSA {
                xm *= 10;
                xe -= 1;
            }
        }
        if xe < NUMBER_MIN_EXPONENT {
            return Some(NumberValue::Zero);
        }
        let signed = if xn == -1 { -(xm as i64) } else { xm as i64 };
        return Some(NumberValue::Normal(signed, xe));
    }

    let signed = if xn == -1 { -(xm as i64) } else { xm as i64 };
    Some(NumberValue::Normal(signed, xe))
}

/// `Number::operator<` (friend function in `Number.h`), applied directly to
/// two [`NumberValue`]s — used by `float_compare`.
fn number_lt(x: NumberValue, y: NumberValue) -> bool {
    let (xm, xe) = match x {
        NumberValue::Zero => (0i64, i32::MIN),
        NumberValue::Normal(m, e) => (m, e),
    };
    let (ym, ye) = match y {
        NumberValue::Zero => (0i64, i32::MIN),
        NumberValue::Normal(m, e) => (m, e),
    };
    let lneg = xm < 0;
    let rneg = ym < 0;
    if lneg != rneg {
        return lneg;
    }
    if xm == 0 {
        return ym > 0;
    }
    if ym == 0 {
        return false;
    }
    if xe > ye {
        return lneg;
    }
    if xe < ye {
        return !lneg;
    }
    xm < ym
}

/// A `ripple::IOUAmount`-shaped `(mantissa, exponent)` pair, always already
/// clamped to IOUAmount's own (narrower-than-`Number`) exponent range.
#[derive(Clone, Copy, PartialEq, Eq)]
struct IouPair {
    mantissa: i64,
    exponent: i32,
}

/// `IOUAmount`'s canonical zero (`operator=(beast::Zero)`): mantissa `0`,
/// exponent `-100` (chosen upstream so it sorts below any real small
/// positive amount — see `IOUAmount.h`'s comment on that constant).
const IOU_ZERO: IouPair = IouPair {
    mantissa: 0,
    exponent: -100,
};

/// `IOUAmount(std::int64_t mantissa, int exponent)`, i.e. `IOUAmount::
/// normalize()`'s `getSTNumberSwitchover() == true` branch (the only branch
/// ever taken — see the module doc comment): normalize via [`Number`],
/// then re-clamp to IOUAmount's own `-96..=80` exponent range.
fn iou_new(mantissa: i64, exponent: i32) -> Result<IouPair, i64> {
    let Some(nv) = number_normalize(mantissa, exponent) else {
        return Err(XFL_OVERFLOW); // `Number::normalize`'s "value overflow"
    };
    match nv {
        NumberValue::Zero => Ok(IOU_ZERO),
        NumberValue::Normal(_, e) if e > MAX_EXPONENT => Err(XFL_OVERFLOW),
        NumberValue::Normal(_, e) if e < MIN_EXPONENT => Ok(IOU_ZERO),
        NumberValue::Normal(m, e) => Ok(IouPair {
            mantissa: m,
            exponent: e,
        }),
    }
}

/// `IOUAmount(Number const&)`: takes an already `Number`-normalized value
/// and only applies IOUAmount's own exponent clamp (no second `Number`
/// normalization pass) — used for `float_sum`'s addition result, which
/// arrives pre-normalized from [`number_add`].
fn iou_from_number(nv: NumberValue) -> Result<IouPair, i64> {
    match nv {
        NumberValue::Zero => Ok(IOU_ZERO),
        NumberValue::Normal(_, e) if e > MAX_EXPONENT => Err(XFL_OVERFLOW),
        NumberValue::Normal(_, e) if e < MIN_EXPONENT => Ok(IOU_ZERO),
        NumberValue::Normal(m, e) => Ok(IouPair {
            mantissa: m,
            exponent: e,
        }),
    }
}

fn iou_to_number(p: IouPair) -> NumberValue {
    if p.mantissa == 0 {
        NumberValue::Zero
    } else {
        NumberValue::Normal(p.mantissa, p.exponent)
    }
}

// ---------------------------------------------------------------------
// `float_*` bodies (`HookAPI::float_*`).
// ---------------------------------------------------------------------

/// `HookAPI::float_set` (signed-mantissa `normalize_xfl` instantiation
/// inlined here, since `float_set` is its only caller).
pub(crate) fn float_set(exponent: i32, mantissa: i64) -> i64 {
    if mantissa == 0 {
        return 0;
    }
    let neg = mantissa < 0;
    // C++ special-cases `mantissa == i64::MIN` with `man++` before negating
    // (plain `-man` overflows `int64_t`); `i64::unsigned_abs` handles
    // `i64::MIN` directly (`u64` has the range), and `normalize_xfl`
    // renormalizes any magnitude to 16 significant digits regardless, so the
    // two approaches are observationally identical here.
    let mag = mantissa.unsigned_abs();
    match normalize_xfl(mag, exponent, neg) {
        Normalized::Overflow => INVALID_FLOAT, // C++ remaps XFL_OVERFLOW -> INVALID_FLOAT here
        Normalized::Value(0) => INVALID_FLOAT, // underflow must be reported as an error for float_set
        Normalized::Value(bits) => bits,
    }
}

/// `HookAPI::float_multiply` + `float_multiply_internal_parts`.
pub(crate) fn float_multiply(f1: i64, f2: i64) -> i64 {
    if !is_valid(f1) || !is_valid(f2) {
        return INVALID_FLOAT;
    }
    if f1 == 0 || f2 == 0 {
        return 0;
    }
    let man1 = u128::from(get_mantissa(f1));
    let exp1 = get_exponent(f1);
    let neg1 = hook_float_is_negative(f1);
    let man2 = u128::from(get_mantissa(f2));
    let exp2 = get_exponent(f2);
    let neg2 = hook_float_is_negative(f2);

    let mult = (man1 * man2) / u128::from(POWERS_OF_TEN[15]);
    let man_out = mult as u64;
    if mult > u128::from(man_out) {
        return XFL_OVERFLOW;
    }
    let exp_out = exp1 + exp2 + 15;
    let neg_out = neg1 != neg2;
    match normalize_xfl(man_out, exp_out, neg_out) {
        Normalized::Overflow => XFL_OVERFLOW,
        Normalized::Value(bits) => bits,
    }
}

/// `ripple::mulRatio` (`IOUAmount.cpp`) restricted to the always-non-negative
/// mantissa xahaud's call site (`HookAPI::mulratio_internal`) passes — the
/// real function's `!roundUp && neg` rounding branch is unreachable there
/// and not ported (a documented simplification, not a guess).
#[allow(clippy::indexing_slicing)] // `POWER_TABLE[i]`/`log10_ceil`'s index are bounded by the `i < 30` loop guard; `room_to_grow`/`must_shrink` are each `FL64 - log10_ceil(..)` (or the reverse), so bounded to `0..=30` and only indexed after a `> 0` guard
fn mul_ratio(
    mantissa: u64,
    exponent: i32,
    round_up: bool,
    num: u32,
    den: u32,
) -> Result<IouPair, i64> {
    // `powerTable[i] = 10^i` for `i in 0..30` (`2^96 < 10^29`).
    const POWER_TABLE: [u128; 30] = {
        let mut t = [0u128; 30];
        let mut i = 0;
        let mut cur: u128 = 1;
        while i < 30 {
            t[i] = cur;
            cur *= 10;
            i += 1;
        }
        t
    };
    /// `lower_bound`-based `log10Floor`/`log10Ceil` from `IOUAmount.cpp`.
    fn log10_ceil(v: u128) -> i32 {
        let mut idx = 0usize;
        while idx < POWER_TABLE.len() && POWER_TABLE[idx] < v {
            idx += 1;
        }
        idx as i32
    }

    const FL64: i32 = 18; // log10Floor(i64::MAX) — precomputed, matches IOUAmount.cpp's `fl64`

    let den128 = u128::from(den);
    let mul = u128::from(mantissa) * u128::from(num);
    let mut low = mul / den128;
    let mut rem = mul - low * den128;
    let mut exponent = exponent;

    if rem != 0 {
        let room_to_grow = FL64 - log10_ceil(low);
        if room_to_grow > 0 {
            exponent -= room_to_grow;
            low *= POWER_TABLE[room_to_grow as usize];
            rem *= POWER_TABLE[room_to_grow as usize];
        }
        let add_rem = rem / den128;
        low += add_rem;
        rem -= add_rem * den128;
    }

    let mut has_rem = rem != 0;
    let must_shrink = log10_ceil(low) - FL64;
    if must_shrink > 0 {
        let sav = low;
        exponent += must_shrink;
        low /= POWER_TABLE[must_shrink as usize];
        if !has_rem {
            has_rem = sav - low * POWER_TABLE[must_shrink as usize] != 0;
        }
    }

    // `low` fits `i64` by construction (shrunk to at most `fl64` digits).
    let mantissa_i64 = low as i64;
    let result = iou_new(mantissa_i64, exponent)?;

    if has_rem && round_up {
        return if result.mantissa == 0 {
            Ok(IouPair {
                mantissa: MIN_MANTISSA as i64,
                exponent: MIN_EXPONENT,
            }) // IOUAmount::minPositiveAmount()
        } else {
            iou_new(result.mantissa + 1, result.exponent)
        };
    }
    Ok(result)
}

/// `HookAPI::float_mulratio` + `mulratio_internal`.
pub(crate) fn float_mulratio(f1: i64, round_up: u32, numerator: u32, denominator: u32) -> i64 {
    if !is_valid(f1) {
        return INVALID_FLOAT;
    }
    if f1 == 0 {
        return 0;
    }
    if denominator == 0 {
        return DIVISION_BY_ZERO;
    }
    let man1 = get_mantissa(f1);
    let exp1 = get_exponent(f1);
    let out = match mul_ratio(man1, exp1, round_up != 0, numerator, denominator) {
        Ok(p) => p,
        Err(_) => return XFL_OVERFLOW,
    };
    let mag = out.mantissa.unsigned_abs();
    match make_float(mag, out.exponent, hook_float_is_negative(f1)) {
        Ok(bits) => bits,
        Err(e) => e,
    }
}

/// `HookAPI::float_negate` — canonical zero stays zero (no sign to flip).
pub(crate) fn float_negate(f1: i64) -> i64 {
    if !is_valid(f1) {
        return INVALID_FLOAT;
    }
    if f1 == 0 { 0 } else { invert_sign(f1) }
}

/// `HookAPI::float_compare`.
pub(crate) fn float_compare(f1: i64, f2: i64, mode: u32) -> i64 {
    if !is_valid(f1) || !is_valid(f2) {
        return INVALID_FLOAT;
    }
    let equal_flag = mode & rshooks_core::COMPARE_EQUAL != 0;
    let less_flag = mode & rshooks_core::COMPARE_LESS != 0;
    let greater_flag = mode & rshooks_core::COMPARE_GREATER != 0;
    let not_equal = less_flag && greater_flag;

    if (equal_flag && less_flag && greater_flag) || mode == 0 {
        return INVALID_ARGUMENT;
    }
    if mode & !0b111 != 0 {
        return INVALID_ARGUMENT;
    }

    let man1 = (get_mantissa(f1) as i64) * if hook_float_is_negative(f1) { -1 } else { 1 };
    let exp1 = get_exponent(f1);
    let man2 = (get_mantissa(f2) as i64) * if hook_float_is_negative(f2) { -1 } else { 1 };
    let exp2 = get_exponent(f2);

    let amt1 = match iou_new(man1, exp1) {
        Ok(p) => p,
        Err(_) => return XFL_OVERFLOW,
    };
    let amt2 = match iou_new(man2, exp2) {
        Ok(p) => p,
        Err(_) => return XFL_OVERFLOW,
    };

    // `IOUAmount::operator==` is a direct field compare (no `Number`
    // involved) — see the module doc comment.
    let eq = amt1 == amt2;
    let lt = number_lt(iou_to_number(amt1), iou_to_number(amt2));
    let gt = number_lt(iou_to_number(amt2), iou_to_number(amt1));

    if not_equal && !eq {
        return 1;
    }
    if equal_flag && eq {
        return 1;
    }
    if greater_flag && gt {
        return 1;
    }
    if less_flag && lt {
        return 1;
    }
    0
}

/// `HookAPI::float_sum`.
pub(crate) fn float_sum(f1: i64, f2: i64) -> i64 {
    if !is_valid(f1) || !is_valid(f2) {
        return INVALID_FLOAT;
    }
    if f1 == 0 {
        return f2;
    }
    if f2 == 0 {
        return f1;
    }

    let man1 = (get_mantissa(f1) as i64) * if hook_float_is_negative(f1) { -1 } else { 1 };
    let exp1 = get_exponent(f1);
    let man2 = (get_mantissa(f2) as i64) * if hook_float_is_negative(f2) { -1 } else { 1 };
    let exp2 = get_exponent(f2);

    let amt1 = match iou_new(man1, exp1) {
        Ok(p) => p,
        Err(_) => return XFL_OVERFLOW,
    };
    let amt2 = match iou_new(man2, exp2) {
        Ok(p) => p,
        Err(_) => return XFL_OVERFLOW,
    };

    let sum = if amt2.mantissa == 0 {
        amt1
    } else if amt1.mantissa == 0 {
        amt2
    } else {
        let Some(nv) = number_add(iou_to_number(amt1), iou_to_number(amt2)) else {
            return XFL_OVERFLOW;
        };
        match iou_from_number(nv) {
            Ok(p) => p,
            Err(_) => return XFL_OVERFLOW,
        }
    };

    // `make_float(IOUAmount&)` on a zero-mantissa amount always yields
    // `EXPONENT_UNDERSIZED`, which `float_sum` remaps to `0` — collapsed
    // here into a direct check. HookAPI.cpp's own path corrupts an
    // intermediate value via an `Expected::error()` cast first; not worth
    // reproducing bug-for-bug since the net observable behavior is this
    // simple check.
    if sum.mantissa == 0 {
        return 0;
    }
    let mag = sum.mantissa.unsigned_abs();
    // `sum` is already IOU-range-clamped, so `make_float` here cannot
    // actually fail; `unwrap_or_default` (`0`) is an unreachable fallback,
    // not a real "sum came out zero" case (that's the check above).
    make_float(mag, sum.exponent, sum.mantissa < 0).unwrap_or_default()
}

/// `HookAPI::float_sto`. Currency/issuer are always exactly 20 bytes when
/// `Some` at this boundary (`rshooks::types::CurrencyCode`/`AccountId` are
/// fixed-width — the 3-byte short currency form the raw wasm ABI also
/// accepts is not reachable through the typed `rshooks` API this backend
/// serves).
#[allow(clippy::indexing_slicing)] // `POWERS_OF_TEN[shift]` is reached only after the `(0..=15).contains(&shift)` guard immediately above it
pub(crate) fn float_sto(
    currency: Option<&[u8]>,
    issuer: Option<&[u8]>,
    amount: i64,
    field_code: u32,
) -> Result<Vec<u8>, i64> {
    if !is_valid(amount) {
        return Err(INVALID_FLOAT);
    }

    let field = (field_code & 0xFFFF) as u16;
    let ty = (field_code >> 16) as u16;
    let is_xrp = field_code == 0;
    let is_short = field_code == 0xFFFF_FFFF;

    match (currency, issuer) {
        (Some(_), None) | (None, Some(_)) => return Err(INVALID_ARGUMENT),
        _ => {}
    }
    if issuer.is_some() {
        if is_xrp || is_short {
            return Err(INVALID_ARGUMENT);
        }
    } else if !is_xrp && !is_short {
        return Err(INVALID_ARGUMENT);
    }

    let mut out = Vec::with_capacity(48);
    write_field_header(&mut out, field, ty);

    let man = get_mantissa(amount);
    let exp = get_exponent(amount);
    // Deliberately the *raw* helper, not zero-aware — see the module doc
    // comment's quirk writeup.
    let neg = hook_float_is_negative(amount);

    let mut amt = [0u8; 8];
    if is_xrp {
        let shift = -exp;
        if !(0..=15).contains(&shift) {
            return Err(XFL_OVERFLOW); // "https://github.com/Xahau/xahaud/issues/586"
        }
        let drops = if shift > 0 {
            man / POWERS_OF_TEN[shift as usize]
        } else {
            man
        };
        amt[0] = if neg { 0 } else { 0b0100_0000 } | ((drops >> 56) & 0x3F) as u8;
        amt[1] = ((drops >> 48) & 0xFF) as u8;
        amt[2] = ((drops >> 40) & 0xFF) as u8;
        amt[3] = ((drops >> 32) & 0xFF) as u8;
        amt[4] = ((drops >> 24) & 0xFF) as u8;
        amt[5] = ((drops >> 16) & 0xFF) as u8;
        amt[6] = ((drops >> 8) & 0xFF) as u8;
        amt[7] = (drops & 0xFF) as u8;
    } else if man == 0 {
        amt[0] = 0b1000_0000; // canonical IOU zero: `0x8000000000000000`
    } else {
        let exp_biased = (exp + EXPONENT_BIAS) as u8; // 1..=177, always fits
        amt[0] = (if neg { 0b1000_0000 } else { 0b1100_0000 }) | (exp_biased >> 2);
        amt[1] = ((exp_biased & 0b11) << 6) | ((man >> 48) & 0x3F) as u8;
        amt[2] = ((man >> 40) & 0xFF) as u8;
        amt[3] = ((man >> 32) & 0xFF) as u8;
        amt[4] = ((man >> 24) & 0xFF) as u8;
        amt[5] = ((man >> 16) & 0xFF) as u8;
        amt[6] = ((man >> 8) & 0xFF) as u8;
        amt[7] = (man & 0xFF) as u8;
    }
    out.extend_from_slice(&amt);

    if !is_xrp && !is_short {
        // Validated above: reaching here without `is_xrp`/`is_short`
        // guarantees `issuer` (and therefore `currency`) is `Some`.
        if let (Some(c), Some(i)) = (currency, issuer) {
            out.extend_from_slice(c);
            out.extend_from_slice(i);
        }
    }
    Ok(out)
}

/// Writes the 0/1/2/3-byte STO field header for `(field, type)` — the exact
/// byte layout `HookAPI::float_sto` inlines; factored out because
/// [`crate::host::sto`] (a later stage) will need the identical logic.
fn write_field_header(out: &mut Vec<u8>, field: u16, ty: u16) {
    if field == 0 && ty == 0 {
        // native/XRP: no header
    } else if field == 0xFFFF && ty == 0xFFFF {
        // "short": no header
    } else if field < 16 && ty < 16 {
        out.push(((ty as u8) << 4) + field as u8);
    } else if field >= 16 && ty < 16 {
        out.push((ty as u8) << 4);
        out.push(field as u8);
    } else if field < 16 && ty >= 16 {
        out.push((field as u8) << 4);
        out.push(ty as u8);
    } else {
        out.push(0);
        out.push(ty as u8);
        out.push(field as u8);
    }
}

/// `HookAPI::float_sto_set`. For a native (`is_xrp`) amount, xahaud skips
/// byte 0 entirely (rather than folding its low 6 magnitude bits into the
/// mantissa) and reinterprets bytes `1..8` as a bare integer at exponent
/// `0`, then renormalizes; that reinterpretation also masks the new byte 0
/// (wire byte 1) with `0x3F` (see `mantissa_bytes[0] & 0x3F` below),
/// dropping its top 2 bits too. Total loss: 8 magnitude bits (byte 0's low
/// 6 plus byte 1's top 2), not just 6. Lossless in practice for any drops
/// [`float_sto`] itself produces (`drops <= mantissa <= MAX_MANTISSA <
/// 2^54` always — see `native_amount_round_trip_via_drops`), but a native
/// blob from elsewhere with mainnet-scale drops `>= 2^54` (e.g. a real
/// ledger `Amount`) silently loses those 8 bits — see
/// `native_amount_with_large_drops_loses_byte0_low_bits`. Ported
/// byte-for-byte regardless: that really is what the host returns.
#[allow(clippy::indexing_slicing)] // every index/slice below is reached only after an explicit `len()` guard immediately before it (`upto.len() > 8`/`< 11`/`< 10`/`< 8`) that establishes enough remaining bytes
pub(crate) fn float_sto_set(sto: &[u8]) -> i64 {
    let mut upto = sto;

    if upto.len() > 8 {
        let hi = upto[0] >> 4;
        let lo = upto[0] & 0xF;
        if hi == 0 && lo == 0 {
            if upto.len() < 11 {
                return rshooks_core::NOT_AN_OBJECT;
            }
            upto = &upto[3..];
        } else if hi == 0 || lo == 0 {
            if upto.len() < 10 {
                return rshooks_core::NOT_AN_OBJECT;
            }
            upto = &upto[2..];
        } else {
            upto = &upto[1..];
        }
    }

    if upto.len() < 8 {
        return rshooks_core::NOT_AN_OBJECT;
    }

    let is_xrp = (upto[0] & 0b1000_0000) == 0;
    let is_negative = (upto[0] & 0b0100_0000) == 0;

    let (exponent, mantissa_bytes): (i32, &[u8]) = if is_xrp {
        (0, &upto[1..8])
    } else {
        let exponent =
            (((i32::from(upto[0]) & 0x3F) << 2) + (i32::from(upto[1]) >> 6)) - EXPONENT_BIAS;
        (exponent, &upto[1..8])
    };

    let mut mantissa: u64 = (u64::from(mantissa_bytes[0]) & 0x3F) << 48;
    mantissa += u64::from(mantissa_bytes[1]) << 40;
    mantissa += u64::from(mantissa_bytes[2]) << 32;
    mantissa += u64::from(mantissa_bytes[3]) << 24;
    mantissa += u64::from(mantissa_bytes[4]) << 16;
    mantissa += u64::from(mantissa_bytes[5]) << 8;
    mantissa += u64::from(mantissa_bytes[6]);

    if mantissa == 0 {
        return 0;
    }
    match normalize_xfl(mantissa, exponent, is_negative) {
        Normalized::Value(bits) => bits,
        Normalized::Overflow => XFL_OVERFLOW,
    }
}

/// `HookAPI::slot_float` (P2-D — `Xahau/xahaud`, branch `dev`,
/// `src/xrpld/app/hook/detail/HookAPI.cpp:2315-2368`): converts a slot's
/// serialized `Amount` value bytes (8 native / 48 IOU, no header, matching
/// `crate::host::slots`' slot-content convention) to an XFL bit pattern.
///
/// Not [`float_sto_set`]'s decode: the native path here uses the *full*
/// 62-bit drops magnitude (`st_amt.xrp().drops()`), mantissa = drops,
/// exponent = -6, then [`normalize_xfl`] — `float_sto_set`'s
/// byte-0-skipping quirk is specific to that function. The IOU path decodes
/// the same wire mantissa/exponent layout but, unlike `float_sto_set`, does
/// not renormalize: a well-formed on-ledger IOU amount is already
/// canonical, mirroring the cited source's non-renormalizing
/// `st_amt.iou()` → `make_float(IOUAmount)` overload.
///
/// `bytes.len()` not 8 or 48 is defensive-only: `crate::host::slots` only
/// constructs `SlotKind::Amount` from `crate::slot_obj::classify_amount`
/// output (8 or 48 bytes) — unreachable via real slot navigation.
#[allow(clippy::indexing_slicing)] // every index below is reached only after the `bytes.len() == 8`/`== 48` match arm guard establishes it in-bounds — same pattern as `float_sto_set` above
pub(crate) fn slot_amount_to_xfl(bytes: &[u8]) -> i64 {
    match bytes.len() {
        8 => {
            let is_negative = bytes[0] & 0b0100_0000 == 0;
            let mut v = [0u8; 8];
            v.copy_from_slice(bytes);
            v[0] &= 0x3F; // clear the native/sign control bits, keep the full 62-bit magnitude
            let drops = u64::from_be_bytes(v);
            match normalize_xfl(drops, -6, is_negative) {
                Normalized::Value(bits) => bits,
                Normalized::Overflow => XFL_OVERFLOW,
            }
        }
        48 => {
            let is_negative = bytes[0] & 0b0100_0000 == 0;
            let exponent =
                (((i32::from(bytes[0]) & 0x3F) << 2) + (i32::from(bytes[1]) >> 6)) - EXPONENT_BIAS;
            let mut mantissa: u64 = (u64::from(bytes[1]) & 0x3F) << 48;
            mantissa += u64::from(bytes[2]) << 40;
            mantissa += u64::from(bytes[3]) << 32;
            mantissa += u64::from(bytes[4]) << 24;
            mantissa += u64::from(bytes[5]) << 16;
            mantissa += u64::from(bytes[6]) << 8;
            mantissa += u64::from(bytes[7]);
            if mantissa == 0 {
                return 0;
            }
            match make_float(mantissa, exponent, is_negative) {
                Ok(bits) => bits,
                // Defensive: a well-formed ledger IOU amount's wire
                // mantissa/exponent are always already in XFL range.
                Err(_) => rshooks_core::NOT_AN_AMOUNT,
            }
        }
        _ => rshooks_core::NOT_AN_AMOUNT,
    }
}

/// `HookAPI::float_invert`.
pub(crate) fn float_invert(f1: i64) -> i64 {
    if !is_valid(f1) {
        return INVALID_FLOAT;
    }
    if f1 == 0 {
        return DIVISION_BY_ZERO;
    }
    if f1 == FLOAT_ONE {
        return FLOAT_ONE;
    }
    float_divide_internal(FLOAT_ONE, f1)
}

/// `HookAPI::float_divide`.
pub(crate) fn float_divide(f1: i64, f2: i64) -> i64 {
    if !is_valid(f1) || !is_valid(f2) {
        return INVALID_FLOAT;
    }
    float_divide_internal(f1, f2)
}

/// `HookAPI::float_divide_internal` (long division by hand, matching the
/// `fixFloatDivide`-amendment-enabled branch — assumed active since it has
/// been active on Xahau mainnet well before this port; not independently
/// re-verifiable from the vendored headers alone). Operands are
/// pre-validated by [`float_divide`]/[`float_invert`]; the C++'s own
/// `normalize_xfl(man,exp)` calls on an already-normalized operand are
/// always a no-op and are elided here.
fn float_divide_internal(f1: i64, f2: i64) -> i64 {
    if f2 == 0 {
        return DIVISION_BY_ZERO;
    }
    if f1 == 0 {
        return 0;
    }
    if f2 == FLOAT_ONE {
        return f1;
    }

    let mut man1 = get_mantissa(f1);
    let exp1 = get_exponent(f1);
    let neg1 = hook_float_is_negative(f1);
    let mut man2 = get_mantissa(f2);
    let mut exp2 = get_exponent(f2);
    let neg2 = hook_float_is_negative(f2);

    while man2 > man1 {
        man2 /= 10;
        exp2 += 1;
    }
    if man2 == 0 {
        return DIVISION_BY_ZERO;
    }
    while man2 < man1 {
        if man2.saturating_mul(10) > man1 {
            break;
        }
        man2 *= 10;
        exp2 -= 1;
    }

    let mut man3: u64 = 0;
    let mut exp3 = exp1 - exp2;
    while man2 > 0 {
        let mut i: u64 = 0;
        while man1 >= man2 {
            man1 -= man2;
            i += 1;
        }
        man3 = man3 * 10 + i;
        man2 /= 10;
        if man2 == 0 {
            break;
        }
        exp3 -= 1;
    }

    let neg3 = neg1 != neg2;
    match normalize_xfl(man3, exp3, neg3) {
        Normalized::Value(bits) => bits,
        Normalized::Overflow => XFL_OVERFLOW,
    }
}

/// `HookAPI::float_one` — canonical bits for `1.0` (`mantissa =
/// 1_000_000_000_000_000`, `exponent = -15`). Cross-checked in this
/// module's tests against `XFL!(1)`'s independently-derived encoding
/// (`rshooks_macros::xfl_literal`'s `reference_vectors` test).
const FLOAT_ONE: i64 = 6_089_866_696_204_910_592;

pub(crate) fn float_one() -> i64 {
    FLOAT_ONE
}

/// `HookAPI::float_mantissa`.
pub(crate) fn float_mantissa(f1: i64) -> i64 {
    if !is_valid(f1) {
        return INVALID_FLOAT;
    }
    get_mantissa(f1) as i64
}

/// `HookAPI::float_sign`.
pub(crate) fn float_sign(f1: i64) -> i64 {
    if !is_valid(f1) {
        return INVALID_FLOAT;
    }
    if f1 == 0 {
        0
    } else {
        i64::from(hook_float_is_negative(f1))
    }
}

/// `HookAPI::float_int`.
#[allow(clippy::indexing_slicing)] // `POWERS_OF_TEN[shift]` is reached only after the `shift > 15`/`shift < 0` guards above leave `shift` in `0..=15`
pub(crate) fn float_int(f1: i64, decimal_places: u32, absolute: u32) -> i64 {
    if !is_valid(f1) {
        return INVALID_FLOAT;
    }
    if f1 == 0 {
        return 0;
    }
    let mut man1 = get_mantissa(f1);
    let exp1 = get_exponent(f1);
    let neg1 = hook_float_is_negative(f1);

    if decimal_places > 15 {
        return INVALID_ARGUMENT;
    }
    if neg1 && absolute == 0 {
        return CANT_RETURN_NEGATIVE;
    }

    // `decimal_places <= 15` (checked above) and `exp1` is in `-96..=80`, so
    // this never overflows `i32`.
    let shift = -(exp1 + decimal_places as i32);
    if shift > 15 {
        return 0;
    }
    if shift < 0 {
        return TOO_BIG;
    }
    if shift > 0 {
        man1 /= POWERS_OF_TEN[shift as usize];
    }
    man1 as i64
}

/// `HookAPI::float_log` — uses `f64::log10`, exactly as xahaud's own
/// `double`-based implementation does (design doc §4 explicitly allows
/// this: "float_log/float_root may use f64 exactly where xahaud does").
pub(crate) fn float_log(f1: i64) -> i64 {
    if !is_valid(f1) {
        return INVALID_FLOAT;
    }
    if f1 == 0 {
        return INVALID_ARGUMENT;
    }
    if hook_float_is_negative(f1) {
        return rshooks_core::COMPLEX_NOT_SUPPORTED;
    }
    let man1 = get_mantissa(f1);
    let exp1 = get_exponent(f1);
    let result = (man1 as f64).log10() + f64::from(exp1);
    match double_to_xfl(result) {
        Ok(bits) => bits,
        Err(e) => e,
    }
}

/// `HookAPI::float_root`.
pub(crate) fn float_root(f1: i64, n: u32) -> i64 {
    if !is_valid(f1) {
        return INVALID_FLOAT;
    }
    if f1 == 0 {
        return 0;
    }
    if n < 2 {
        return INVALID_ARGUMENT;
    }
    if hook_float_is_negative(f1) {
        return rshooks_core::COMPLEX_NOT_SUPPORTED;
    }
    let man1 = get_mantissa(f1);
    let exp1 = get_exponent(f1);
    let inp = (man1 as f64) * 10f64.powi(exp1);
    let result = inp.powf(1.0 / f64::from(n));
    match double_to_xfl(result) {
        Ok(bits) => bits,
        Err(e) => e,
    }
}

/// `HookAPI::double_to_xfl` — the one place other than `float_log`/
/// `float_root` themselves this is used.
fn double_to_xfl(x: f64) -> Result<i64, i64> {
    if x == 0.0 {
        return Ok(0);
    }
    let neg = x < 0.0;
    let mut absresult = if neg { -x } else { x };
    let mut exp_out = absresult.log10() as i32;
    absresult *= 10f64.powf(f64::from(-exp_out) + 15.0);
    let mut result = absresult as i64;
    if result < MIN_MANTISSA as i64 {
        if result == MIN_MANTISSA as i64 - 1 {
            result += 1;
        } else {
            result *= 10;
            exp_out -= 1;
        }
    }
    if result > MAX_MANTISSA as i64 {
        if result == MAX_MANTISSA as i64 + 1 {
            result -= 1;
        } else {
            result /= 10;
            exp_out += 1;
        }
    }
    exp_out -= 15;
    match make_float(result as u64, exp_out, neg) {
        Ok(bits) => Ok(bits),
        Err(rshooks_core::EXPONENT_UNDERSIZED) => Ok(0), // matches HookAPI.cpp's explicit remap
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt, docs/DESIGN.md §8

    use super::*;

    // -- cross-checks against `XFL!`'s independently-derived constants
    // (`rshooks_macros::xfl_literal::tests::reference_vectors`) --

    #[test]
    fn float_one_matches_xfl_macro_reference_vector() {
        assert_eq!(float_one(), 6_089_866_696_204_910_592);
        assert_eq!(
            make_float(MIN_MANTISSA, -15, false),
            Ok(6_089_866_696_204_910_592)
        );
    }

    #[test]
    fn float_set_matches_xfl_macro_reference_vectors() {
        // XFL!(1)
        assert_eq!(
            float_set(-15, 1_000_000_000_000_000),
            6_089_866_696_204_910_592
        );
        // XFL!(-1)
        assert_eq!(
            float_set(-15, -1_000_000_000_000_000),
            1_478_180_677_777_522_688
        );
        // XFL!(0.1)
        assert_eq!(
            float_set(-16, 1_000_000_000_000_000),
            6_071_852_297_695_428_608
        );
        // XFL!(2600000)
        assert_eq!(
            float_set(-9, 2_600_000_000_000_000),
            6_199_553_087_261_802_496
        );
        // XFL!(0.003333333333333333) -- 16 significant digits, exponent -18
        assert_eq!(
            float_set(-18, 3_333_333_333_333_333),
            6_038_156_834_009_797_973
        );
    }

    #[test]
    fn float_set_mantissa_zero_is_canonical_zero_not_an_error() {
        // Deviates from the `hook-api` skill's "mantissa=0 -> INVALID_FLOAT"
        // claim: `HookAPI::float_set` short-circuits `mantissa == 0` to `0`
        // before reaching the error-producing normalize path (confirmed
        // against `HookAPI.cpp` directly).
        assert_eq!(float_set(0, 0), 0);
        assert_eq!(float_set(80, 0), 0);
    }

    #[test]
    fn float_set_underflow_is_invalid_float() {
        // A representable mantissa whose exponent underflows past -96
        // reports INVALID_FLOAT (not canonical zero) -- `float_set`'s own
        // explicit remap of `normalize_xfl`'s `Value(0)` underflow.
        assert_eq!(float_set(-200, 1_000_000_000_000_000), INVALID_FLOAT);
    }

    #[test]
    fn float_set_overflow_is_invalid_float_not_xfl_overflow() {
        assert_eq!(float_set(200, 1_000_000_000_000_000), INVALID_FLOAT);
    }

    // -- is_valid / round-trip --

    #[test]
    fn canonical_zero_is_valid() {
        assert!(is_valid(0));
    }

    #[test]
    fn negative_i64_is_never_a_valid_float() {
        assert!(!is_valid(-1));
        assert!(!is_valid(i64::MIN));
    }

    #[test]
    fn mantissa_exponent_sign_round_trip() {
        let one = float_one();
        assert_eq!(get_mantissa(one), MIN_MANTISSA);
        assert_eq!(get_exponent(one), -15);
        assert!(!hook_float_is_negative(one));
        let neg_one = float_negate(one);
        assert_eq!(get_mantissa(neg_one), MIN_MANTISSA);
        assert_eq!(get_exponent(neg_one), -15);
        assert!(hook_float_is_negative(neg_one));
    }

    // -- arithmetic identities --

    #[test]
    fn x_times_one_equals_x() {
        let x = float_set(-9, 2_600_000_000_000_000); // XFL!(2600000)
        assert_eq!(float_multiply(x, float_one()), x);
    }

    #[test]
    fn x_plus_zero_equals_x_validated() {
        let x = float_set(-9, 2_600_000_000_000_000);
        assert_eq!(float_sum(x, 0), x);
        assert_eq!(float_sum(0, x), x);
    }

    #[test]
    fn x_minus_x_is_canonical_zero() {
        let x = float_set(-9, 2_600_000_000_000_000);
        let neg_x = float_negate(x);
        assert_eq!(float_sum(x, neg_x), 0);
    }

    // `float_set(-15, N * 1_000_000_000_000_000)` is the literal integer `N`
    // (mantissa padded to 16 significant digits, exponent -15 compensating
    // — the same normalized form `float_one` uses for `1.0`).
    #[test]
    fn known_product() {
        // 2 * 3 = 6
        let two = float_set(-15, 2_000_000_000_000_000);
        let three = float_set(-15, 3_000_000_000_000_000);
        let six = float_set(-15, 6_000_000_000_000_000);
        assert_eq!(float_multiply(two, three), six);
    }

    #[test]
    fn known_sum() {
        // 2 + 3 = 5
        let two = float_set(-15, 2_000_000_000_000_000);
        let three = float_set(-15, 3_000_000_000_000_000);
        let five = float_set(-15, 5_000_000_000_000_000);
        assert_eq!(float_sum(two, three), five);
    }

    #[test]
    fn known_quotient() {
        // 6 / 3 = 2
        let six = float_set(-15, 6_000_000_000_000_000);
        let three = float_set(-15, 3_000_000_000_000_000);
        let two = float_set(-15, 2_000_000_000_000_000);
        assert_eq!(float_divide(six, three), two);
    }

    #[test]
    fn divide_by_zero_and_invert_zero() {
        let one = float_one();
        assert_eq!(float_divide(one, 0), DIVISION_BY_ZERO);
        assert_eq!(float_invert(0), DIVISION_BY_ZERO);
    }

    #[test]
    fn invert_one_is_one() {
        assert_eq!(float_invert(float_one()), float_one());
    }

    #[test]
    fn invert_two_times_two_is_one() {
        let two = float_set(-15, 2_000_000_000_000_000);
        let inv = float_invert(two);
        assert!(is_valid(inv));
        assert_eq!(float_multiply(two, inv), float_one());
    }

    #[test]
    fn mulratio_denominator_zero_is_division_by_zero() {
        let one = float_one();
        assert_eq!(float_mulratio(one, 0, 1, 0), DIVISION_BY_ZERO);
    }

    #[test]
    fn mulratio_half_of_two_is_one() {
        let two = float_set(-15, 2_000_000_000_000_000);
        assert_eq!(float_mulratio(two, 0, 1, 2), float_one());
    }

    #[test]
    fn mulratio_rounding_up_vs_down_differ_on_a_true_fraction() {
        // 1 * (1/3): round down truncates, round up bumps the last digit.
        let one = float_one();
        let down = float_mulratio(one, 0, 1, 3);
        let up = float_mulratio(one, 1, 1, 3);
        assert!(is_valid(down));
        assert!(is_valid(up));
        assert_ne!(down, up);
        assert_eq!(get_mantissa(up), get_mantissa(down) + 1);
    }

    #[test]
    fn negate_canonical_zero_stays_zero() {
        assert_eq!(float_negate(0), 0);
    }

    #[test]
    fn negate_is_involutive() {
        let x = float_set(-9, 2_600_000_000_000_000);
        assert_eq!(float_negate(float_negate(x)), x);
    }

    // -- compare mode matrix --

    #[test]
    fn compare_mode_matrix() {
        let one = float_one();
        let two = float_set(0, 2_000_000_000_000_000);
        let one_again = float_set(-15, 1_000_000_000_000_000);

        assert_eq!(
            float_compare(one, one_again, rshooks_core::COMPARE_EQUAL),
            1
        );
        assert_eq!(float_compare(one, two, rshooks_core::COMPARE_EQUAL), 0);
        assert_eq!(float_compare(one, two, rshooks_core::COMPARE_LESS), 1);
        assert_eq!(float_compare(two, one, rshooks_core::COMPARE_LESS), 0);
        assert_eq!(float_compare(two, one, rshooks_core::COMPARE_GREATER), 1);
        assert_eq!(float_compare(one, two, rshooks_core::COMPARE_GREATER), 0);
        let le = rshooks_core::COMPARE_LESS | rshooks_core::COMPARE_EQUAL;
        assert_eq!(float_compare(one, one_again, le), 1);
        assert_eq!(float_compare(one, two, le), 1);
        assert_eq!(float_compare(two, one, le), 0);
        let ge = rshooks_core::COMPARE_GREATER | rshooks_core::COMPARE_EQUAL;
        assert_eq!(float_compare(one, one_again, ge), 1);
        assert_eq!(float_compare(two, one, ge), 1);
        assert_eq!(float_compare(one, two, ge), 0);
        let ne = rshooks_core::COMPARE_LESS | rshooks_core::COMPARE_GREATER;
        assert_eq!(float_compare(one, two, ne), 1);
        assert_eq!(float_compare(one, one_again, ne), 0);
    }

    #[test]
    fn compare_negative_numbers_order_correctly() {
        let one = float_one();
        let neg_one = float_negate(one);
        assert_eq!(float_compare(neg_one, one, rshooks_core::COMPARE_LESS), 1);
        assert_eq!(
            float_compare(one, neg_one, rshooks_core::COMPARE_GREATER),
            1
        );
    }

    #[test]
    fn compare_mode_zero_or_all_three_bits_is_invalid_argument() {
        let one = float_one();
        assert_eq!(float_compare(one, one, 0), INVALID_ARGUMENT);
        assert_eq!(float_compare(one, one, 0b111), INVALID_ARGUMENT);
        assert_eq!(float_compare(one, one, 0b1000), INVALID_ARGUMENT);
    }

    // -- float_int --

    #[test]
    fn float_int_basic() {
        let x = float_set(0, 4_200_000_000_000_000); // exponent 0, mantissa 4.2e15 -> value 4_200_000_000_000_000
        assert_eq!(float_int(x, 0, 0), 4_200_000_000_000_000);
    }

    #[test]
    fn float_int_negative_requires_absolute() {
        let x = float_negate(float_one());
        assert_eq!(float_int(x, 0, 0), CANT_RETURN_NEGATIVE);
        assert_eq!(float_int(x, 0, 1), 1);
    }

    #[test]
    fn float_int_decimal_places_out_of_range() {
        assert_eq!(float_int(float_one(), 16, 0), INVALID_ARGUMENT);
    }

    #[test]
    fn float_int_too_big() {
        // exponent 80 with decimal_places 0 -> shift = -80, far below 0 -> TOO_BIG
        let huge = float_set(80, 1_000_000_000_000_000);
        assert_eq!(float_int(huge, 0, 0), TOO_BIG);
    }

    #[test]
    fn float_int_underflow_is_zero() {
        // shift > 15 -> 0
        let tiny = float_set(-96, 1_000_000_000_000_000);
        assert_eq!(float_int(tiny, 0, 0), 0);
    }

    // -- float_log / float_root --

    #[test]
    fn float_log_of_one_is_zero() {
        assert_eq!(float_log(float_one()), 0);
    }

    #[test]
    fn float_log_zero_is_invalid_argument() {
        assert_eq!(float_log(0), INVALID_ARGUMENT);
    }

    #[test]
    fn float_log_negative_is_complex_not_supported() {
        let neg_one = float_negate(float_one());
        assert_eq!(float_log(neg_one), rshooks_core::COMPLEX_NOT_SUPPORTED);
    }

    #[test]
    fn float_root_square_root_of_four_is_two() {
        let four = float_set(-15, 4_000_000_000_000_000);
        let two = float_set(-15, 2_000_000_000_000_000);
        let root = float_root(four, 2);
        assert!(is_valid(root));
        // f64-based, so allow it to land within one ULP-scale of `two`'s
        // mantissa rather than demanding bit-exact equality.
        assert_eq!(get_exponent(root), get_exponent(two));
        assert!(get_mantissa(root).abs_diff(get_mantissa(two)) <= 1);
    }

    #[test]
    fn float_root_n_below_two_is_invalid_argument() {
        assert_eq!(float_root(float_one(), 1), INVALID_ARGUMENT);
        assert_eq!(float_root(float_one(), 0), INVALID_ARGUMENT);
    }

    #[test]
    fn float_root_zero_is_zero() {
        assert_eq!(float_root(0, 2), 0);
    }

    // -- validation propagation --

    #[test]
    fn invalid_operand_reports_invalid_float_across_the_family() {
        let bad = -1i64; // bit 63 set -> never valid
        let one = float_one();
        assert_eq!(float_multiply(bad, one), INVALID_FLOAT);
        assert_eq!(float_multiply(one, bad), INVALID_FLOAT);
        assert_eq!(float_sum(bad, one), INVALID_FLOAT);
        assert_eq!(float_divide(bad, one), INVALID_FLOAT);
        assert_eq!(
            float_compare(bad, one, rshooks_core::COMPARE_EQUAL),
            INVALID_FLOAT
        );
        assert_eq!(float_negate(bad), INVALID_FLOAT);
        assert_eq!(float_mantissa(bad), INVALID_FLOAT);
        assert_eq!(float_sign(bad), INVALID_FLOAT);
        assert_eq!(float_int(bad, 0, 0), INVALID_FLOAT);
        assert_eq!(float_log(bad), INVALID_FLOAT);
        assert_eq!(float_root(bad, 2), INVALID_FLOAT);
        assert_eq!(float_mulratio(bad, 0, 1, 2), INVALID_FLOAT);
    }

    // -- float_sto / float_sto_set --

    #[test]
    fn native_zero_encodes_as_all_zero_bytes_not_positive_zero() {
        // The documented quirk (module doc comment): canonical-zero XFL
        // encoded as a native amount produces 8 all-zero bytes, not the
        // conventional `0x40..0` "positive zero" pattern.
        let out = float_sto(None, None, 0, 0).unwrap();
        assert_eq!(out, [0u8; 8]);
    }

    #[test]
    fn native_amount_round_trip_via_drops() {
        // The XFL value 5_000_000 (mantissa 5e15, exponent -9) encoded as a
        // native amount stores the integer 5_000_000 as its 62-bit drops
        // field (shift = -exponent = 9).
        let value = float_set(-9, 5_000_000_000_000_000);
        let out = float_sto(None, None, value, 0).unwrap();
        assert_eq!(out.len(), 8);
        // top bit clear (native), second bit set (non-negative)
        assert_eq!(out[0] & 0b1100_0000, 0b0100_0000);
        let drops = u64::from_be_bytes(out.clone().try_into().unwrap()) & !(0b11u64 << 62);
        assert_eq!(drops, 5_000_000);

        // float_sto_set reinterprets the drops bytes as a bare integer at
        // exponent 0 before renormalizing; for realistic drops counts (well
        // under 2^54, so byte 0's skipped low 6 bits are always 0) that
        // still round-trips to the same canonical XFL, since XFL
        // normalization is a deterministic, unique function of the real
        // number represented.
        let decoded = float_sto_set(&out);
        assert_eq!(decoded, float_set(0, 5_000_000));
        assert_eq!(decoded, value);
    }

    #[test]
    fn native_amount_with_large_drops_loses_byte0_low_bits() {
        // A hand-constructed native amount blob (not one `float_sto` could
        // produce) whose drops value needs byte0's low 6 bits: `drops =
        // (0x3F << 56) | 1`, i.e. `0x3F00000000000001`, comfortably above
        // `2^54`.
        let drops: u64 = (0x3Fu64 << 56) | 1;
        let mut out = drops.to_be_bytes();
        out[0] |= 0b0100_0000; // native, non-negative
        let decoded = float_sto_set(&out);
        // The decoded value reflects only the low 54 bits actually read
        // (bytes 1..8 reinterpreted at exponent 0) -- byte 0's `0x3F`
        // contribution (`0x3F << 56`) is silently dropped.
        let expected = float_set(0, i64::try_from(drops & ((1u64 << 54) - 1)).unwrap());
        assert_eq!(decoded, expected);
        assert_ne!(decoded, float_set(0, i64::try_from(drops).unwrap()));
    }

    #[test]
    fn iou_amount_round_trips_through_sto_and_sto_set() {
        let currency = [0u8; 20];
        let issuer = [7u8; 20];
        let x = float_set(-9, 2_600_000_000_000_000); // XFL!(2600000)
        let out = float_sto(Some(&currency), Some(&issuer), x, 0x0006_0001).unwrap();
        assert_eq!(out.len(), 1 + 8 + 20 + 20);
        let decoded = float_sto_set(&out);
        assert_eq!(decoded, x);
    }

    #[test]
    fn iou_amount_short_form_round_trips_without_header() {
        // `field_code == 0xFFFFFFFF` ("short") writes just the bare 8-byte
        // amount, no header/currency/issuer tail -- xahaud rejects pairing
        // it with an issuer (`HookAPI::float_sto`'s `is_short` check), so
        // `currency`/`issuer` must be `None` here.
        let x = float_one();
        let out = float_sto(None, None, x, 0xFFFF_FFFF).unwrap();
        assert_eq!(out.len(), 8);
        let decoded = float_sto_set(&out);
        assert_eq!(decoded, x);
    }

    #[test]
    fn iou_zero_encodes_as_canonical_amount_zero() {
        let currency = [3u8; 20];
        let issuer = [4u8; 20];
        let out = float_sto(Some(&currency), Some(&issuer), 0, 0x0006_0001).unwrap();
        // header byte + 0x8000000000000000 + currency + issuer
        assert_eq!(out[1], 0b1000_0000);
        assert_eq!(&out[2..9], &[0u8; 7]);
    }

    #[test]
    fn sto_mixed_currency_issuer_options_is_invalid_argument() {
        let currency = [0u8; 20];
        assert_eq!(
            float_sto(Some(&currency), None, float_one(), 0x0006_0001),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn sto_field_code_header_lengths() {
        // field<16,type<16 -> 1 byte
        let mut out = Vec::new();
        write_field_header(&mut out, 1, 6);
        assert_eq!(out, [0x61]);
        // field>=16,type<16 -> 2 bytes
        out.clear();
        write_field_header(&mut out, 20, 6);
        assert_eq!(out, [0x60, 20]);
        // field<16,type>=16 -> 2 bytes
        out.clear();
        write_field_header(&mut out, 1, 20);
        assert_eq!(out, [0x10, 20]);
        // field>=16,type>=16 -> 3 bytes
        out.clear();
        write_field_header(&mut out, 20, 20);
        assert_eq!(out, [0, 20, 20]);
    }
}
