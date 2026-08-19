//! Xahau decimal floating-point values.
//!
//! [`XFL`] delegates arithmetic and comparisons to the Hook API. Its operators
//! return [`Result`] because invalid float values are reported by the host.
//!
//! Use [`crate::xfl_unchecked::XFLUnchecked`] only when a measured hot path
//! justifies deferring validation until the end of an arithmetic chain.

use crate::api;
use crate::error::{Result, res};
use crate::types::{AccountId, CurrencyCode};

/// Bias applied to the stored 8-bit exponent field (bits 54..=61): unbiased
/// exponent = stored field - 97.
const EXPONENT_BIAS: i64 = 97;
/// Bit offset of the exponent field.
const EXPONENT_SHIFT: u32 = 54;
/// Mask for the 8-bit exponent field once shifted into place. `u64`, not
/// `i64`, to match [`XFL`]'s internal storage — see its type doc comment.
const EXPONENT_MASK: u64 = 0xFF;

/// A Xahau XFL value: an opaque wrapper over the raw bit pattern the Hook
/// API's `float_*` functions operate on.
///
/// The inner field is deliberately private: XFL host calls return negative
/// values as error codes sharing the same `i64` channel as valid floats, so
/// a public field would let a caller smuggle a raw error code in as if it
/// were a value. [`XFL::from_raw_bits`] / [`XFL::raw_bits`] are the explicit,
/// documented escape hatches for unchecked representation access — both
/// still speak `i64` at the public boundary, matching the Hook API's FFI
/// convention (every `float_*` extern function takes/returns `i64`) and the
/// existing persisted-state encoding (`convert.rs`'s `ToBytes`/`FromBytes`
/// impls for `XFL` round-trip through `i64`'s own).
///
/// **Internally stored as `u64`, not `i64`**, unlike the FFI boundary and
/// unlike [`crate::xfl_unchecked::XFLUnchecked`] (which deliberately keeps
/// `i64`, since it exists specifically to hold values that *might* be
/// negative error codes — see its module doc comment). Every `XFL` obtained
/// through the validated API (i.e. everything except [`XFL::from_raw_bits`])
/// is guaranteed by the host to have bit 63 clear (see the module doc
/// comment's bit layout) — always non-negative when read back as `i64` — so
/// `u64` mirrors that invariant and matches how Rust conventionally
/// represents an opaque bit pattern rather than a signed quantity (compare
/// `f64::to_bits() -> u64`, not `-> i64`): the `i64` FFI type is an
/// implementation detail of the Hook API's C ABI (which multiplexes error
/// codes onto XFL's own return channel), not a property of what an XFL
/// bit pattern actually *is*. This is a documentation/domain-modeling
/// choice, not an enforced safety property: [`XFL::from_raw_bits`] still
/// accepts an arbitrary `i64` (including a negative one), bit-cast as-is
/// into the `u64` field — see its own doc comment. `impl From<XFL> for
/// u64` exposes that native `u64` shape directly (`u64::from(xfl)`/
/// `xfl.into()`) alongside `raw_bits`'s `i64` shape — pick whichever the
/// call site actually needs; there is no corresponding `From<u64> for
/// XFL` in the other direction, only [`XFL::from_raw_bits`] (`i64`), to
/// keep exactly one documented construction path. Both directions
/// (`XFL` → `u64` here, [`crate::xfl_unchecked::XFLUnchecked`] → `i64`)
/// are bare bit-pattern reinterpretations, not validity checks — exactly
/// as honest as [`XFL::raw_bits`]/[`XFL::from_raw_bits`] already are, and
/// no more claim-laden: reading the bits back out never asserted they
/// were valid to begin with, so there is nothing here for a fallible
/// conversion to usefully guard.
///
/// `PartialEq`/`PartialOrd` are implemented, backed by the fallible
/// `float_compare` host call, with a `false`/`None` fallback on failure
/// (see the module doc comment's "Comparison: both methods and operators,
/// both via `float_compare`" section for why, and for when to use
/// [`XFL::eq`]/[`XFL::lt`]/[`XFL::gt`]/[`XFL::compare`] — all of which
/// return `Result<bool>` — instead of `==`/`<`/`>`).
///
/// # Examples
///
/// ```
/// use rshooks::xfl::XFL;
///
/// let one = XFL::one();
/// assert_eq!(one.raw_bits(), XFL::from_raw_bits(one.raw_bits()).raw_bits());
/// ```
#[derive(Clone, Copy, Debug)]
pub struct XFL(u64);

