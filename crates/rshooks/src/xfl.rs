//! Xahau decimal floating-point values.
//!
//! [`XFL`] delegates arithmetic and comparisons to the Hook API. Its operators
//! return [`Result`] because invalid float values are reported by the host.
//!
//! Use [`crate::xfl_unchecked::XFLUnchecked`] only when a measured hot path
//! justifies deferring validation until the end of an arithmetic chain.

use crate::api;
use crate::error::{HookError, Result, res};
use crate::types::{AccountId, CurrencyCode, IouAmount};

/// Bias applied to the stored 8-bit exponent field (bits 54..=61): unbiased
/// exponent = stored field - 97.
const EXPONENT_BIAS: i64 = 97;
/// Bit offset of the exponent field.
const EXPONENT_SHIFT: u32 = 54;
/// Mask for the 8-bit exponent field once shifted into place. `u64`, not
/// `i64`, to match [`XFL`]'s internal storage — see its type doc comment.
const EXPONENT_MASK: u64 = 0xFF;
/// Mask for the 54-bit mantissa field (bits 0..=53).
const MANTISSA_MASK: u64 = (1u64 << EXPONENT_SHIFT) - 1;
/// Lower bound of a canonical nonzero mantissa (`hook_float::minMantissa`).
const MIN_MANTISSA: u64 = 1_000_000_000_000_000;
/// Upper bound of a canonical nonzero mantissa (`hook_float::maxMantissa`).
const MAX_MANTISSA: u64 = 9_999_999_999_999_999;
/// Lower bound of a canonical unbiased exponent (stored field `1`).
const MIN_EXPONENT: i64 = -96;
/// Upper bound of a canonical unbiased exponent (stored field `177`).
const MAX_EXPONENT: i64 = 80;
/// Bit offset of the sign bit within a canonical nonzero encoding: set
/// means positive, clear means negative (`hook_float::is_negative`'s
/// convention — see [`is_canonical`]'s doc comment; not consulted for
/// canonical zero, which short-circuits before this bit is read).
const SIGN_SHIFT: u32 = 62;

/// Whether raw bits `bits` are a canonical XFL encoding — the Hook API's own
/// `RETURN_IF_INVALID_FLOAT` gate (`applyHook.cpp`), reproduced as a local
/// bit test since the canonical ranges are fixed protocol constants, not
/// host state. `bits < 0` is never valid (that channel is reserved for host
/// error codes, e.g. [`crate::error::HookError::NotImplemented`]'s `-14`);
/// `bits == 0` is canonical zero; otherwise the mantissa and exponent fields
/// must both fall within their canonical ranges (the sign bit is
/// unconstrained either way).
#[inline(always)]
fn is_canonical(bits: i64) -> bool {
    if bits < 0 {
        return false;
    }
    if bits == 0 {
        return true;
    }
    let bits = bits as u64;
    let mantissa = bits & MANTISSA_MASK;
    let exponent_field = (bits >> EXPONENT_SHIFT) & EXPONENT_MASK;
    // See `XFL::exponent`'s own comment: `wrapping_sub` sidesteps
    // `clippy::arithmetic_side_effects` without an `#[allow]`; `EXPONENT_BIAS`
    // is the fixed constant 97, so this never actually wraps.
    let exponent = (exponent_field as i64).wrapping_sub(EXPONENT_BIAS);
    (MIN_MANTISSA..=MAX_MANTISSA).contains(&mantissa)
        && (MIN_EXPONENT..=MAX_EXPONENT).contains(&exponent)
}

/// A Xahau XFL value: an opaque wrapper over the raw bit pattern the Hook
/// API's `float_*` functions operate on.
///
/// The inner field is private: XFL host calls return negative values as
/// error codes on the same `i64` channel as valid floats, so a public field
/// would let a caller smuggle a raw error code in as if it were a value.
/// [`XFL::from_raw_bits`] / [`XFL::raw_bits`] are the explicit escape
/// hatches for unchecked representation access, both speaking `i64` to
/// match the Hook API's FFI convention and the persisted-state encoding
/// (`convert.rs`'s `ToBytes`/`FromBytes` impls for `XFL`).
///
/// **Internally stored as `u64`, not `i64`** — unlike the FFI boundary and
/// unlike [`crate::xfl_unchecked::XFLUnchecked`], which keeps `i64` since it
/// exists to hold values that might be negative error codes. Every `XFL`
/// obtained through the validated API (i.e. everything except
/// [`XFL::from_raw_bits`]) has bit 63 clear, so `u64` mirrors that
/// invariant (compare `f64::to_bits() -> u64`). [`XFL::from_raw_bits`]
/// still accepts and bit-casts an arbitrary `i64`, including a negative
/// one, with no validation. `impl From<XFL> for u64` exposes this native
/// `u64` shape directly (`u64::from(xfl)`/`xfl.into()`); there is no
/// `From<u64> for XFL` in the other direction, only `from_raw_bits`, to
/// keep a single documented construction path.
///
/// `PartialEq`/`PartialOrd` are implemented via the fallible
/// `float_compare` host call, falling back to `false`/`None` on failure —
/// use [`XFL::eq`]/[`XFL::lt`]/[`XFL::gt`]/[`XFL::compare`] directly for the
/// real `Result<bool>`.
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
    /// state). `bits` is bit-cast as-is into the internal `u64` storage — a
    /// negative `bits` (e.g. a smuggled-in error code) becomes a large
    /// `u64`, not an error.
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
    /// to `i64` from the internal `u64` storage — lossless, exactly
    /// reversing [`XFL::from_raw_bits`] for every input.
    ///
    /// `const fn` for the same reason as [`XFL::from_raw_bits`].
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
        // Lossless cast (field is masked to 0..=0xFF). The subtraction must
        // happen in a signed type -- a field below 97 decodes to a negative
        // exponent (e.g. field `1` -> `-96`), which `u64` subtraction would
        // wrap instead of producing. `wrapping_sub` (not `-`) sidesteps
        // `clippy::arithmetic_side_effects`; `EXPONENT_BIAS` is the fixed
        // constant 97, so it never actually wraps.
        let field = field as i64;
        Ok(field.wrapping_sub(EXPONENT_BIAS))
    }

    /// Whether `self` is the canonical XFL zero.
    ///
    /// Pure bit test, no host call, infallible: zero has exactly one
    /// canonical encoding (raw bits `0`), so this is exact even for a value
    /// obtained through [`XFL::from_raw_bits`] with an invalid or
    /// non-canonical bit pattern — such a value simply isn't the zero
    /// encoding. Unlike [`XFL::is_strictly_positive`]/
    /// [`XFL::is_strictly_negative`], no canonical-encoding check is needed
    /// first.
    ///
    /// # Examples
    ///
    /// ```
    /// use rshooks::XFL;
    /// use rshooks::xfl::XFL as XflType;
    ///
    /// assert!(XflType::from_raw_bits(0).is_zero());
    /// assert!(!XFL!(1).is_zero());
    /// ```
    #[inline(always)]
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Whether `self` is strictly greater than zero.
    ///
    /// Fallible, unlike [`XFL::is_zero`]: classifying a sign first requires
    /// confirming `self` is a canonical encoding at all. A value from
    /// [`XFL::from_raw_bits`] can hold an out-of-range mantissa/exponent, or
    /// a negative bit pattern reserved for a Hook API error code — neither
    /// has a numeric sign, so this returns
    /// `Err(`[`crate::error::HookError::InvalidFloat`]`)` instead of
    /// misreading the sign bit, mirroring the host's own
    /// `RETURN_IF_INVALID_FLOAT` gate (see [`is_canonical`]).
    ///
    /// Implemented as a local bit test, not a `float_sign`/`float_compare`
    /// host round trip, since the canonical-range rule and sign convention
    /// are fixed protocol constants. `self.is_zero()` is neither strictly
    /// positive nor strictly negative.
    ///
    /// # Examples
    ///
    /// ```
    /// use rshooks::XFL;
    /// use rshooks::error::HookError;
    /// use rshooks::xfl::XFL as XflType;
    ///
    /// assert_eq!(XFL!(1).is_strictly_positive(), Ok(true));
    /// assert_eq!(XFL!(-1).is_strictly_positive(), Ok(false));
    /// assert_eq!(XFL!(0).is_strictly_positive(), Ok(false));
    ///
    /// // Application-side error mapping: fold an invalid encoding into a
    /// // domain-specific decision instead of propagating `HookError` further.
    /// fn accepts_only_positive(amount: XflType) -> Result<(), &'static str> {
    ///     match amount.is_strictly_positive() {
    ///         Ok(true) => Ok(()),
    ///         Ok(false) => Err("amount must be positive"),
    ///         Err(HookError::InvalidFloat) => Err("amount is not a valid XFL value"),
    ///         Err(_) => Err("could not evaluate amount"),
    ///     }
    /// }
    ///
    /// assert_eq!(accepts_only_positive(XflType::from_raw_bits(-1)),
    ///     Err("amount is not a valid XFL value"));
    /// ```
    #[inline(always)]
    pub fn is_strictly_positive(self) -> Result<bool> {
        let bits = self.raw_bits();
        if bits == 0 {
            return Ok(false);
        }
        if !is_canonical(bits) {
            return Err(HookError::InvalidFloat);
        }
        Ok((self.0 >> SIGN_SHIFT) & 1 != 0)
    }

    /// Whether `self` is strictly less than zero.
    ///
    /// Mirror image of [`XFL::is_strictly_positive`] — see its doc comment
    /// for the fallibility rationale and canonical-encoding gate; this
    /// differs only in reading the sign bit clear (not set) on a canonical
    /// nonzero value.
    ///
    /// # Examples
    ///
    /// ```
    /// use rshooks::XFL;
    ///
    /// assert_eq!(XFL!(-1).is_strictly_negative(), Ok(true));
    /// assert_eq!(XFL!(1).is_strictly_negative(), Ok(false));
    /// assert_eq!(XFL!(0).is_strictly_negative(), Ok(false));
    /// ```
    #[inline(always)]
    pub fn is_strictly_negative(self) -> Result<bool> {
        let bits = self.raw_bits();
        if bits == 0 {
            return Ok(false);
        }
        if !is_canonical(bits) {
            return Err(HookError::InvalidFloat);
        }
        Ok((self.0 >> SIGN_SHIFT) & 1 == 0)
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

impl IouAmount {
    /// Decodes the amount's value component as an [`XFL`], via
    /// [`XFL::sto_set`] (`float_sto_set`).
    ///
    /// Hands the host exactly the 8-byte value component, never the full
    /// 48 bytes and never a local bit-reinterpret: the wire value component
    /// sets an always-on "not native" flag bit a real XFL never sets, so
    /// either shortcut produces a wrong result — see
    /// [`api::float::float_sto_set`]'s doc comment.
    #[inline(always)]
    pub fn xfl(&self) -> Result<XFL> {
        let value = self.0.get(..crate::types::NATIVE_AMOUNT_LEN).unwrap_or(&[]);
        XFL::sto_set(value)
    }
}

impl From<XFL> for u64 {
    /// The internal `u64` bit pattern — the same value [`XFL::raw_bits`]
    /// returns, just as `u64` (`XFL`'s native storage shape) rather than
    /// `i64` (the FFI-boundary shape `raw_bits`/`from_raw_bits` keep for
    /// interop). Lets a caller write `u64::from(xfl)`/`xfl.into()` instead
    /// of `xfl.raw_bits() as u64`.
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
    // Dispatches to this module's own fallible `Add`/`Neg` impls, not raw
    // integer arithmetic -- `clippy::arithmetic_side_effects` can't tell
    // the difference from the operator syntax alone, so it flags this
    // unconditionally with no real overflow/panic risk.
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
    /// failure; call [`XFL::eq`] directly for the real `Result<bool>`.
    #[inline(always)]
    fn eq(&self, other: &XFL) -> bool {
        XFL::eq(*self, *other).unwrap_or(false)
    }
}

impl PartialOrd for XFL {
    /// `self.partial_cmp(other)`, via up to two `float_compare` host calls
    /// (`COMPARE_LESS`, then — only if that came back `false` —
    /// `COMPARE_GREATER`; `false` for both means `Ordering::Equal`). Falls
    /// back to `None` on a `float_compare` failure at either step; call
    /// [`XFL::lt`]/[`XFL::gt`]/[`XFL::compare`] directly for the real
    /// `Result<bool>`.
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
// `impl $Trait<Result<XFL, HookError>> for XFL` for each `$Trait::$method`,
// so a chain like `((a + b) + c) + d` short-circuits on the first error
// without an explicit `?` between steps.
//
// Rust's orphan rules disallow `Result` on both sides of an operator impl —
// combine independently fallible values with `?` first.
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
        // Must agree exactly with `xfl.raw_bits() as u64` -- same bits, just
        // `u64` (native storage) instead of `i64` (FFI-boundary shape).
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
        // `matches!`, not `assert_eq!`: `XFL`'s `PartialEq` forwards to the
        // fallible `float_compare`-backed `eq` and falls back to `false` on
        // failure, so `assert_eq!` comparing two `Ok(XFL)`s would be a trap.
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
        // Given an `Err` input, the chained op never reaches the host stub;
        // a mismatched-code assertion here would fail if the wrong error
        // propagated. `matches!`, not `assert_eq!`, as above.
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
    // Bound to a variable rather than asserted inline: `clippy::
    // bool_assert_comparison` wants `assert!(!(...))`, but `clippy::
    // neg_cmp_op_on_partial_ord` objects to negating `<`/`>` directly since
    // the operands could be incomparable.
    fn comparison_operators_fall_back_like_f64_nan_on_host() {
        // `float_compare`'s host stub deterministically errors, so every
        // `PartialEq`/`PartialOrd` call here exercises the `false`/`None`
        // fallback rather than rolling back -- `rollback` never returns on
        // a host target, so falling back instead is what lets this test
        // return at all.
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

    /// Builds a raw XFL encoding directly from its stored fields, bypassing
    /// `XFL::new`/`float_set` (unavailable without the `testenv` feature's
    /// host stub) — mirrors `hook_float::make_float`'s field layout, so
    /// boundary and invalid encodings can be constructed precisely.
    /// `exponent_field` is the stored (biased) field, not the unbiased
    /// exponent — callers pass the field value directly (see
    /// `exponent_decodes_bias_97_field` above for the bias-97 mapping).
    fn encode(mantissa: u64, exponent_field: u64, positive: bool) -> i64 {
        let mut bits = mantissa | (exponent_field << EXPONENT_SHIFT);
        if positive {
            bits |= 1u64 << SIGN_SHIFT;
        }
        bits as i64
    }

    // Pinned as literal bit patterns rather than built via `XFL!` — these
    // are the same reference vectors `tests/ui/pass/xfl_const.rs` pins for
    // `XFL!(1)`/`XFL!(-1)`/`XFL!(0.1)`; `NEG_TENTH_BITS` is `TENTH_BITS`
    // with only the sign bit (62) flipped.
    const ONE_BITS: i64 = 6_089_866_696_204_910_592;
    const NEG_ONE_BITS: i64 = 1_478_180_677_777_522_688;
    const TENTH_BITS: i64 = 6_071_852_297_695_428_608;
    const NEG_TENTH_BITS: i64 = 1_460_166_279_268_040_704;

    #[test]
    fn is_zero_is_pure_bit_equality_against_canonical_zero() {
        assert!(XFL::from_raw_bits(0).is_zero());
        assert!(!XFL::from_raw_bits(ONE_BITS).is_zero());
        assert!(!XFL::from_raw_bits(NEG_ONE_BITS).is_zero());
        // Not zero even though invalid -- `is_zero` never claims validity,
        // it only ever compares against the one canonical zero pattern.
        assert!(!XFL::from_raw_bits(-1).is_zero());
        assert!(!XFL::from_raw_bits(i64::MIN).is_zero());
    }

    #[test]
    fn strictly_positive_and_negative_match_known_values() {
        // Integers.
        assert_eq!(
            XFL::from_raw_bits(ONE_BITS).is_strictly_positive(),
            Ok(true)
        );
        assert_eq!(
            XFL::from_raw_bits(ONE_BITS).is_strictly_negative(),
            Ok(false)
        );
        assert_eq!(
            XFL::from_raw_bits(NEG_ONE_BITS).is_strictly_positive(),
            Ok(false)
        );
        assert_eq!(
            XFL::from_raw_bits(NEG_ONE_BITS).is_strictly_negative(),
            Ok(true)
        );
        // Fractions.
        assert_eq!(
            XFL::from_raw_bits(TENTH_BITS).is_strictly_positive(),
            Ok(true)
        );
        assert_eq!(
            XFL::from_raw_bits(TENTH_BITS).is_strictly_negative(),
            Ok(false)
        );
        assert_eq!(
            XFL::from_raw_bits(NEG_TENTH_BITS).is_strictly_positive(),
            Ok(false)
        );
        assert_eq!(
            XFL::from_raw_bits(NEG_TENTH_BITS).is_strictly_negative(),
            Ok(true)
        );
        // Zero is neither.
        assert_eq!(XFL::from_raw_bits(0).is_strictly_positive(), Ok(false));
        assert_eq!(XFL::from_raw_bits(0).is_strictly_negative(), Ok(false));
        assert!(XFL::from_raw_bits(0).is_zero());
    }

    #[test]
    fn boundary_magnitudes_are_canonical_and_correctly_signed() {
        // Min exponent (-96) at the min mantissa: field 1.
        let min_pos = XFL::from_raw_bits(encode(1_000_000_000_000_000, 1, true));
        let min_neg = XFL::from_raw_bits(encode(1_000_000_000_000_000, 1, false));
        assert_eq!(min_pos.is_strictly_positive(), Ok(true));
        assert_eq!(min_pos.is_strictly_negative(), Ok(false));
        assert_eq!(min_neg.is_strictly_positive(), Ok(false));
        assert_eq!(min_neg.is_strictly_negative(), Ok(true));

        // Max exponent (+80) at the max mantissa: field 177.
        let max_pos = XFL::from_raw_bits(encode(9_999_999_999_999_999, 177, true));
        let max_neg = XFL::from_raw_bits(encode(9_999_999_999_999_999, 177, false));
        assert_eq!(max_pos.is_strictly_positive(), Ok(true));
        assert_eq!(max_pos.is_strictly_negative(), Ok(false));
        assert_eq!(max_neg.is_strictly_positive(), Ok(false));
        assert_eq!(max_neg.is_strictly_negative(), Ok(true));
    }

    #[test]
    fn invalid_encodings_error_instead_of_reading_the_sign_bit() {
        // Negative raw bits: the Hook API's error-code channel, never a
        // valid float, regardless of what the lower bits look like.
        for bits in [-1i64, i64::MIN] {
            let v = XFL::from_raw_bits(bits);
            assert!(!v.is_zero());
            assert_eq!(v.is_strictly_positive(), Err(HookError::InvalidFloat));
            assert_eq!(v.is_strictly_negative(), Err(HookError::InvalidFloat));
        }

        // Mantissa field below the canonical minimum (field 97 = unbiased 0).
        let mantissa_too_small = XFL::from_raw_bits(encode(999_999_999_999_999, 97, true));
        assert_eq!(
            mantissa_too_small.is_strictly_positive(),
            Err(HookError::InvalidFloat)
        );
        assert_eq!(
            mantissa_too_small.is_strictly_negative(),
            Err(HookError::InvalidFloat)
        );

        // Mantissa field above the canonical maximum (still fits the 54-bit
        // field: 10^16 < 2^54 - 1).
        let mantissa_too_large = XFL::from_raw_bits(encode(10_000_000_000_000_000, 97, true));
        assert_eq!(
            mantissa_too_large.is_strictly_positive(),
            Err(HookError::InvalidFloat)
        );
        assert_eq!(
            mantissa_too_large.is_strictly_negative(),
            Err(HookError::InvalidFloat)
        );

        // Exponent field below the canonical minimum (field 0 = unbiased -97).
        let exponent_too_small = XFL::from_raw_bits(encode(1_000_000_000_000_000, 0, true));
        assert_eq!(
            exponent_too_small.is_strictly_positive(),
            Err(HookError::InvalidFloat)
        );
        assert_eq!(
            exponent_too_small.is_strictly_negative(),
            Err(HookError::InvalidFloat)
        );

        // Exponent field above the canonical maximum (field 178 = unbiased
        // +81).
        let exponent_too_large = XFL::from_raw_bits(encode(9_999_999_999_999_999, 178, true));
        assert_eq!(
            exponent_too_large.is_strictly_positive(),
            Err(HookError::InvalidFloat)
        );
        assert_eq!(
            exponent_too_large.is_strictly_negative(),
            Err(HookError::InvalidFloat)
        );

        // A value none of these predicates ever classify from the sign bit
        // alone: mantissa/exponent are both invalid, but bit 62 (the sign
        // bit) happens to be set the same way a valid positive value's
        // would be.
        let doubly_invalid = XFL::from_raw_bits(encode(1, 255, true));
        assert_eq!(
            doubly_invalid.is_strictly_positive(),
            Err(HookError::InvalidFloat)
        );
    }

    #[test]
    fn exactly_one_predicate_holds_for_every_valid_value() {
        let cases = [
            XFL::from_raw_bits(0),
            XFL::from_raw_bits(ONE_BITS),
            XFL::from_raw_bits(NEG_ONE_BITS),
            XFL::from_raw_bits(TENTH_BITS),
            XFL::from_raw_bits(NEG_TENTH_BITS),
            XFL::from_raw_bits(encode(1_000_000_000_000_000, 1, true)),
            XFL::from_raw_bits(encode(1_000_000_000_000_000, 1, false)),
            XFL::from_raw_bits(encode(9_999_999_999_999_999, 177, true)),
            XFL::from_raw_bits(encode(9_999_999_999_999_999, 177, false)),
        ];
        for v in cases {
            let pos_result = v.is_strictly_positive();
            let neg_result = v.is_strictly_negative();
            assert!(pos_result.is_ok(), "a valid value must not error");
            assert!(neg_result.is_ok(), "a valid value must not error");
            let flags = [v.is_zero(), pos_result == Ok(true), neg_result == Ok(true)];
            let true_count = flags.into_iter().filter(|&b| b).count();
            assert_eq!(
                true_count, 1,
                "exactly one of is_zero/is_strictly_positive/is_strictly_negative \
                 must hold for {v:?}"
            );
        }
    }
}

/// Proves `IouAmount::xfl` reaches the host through exactly one
/// `float_sto_set` call, carrying only the 8-byte value component — not
/// the full 48-byte amount. See [`api::float::float_sto_set`]'s doc
/// comment for why a wider slice would be silently misparsed.
#[cfg(all(test, feature = "testenv"))]
mod testenv_tests {
    #![allow(clippy::unwrap_used, clippy::panic)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    extern crate std;

    use std::rc::Rc;

    use super::*;
    use crate::types::IOU_AMOUNT_LEN;
    use rshooks_core::backend::{HostBackend, install};

    /// Records the slice `float_sto_set` was called with and answers a
    /// fixed raw bit pattern; `accept`/`rollback` are unused here.
    struct RecordingBackend(i64);

    impl HostBackend for RecordingBackend {
        fn float_sto_set(&self, sto: &[u8]) -> i64 {
            assert_eq!(
                sto,
                &[1u8, 2, 3, 4, 5, 6, 7, 8],
                "IouAmount::xfl must pass only the 8-byte value component"
            );
            self.0
        }

        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("RecordingBackend::accept unexpectedly called")
        }

        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("RecordingBackend::rollback unexpectedly called")
        }
    }

    #[test]
    fn xfl_passes_only_the_value_component_and_returns_the_hosts_answer() {
        let _guard = install(Rc::new(RecordingBackend(42)));
        let mut bytes = [0u8; IOU_AMOUNT_LEN];
        if let Some(value) = bytes.get_mut(..8) {
            value.copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        }
        // The currency/issuer bytes are irrelevant to `xfl()`; leave zeroed.
        let amount = IouAmount(bytes);
        assert_eq!(amount.xfl().unwrap().raw_bits(), 42);
    }
}