impl XFL {
    /// Wrap a raw XFL bit pattern with no validation. Escape hatch for
    /// interop with values obtained outside the typed API (e.g. persisted
    /// state). `bits` is bit-cast as-is into `XFL`'s internal `u64` storage
    /// (see the type doc comment) — a negative `bits` (e.g. a smuggled-in
    /// error code) becomes a large `u64`, not an error, since this
    /// constructor performs no validation either way.
    ///
    /// `const fn` so [`crate::XFL!`](crate) — which expands to
    /// `XFL::from_raw_bits(<bits>i64)` — can populate a `const`/`static`
    /// item, e.g. `const RATE: XFL = XFL!(0.003333333333333333);`.
    #[inline(always)]
    #[must_use]
    pub const fn from_raw_bits(bits: i64) -> XFL {
        XFL(bits as u64)
    }

    /// The raw XFL bit pattern. Escape hatch for interop; does not validate
    /// that `self` is actually a valid (non-error-code) XFL. Bit-cast back
    /// to `i64` from the internal `u64` storage (see the type doc comment)
    /// — lossless and exactly reverses [`XFL::from_raw_bits`] for every
    /// input, including a negative one.
    ///
    /// `const fn` for the same reason as [`XFL::from_raw_bits`] — lets a
    /// `const`/`static` XFL's raw bits be read back out in another const
    /// context.
    #[inline(always)]
    #[must_use]
    pub const fn raw_bits(self) -> i64 {
        self.0 as i64
    }

    /// Reinterpret `self` as an [`crate::xfl_unchecked::XFLUnchecked`] for a
    /// hot-path arithmetic chain. Zero-cost: just moves the raw bit pattern
    /// into the other newtype (as `i64` — see both types' doc comments for
    /// why `XFLUnchecked` keeps `i64` while `XFL` stores `u64`), no host
    /// call and no validation either way.
    #[inline(always)]
    #[must_use]
    pub fn unchecked(self) -> crate::xfl_unchecked::XFLUnchecked {
        crate::xfl_unchecked::XFLUnchecked::from_raw_bits(self.raw_bits())
    }

    /// Construct a normalized XFL from `exponent` and `mantissa`.
    #[inline(always)]
    pub fn new(exponent: i32, mantissa: i64) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_set(exponent, mantissa)) {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_set(exponent, mantissa) }).map(XFL::from_raw_bits)
    }

    /// The XFL representation of `1.0`. Cannot practically fail, so this is
    /// a bare `XFL`, not a `Result`.
    #[inline(always)]
    #[must_use]
    pub fn one() -> XFL {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_one()) {
            return XFL::from_raw_bits(v);
        }
        XFL::from_raw_bits(unsafe { rshooks_core::float_one() })
    }

    /// `1 / self`.
    #[inline(always)]
    pub fn invert(self) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_invert(self.raw_bits())) {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_invert(self.raw_bits()) }).map(XFL::from_raw_bits)
    }

    /// `self * (num / den)`, rounding up when `round_up` is set, down
    /// otherwise.
    #[inline(always)]
    pub fn mulratio(self, round_up: bool, num: u32, den: u32) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| {
            b.float_mulratio(self.raw_bits(), round_up as u32, num, den)
        }) {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_mulratio(self.raw_bits(), round_up as u32, num, den) })
            .map(XFL::from_raw_bits)
    }

    /// The mantissa component of `self` (`0` to `9_999_999_999_999_999`).
    #[inline(always)]
    pub fn mantissa(self) -> Result<i64> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_mantissa(self.raw_bits()))
        {
            return res(v);
        }
        res(unsafe { rshooks_core::float_mantissa(self.raw_bits()) })
    }

    /// The unbiased exponent component of `self` (`-96` to `+80`).
    ///
    /// Decoded locally from the raw bit pattern (bits 54..=61, bias 97) —
    /// there is no `float_exponent` host call. See the module doc comment
    /// for the bit layout. Wrapped in `Ok` for signature uniformity with
    /// [`XFL::mantissa`]; this computation cannot practically fail.
    #[inline(always)]
    pub fn exponent(self) -> Result<i64> {
        let field = (self.0 >> EXPONENT_SHIFT) & EXPONENT_MASK;
        // `field` is masked to 0..=0xFF, well within i64's range, so this
        // cast is lossless; `EXPONENT_BIAS` is the fixed constant 97, so
        // the `wrapping_sub` below never actually wraps — used anyway
        // (rather than a plain `-`) to sidestep `clippy::
        // arithmetic_side_effects` without needing a blanket `#[allow]`.
        // The subtraction has to happen in a *signed* type: a stored field
        // below 97 needs to decode to a negative unbiased exponent (e.g.
        // field `1` -> `-96`), which a `u64` subtraction could never
        // produce (it would wrap to a huge positive value instead).
        let field = field as i64;
        Ok(field.wrapping_sub(EXPONENT_BIAS))
    }

    /// Whether `self` is negative (per `float_sign`: `0` = positive or
    /// zero, `1` = negative — so `true` here means "negative").
    #[inline(always)]
    pub fn sign(self) -> Result<bool> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_sign(self.raw_bits())) {
            return res(v).map(|v| v != 0);
        }
        res(unsafe { rshooks_core::float_sign(self.raw_bits()) }).map(|v| v != 0)
    }

    /// Convert `self` to an integer, keeping `decimal_places` fractional
    /// digits; `absolute` requests the magnitude (dropping the sign)
    /// instead of erroring on a negative result.
    #[inline(always)]
    pub fn to_int(self, decimal_places: u32, absolute: bool) -> Result<i64> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| {
            b.float_int(self.raw_bits(), decimal_places, absolute as u32)
        }) {
            return res(v);
        }
        res(unsafe { rshooks_core::float_int(self.raw_bits(), decimal_places, absolute as u32) })
    }

    /// Compare `self` to `rhs` under the bitmask `mode` (see
    /// `rshooks_core::{COMPARE_EQUAL, COMPARE_LESS, COMPARE_GREATER}`, freely
    /// combinable, e.g. `COMPARE_LESS | COMPARE_EQUAL` for `<=`), via the
    /// `float_compare` host call.
    #[inline(always)]
    pub fn compare(self, rhs: XFL, mode: u32) -> Result<bool> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| {
            b.float_compare(self.raw_bits(), rhs.raw_bits(), mode)
        }) {
            return res(v).map(|v| v != 0);
        }
        res(unsafe { rshooks_core::float_compare(self.raw_bits(), rhs.raw_bits(), mode) })
            .map(|v| v != 0)
    }

    /// `self == rhs`.
    #[inline(always)]
    pub fn eq(self, rhs: XFL) -> Result<bool> {
        self.compare(rhs, rshooks_core::COMPARE_EQUAL)
    }

    /// `self < rhs`.
    #[inline(always)]
    pub fn lt(self, rhs: XFL) -> Result<bool> {
        self.compare(rhs, rshooks_core::COMPARE_LESS)
    }

    /// `self > rhs`.
    #[inline(always)]
    pub fn gt(self, rhs: XFL) -> Result<bool> {
        self.compare(rhs, rshooks_core::COMPARE_GREATER)
    }

    /// `log10(self)`.
    #[inline(always)]
    pub fn log(self) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_log(self.raw_bits())) {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_log(self.raw_bits()) }).map(XFL::from_raw_bits)
    }

    /// `self ^ (1/n)`.
    #[inline(always)]
    pub fn root(self, n: u32) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_root(self.raw_bits(), n)) {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_root(self.raw_bits(), n) }).map(XFL::from_raw_bits)
    }

    /// Encode `self` as a serialized Amount into `out`. Thin forwarding call
    /// to [`api::float::float_sto`] — reuses its pointer-direction and
    /// `Option` handling rather than duplicating it here.
    #[inline(always)]
    pub fn sto(
        self,
        out: &mut [u8],
        currency: Option<&CurrencyCode>,
        issuer: Option<&AccountId>,
        field_code: impl Into<u32>,
    ) -> Result<usize> {
        api::float::float_sto(out, currency, issuer, self, field_code)
    }

    /// Decode a serialized Amount (`buf`) into an XFL. Forwards to
    /// [`api::float::float_sto_set`].
    #[inline(always)]
    pub fn sto_set(buf: &[u8]) -> Result<XFL> {
        api::float::float_sto_set(buf)
    }

    /// Read the amount held in slot `slot_no` as an XFL. Forwards to
    /// [`api::float::slot_float`].
    #[inline(always)]
    pub fn from_slot(slot_no: u32) -> Result<XFL> {
        api::float::slot_float(slot_no)
    }
}

impl From<XFL> for u64 {
    /// The internal `u64` bit pattern (see the type doc comment) — the same
    /// value [`XFL::raw_bits`] returns, just as `u64` (`XFL`'s own native
    /// storage shape) rather than `i64` (the FFI-boundary shape
    /// `raw_bits`/`from_raw_bits` deliberately keep, for interop with the
    /// Hook API and persisted-state encodings that speak `i64`). Lets a
    /// caller who wants the `u64` write `u64::from(xfl)`/`xfl.into()`
    /// instead of `xfl.raw_bits() as u64`.
    #[inline(always)]
    fn from(value: XFL) -> u64 {
        value.0
    }
}

impl core::ops::Neg for XFL {
    type Output = Result<XFL>;

    /// `-self`, via the `float_negate` host call — **not** a local
    /// sign-bit flip. `Output` is `Result<XFL, HookError>` (not a bare
    /// `XFL`) to make room for that call failing, e.g. on an already-invalid
    /// `self`.
    #[inline(always)]
    fn neg(self) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_negate(self.raw_bits())) {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_negate(self.raw_bits()) }).map(XFL::from_raw_bits)
    }
}

impl core::ops::Add for XFL {
    type Output = Result<XFL>;

    /// `self + rhs`, via the `float_sum` host call.
    #[inline(always)]
    fn add(self, rhs: XFL) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) =
            rshooks_core::backend::with_backend(|b| b.float_sum(self.raw_bits(), rhs.raw_bits()))
        {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_sum(self.raw_bits(), rhs.raw_bits()) })
            .map(XFL::from_raw_bits)
    }
}

impl core::ops::Sub for XFL {
    type Output = Result<XFL>;

    /// `self - rhs`, implemented as `self + (-rhs)?`: one `float_negate`
    /// host call plus one `float_sum` host call. There is no dedicated
    /// `float_subtract` host function. The `?` on `-rhs` propagates a
    /// negation failure (e.g. `rhs` already invalid) as this call's own
    /// error, rather than feeding a poisoned value into `float_sum`.
    #[inline(always)]
    // `self + (-rhs)?` dispatches to this module's own `Add`/`Neg` impls
    // above (both fallible host round trips), not raw integer arithmetic —
    // `clippy::arithmetic_side_effects` can't see past the operator syntax
    // to tell the difference, so it flags this unconditionally; there is no
    // overflow/panic risk here to warn about.
    #[allow(clippy::arithmetic_side_effects)]
    fn sub(self, rhs: XFL) -> Result<XFL> {
        self + (-rhs)?
    }
}

impl core::ops::Mul for XFL {
    type Output = Result<XFL>;

    /// `self * rhs`, via the `float_multiply` host call.
    #[inline(always)]
    fn mul(self, rhs: XFL) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| {
            b.float_multiply(self.raw_bits(), rhs.raw_bits())
        }) {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_multiply(self.raw_bits(), rhs.raw_bits()) })
            .map(XFL::from_raw_bits)
    }
}

impl core::ops::Div for XFL {
    type Output = Result<XFL>;

    /// `self / rhs`, via the `float_divide` host call.
    #[inline(always)]
    fn div(self, rhs: XFL) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) =
            rshooks_core::backend::with_backend(|b| b.float_divide(self.raw_bits(), rhs.raw_bits()))
        {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_divide(self.raw_bits(), rhs.raw_bits()) })
            .map(XFL::from_raw_bits)
    }
}

impl PartialEq for XFL {
    /// `self == other`, forwarding to [`XFL::eq`] (`float_compare` under
    /// `COMPARE_EQUAL`). Falls back to `false` on a `float_compare`
    /// failure — see the module doc comment's "Comparison: both methods
    /// and operators, both via `float_compare`" section for why, and for
    /// when to call [`XFL::eq`] directly instead.
    #[inline(always)]
    fn eq(&self, other: &XFL) -> bool {
        XFL::eq(*self, *other).unwrap_or(false)
    }
}

impl PartialOrd for XFL {
    /// `self.partial_cmp(other)`, via up to two `float_compare` host calls
    /// (`COMPARE_LESS`, then — only if that came back `false` — `COMPARE_
    /// GREATER`; a `false` result for both means `Ordering::Equal`).
    /// Falls back to `None` on a `float_compare` failure at either step —
    /// see the module doc comment's "Comparison: both methods and
    /// operators, both via `float_compare`" section for why, and for when
    /// to call [`XFL::lt`]/[`XFL::gt`]/[`XFL::compare`] directly instead.
    #[inline(always)]
    fn partial_cmp(&self, other: &XFL) -> Option<core::cmp::Ordering> {
        match XFL::lt(*self, *other) {
            Ok(true) => Some(core::cmp::Ordering::Less),
            Ok(false) => match XFL::gt(*self, *other) {
                Ok(true) => Some(core::cmp::Ordering::Greater),
                Ok(false) => Some(core::cmp::Ordering::Equal),
                Err(_) => None,
            },
            Err(_) => None,
        }
    }
}

// Generates `impl $Trait<XFL> for Result<XFL, HookError>` and
// `impl $Trait<Result<XFL, HookError>> for XFL` for each listed
// `$Trait::$method`, so a chain of `+`/`-`/`*`/`/` that alternates a plain
// `XFL` in on one side at each step (`((a + b) + c) + d`, ...) short-
// circuits on the first error without an explicit `?` between steps.
//
// Rust's orphan rules do not allow an operator implementation with `Result`
// on both sides. Combine independently fallible values with `?` first.
macro_rules! xfl_result_chain_ops {
    ($( $Trait:ident :: $method:ident ),+ $(,)?) => {
        $(
            impl core::ops::$Trait<XFL> for Result<XFL> {
                type Output = Result<XFL>;

                #[inline(always)]
                fn $method(self, rhs: XFL) -> Result<XFL> {
                    core::ops::$Trait::$method(self?, rhs)
                }
            }

            impl core::ops::$Trait<Result<XFL>> for XFL {
                type Output = Result<XFL>;

                #[inline(always)]
                fn $method(self, rhs: Result<XFL>) -> Result<XFL> {
                    core::ops::$Trait::$method(self, rhs?)
                }
            }
        )+
    };
}

xfl_result_chain_ops!(Add::add, Sub::sub, Mul::mul, Div::div);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn raw_bits_round_trip() {
        for bits in [0i64, 1, -1, i64::MAX, i64::MIN, 42, -42] {
            assert_eq!(XFL::from_raw_bits(bits).raw_bits(), bits);
        }
    }

    #[test]
    fn into_u64_matches_raw_bits_bit_pattern() {
        // `u64::from(xfl)`/`xfl.into()` and `xfl.raw_bits() as u64` must
        // agree exactly -- same underlying bits, just `u64` (the native
        // storage shape) instead of `i64` (the FFI-boundary shape).
        for bits in [0i64, 1, -1, i64::MAX, i64::MIN, 42, -42] {
            let xfl = XFL::from_raw_bits(bits);
            let via_into: u64 = xfl.into();
            let via_from = u64::from(xfl);
            assert_eq!(via_into, bits as u64);
            assert_eq!(via_from, bits as u64);
        }
    }

    #[test]
    fn smoke_not_implemented_on_host() {
        // `matches!`, not `assert_eq!`, for every `Result<XFL, _>` here —
        // every assertion below only ever needs to distinguish `Err(...)`
        // from `Ok(...)` (never compares two `Ok(XFL)`s against each
        // other), so plain `assert_eq!` against an `Err(...)` pattern
        // would actually work today (`Result`'s derived `PartialEq`
        // short-circuits on the `Ok`/`Err` discriminant before ever
        // calling `XFL::eq`) — but `matches!` is used consistently
        // throughout this file regardless, so no assertion here
        // accidentally depends on `XFL`'s `PartialEq` impl (which forwards
        // to the fallible `float_compare`-backed `XFL::eq` and falls back
        // to `false` on failure — see the module doc comment — a real
        // trap for an `assert_eq!` that *does* compare two `Ok(XFL)`s).
        let one = XFL::one();
        assert!(matches!(XFL::new(0, 1), Err(HookError::NotImplemented)));
        assert!(matches!(one + one, Err(HookError::NotImplemented)));
        assert!(matches!(one - one, Err(HookError::NotImplemented)));
        assert!(matches!(one * one, Err(HookError::NotImplemented)));
        assert!(matches!(one / one, Err(HookError::NotImplemented)));
        assert!(matches!(-one, Err(HookError::NotImplemented)));
        assert!(matches!(one.invert(), Err(HookError::NotImplemented)));
        assert!(matches!(
            one.mulratio(false, 1, 2),
            Err(HookError::NotImplemented)
        ));
        assert_eq!(one.mantissa(), Err(HookError::NotImplemented));
        assert_eq!(one.sign(), Err(HookError::NotImplemented));
        assert_eq!(one.to_int(0, false), Err(HookError::NotImplemented));
        assert_eq!(one.compare(one, 1), Err(HookError::NotImplemented));
        assert_eq!(one.eq(one), Err(HookError::NotImplemented));
        assert_eq!(one.lt(one), Err(HookError::NotImplemented));
        assert_eq!(one.gt(one), Err(HookError::NotImplemented));
        assert!(matches!(one.log(), Err(HookError::NotImplemented)));
        assert!(matches!(one.root(2), Err(HookError::NotImplemented)));
        assert!(matches!(
            XFL::sto_set(&[0u8; 8]),
            Err(HookError::NotImplemented)
        ));
        assert!(matches!(XFL::from_slot(1), Err(HookError::NotImplemented)));
    }

    #[test]
    fn result_chain_short_circuits_on_first_error() {
        // `Result<XFL> op XFL` and `XFL op Result<XFL>` both type-check
        // and, given an `Err` input, never reach the host stub (host builds
        // have no way to observe that directly, but a mismatched-code
        // assertion here would fail if the wrong error propagated).
        // `matches!`, not `assert_eq!`, for the same reason as
        // `smoke_not_implemented_on_host` above.
        let one = XFL::one();
        let err: Result<XFL> = Err(HookError::DoesntExist);
        assert!(matches!(err + one, Err(HookError::DoesntExist)));
        assert!(matches!(one + err, Err(HookError::DoesntExist)));
        assert!(matches!(err - one, Err(HookError::DoesntExist)));
        assert!(matches!(one - err, Err(HookError::DoesntExist)));
        assert!(matches!(err * one, Err(HookError::DoesntExist)));
        assert!(matches!(one * err, Err(HookError::DoesntExist)));
        assert!(matches!(err / one, Err(HookError::DoesntExist)));
        assert!(matches!(one / err, Err(HookError::DoesntExist)));
    }

    #[test]
    // `one == one`/`one < one`/`one > one` below are all `false` given the
    // host stub (checked via a bound variable + `assert!`, not
    // `assert_eq!(..., false)` — `clippy::bool_assert_comparison` wants
    // `assert!(!(...))`  for that, but `clippy::neg_cmp_op_on_partial_ord`
    // simultaneously objects to negating `<`/`>` directly, since the
    // operands could genuinely be incomparable; binding first sidesteps
    // both).
    fn comparison_operators_fall_back_like_f64_nan_on_host() {
        // `float_compare`'s host stub is deterministic `NOT_IMPLEMENTED`
        // (an `Err`) regardless of operands, so every `PartialEq`/
        // `PartialOrd` call here exercises the `false`/`None` fallback —
        // and, crucially, returns promptly rather than hanging: rolling
        // the hook back on a `float_compare` failure from inside these
        // trait impls (instead of falling back to `false`/`None`) would
        // loop forever right here, since `rollback` never returns on a
        // host target.
        let one = XFL::one();
        let is_eq = one == one;
        let is_lt = one < one;
        let is_gt = one > one;
        assert!(!is_eq);
        assert_eq!(one.partial_cmp(&one), None);
        assert!(!is_lt);
        assert!(!is_gt);
        // The `Result<bool>`-returning inherent methods these forward to
        // still report the real failure explicitly.
        assert_eq!(one.eq(one), Err(HookError::NotImplemented));
    }

    #[test]
    fn exponent_decodes_bias_97_field() {
        // Stored field 97 (bits 54..=61) decodes to unbiased exponent 0.
        let bits = 97i64 << EXPONENT_SHIFT;
        assert_eq!(XFL::from_raw_bits(bits).exponent(), Ok(0));
        // Stored field 1 (minimum) decodes to -96.
        let bits_min = 1i64 << EXPONENT_SHIFT;
        assert_eq!(XFL::from_raw_bits(bits_min).exponent(), Ok(-96));
        // Stored field 177 (maximum) decodes to +80.
        let bits_max = 177i64 << EXPONENT_SHIFT;
        assert_eq!(XFL::from_raw_bits(bits_max).exponent(), Ok(80));
    }
}
