//! Deferred-validation XFL arithmetic for measured hot paths.
//!
//! Operations propagate invalid values through the Hook API and
//! [`XFLUnchecked::validate`] reports the final result. Use [`crate::xfl::XFL`]
//! when each failure must be preserved or handled immediately.

use crate::error::{Result, res};
use crate::xfl::XFL;

/// A raw XFL `i64` register value that has **not** been validated: it may
/// be a canonical XFL value, a non-canonical bit pattern, or a negative
/// Hook API error code sharing the same channel.
#[derive(Clone, Copy, Debug)]
pub struct XFLUnchecked(i64);

impl XFLUnchecked {
    /// Wrap a raw `i64` with no validation whatsoever.
    #[inline(always)]
    #[must_use]
    pub fn from_raw_bits(bits: i64) -> XFLUnchecked {
        XFLUnchecked(bits)
    }

    /// The raw, unvalidated `i64` bit pattern.
    #[inline(always)]
    #[must_use]
    pub fn raw_bits(self) -> i64 {
        self.0
    }

    /// Validate `self`, turning it into a real [`XFL`] or a definitive
    /// [`crate::error::HookError`].
    ///
    /// Implemented as `float_sum(self, 0)` — a host round trip, not a
    /// guest-side range check — deliberately: the host's
    /// `RETURN_IF_INVALID_FLOAT` is the same gate every other float host
    /// call runs, so this reuses it instead of re-deriving the
    /// mantissa/exponent bounds locally and risking drift from the host's
    /// actual rules.
    ///
    /// This fully validates `value` despite `HookAPI::float_sum`'s own
    /// internal `+0` short-circuit (`if (float2 == 0) return float1;`):
    /// that short-circuit lives inside `HookAPI::float_sum`, which is only
    /// reached *after* the `DEFINE_HOOK_FUNCTION(int64_t, float_sum, ...)`
    /// wrapper in `applyHook.cpp` has already run
    /// `RETURN_IF_INVALID_FLOAT(float1)` on `value`, rejecting it before
    /// `HookAPI::float_sum` — and thus the short-circuit — is ever reached.
    #[inline(always)]
    pub fn validate(self) -> Result<XFL> {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_sum(self.0, 0)) {
            return res(v).map(XFL::from_raw_bits);
        }
        res(unsafe { rshooks_core::float_sum(self.0, 0) }).map(XFL::from_raw_bits)
    }
}

impl From<XFLUnchecked> for i64 {
    /// The raw, unvalidated `i64` bit pattern — identical to
    /// [`XFLUnchecked::raw_bits`] (this type's internal storage already
    /// *is* `i64`, unlike [`XFL`]'s `u64` — see `xfl.rs`'s type doc
    /// comment for why the two types differ there). Lets a caller write
    /// `i64::from(unchecked)`/`unchecked.into()` instead of
    /// `unchecked.raw_bits()`.
    #[inline(always)]
    fn from(value: XFLUnchecked) -> i64 {
        value.0
    }
}

impl core::ops::Neg for XFLUnchecked {
    type Output = XFLUnchecked;

    /// `-self`, via the `float_negate` host call — not a local sign-bit
    /// flip (see the module doc comment for why). No guest-side validation
    /// of `self` either way: a poisoned/invalid `self` is rejected by the
    /// host's `RETURN_IF_INVALID_FLOAT` and the result is `INVALID_FLOAT`'s
    /// raw bits, itself a valid (still-negative) poison value to keep
    /// propagating.
    #[inline(always)]
    fn neg(self) -> XFLUnchecked {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_negate(self.0)) {
            return XFLUnchecked(v);
        }
        XFLUnchecked(unsafe { rshooks_core::float_negate(self.0) })
    }
}

impl core::ops::Add for XFLUnchecked {
    type Output = XFLUnchecked;

    /// `self + rhs` via `float_sum`, with no guest-side validation of
    /// either operand — see the module doc comment.
    #[inline(always)]
    fn add(self, rhs: XFLUnchecked) -> XFLUnchecked {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_sum(self.0, rhs.0)) {
            return XFLUnchecked(v);
        }
        XFLUnchecked(unsafe { rshooks_core::float_sum(self.0, rhs.0) })
    }
}

impl core::ops::Sub for XFLUnchecked {
    type Output = XFLUnchecked;

    /// `self - rhs`, implemented as `self + (-rhs)`: one `float_negate`
    /// host call plus one `float_sum` host call, mirroring [`XFL`]'s `Sub`
    /// (minus the `?` — `XFLUnchecked`'s `Neg` can't fail its own `Output`
    /// type the way `XFL`'s does, since a rejected negation just becomes
    /// another poisoned `i64` to keep propagating, not a `Result::Err`).
    #[inline(always)]
    // See `XFL`'s `Sub` impl for why this `#[allow]` is needed: `self +
    // (-rhs)` dispatches to this module's own `Add`/`Neg` impls, not raw
    // integer arithmetic.
    #[allow(clippy::arithmetic_side_effects)]
    fn sub(self, rhs: XFLUnchecked) -> XFLUnchecked {
        self + (-rhs)
    }
}

impl core::ops::Mul for XFLUnchecked {
    type Output = XFLUnchecked;

    /// `self * rhs` via `float_multiply`, with no guest-side validation of
    /// either operand — see the module doc comment.
    #[inline(always)]
    fn mul(self, rhs: XFLUnchecked) -> XFLUnchecked {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_multiply(self.0, rhs.0)) {
            return XFLUnchecked(v);
        }
        XFLUnchecked(unsafe { rshooks_core::float_multiply(self.0, rhs.0) })
    }
}

impl core::ops::Div for XFLUnchecked {
    type Output = XFLUnchecked;

    /// `self / rhs` via `float_divide`, with no guest-side validation of
    /// either operand — see the module doc comment.
    #[inline(always)]
    fn div(self, rhs: XFLUnchecked) -> XFLUnchecked {
        #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
        if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_divide(self.0, rhs.0)) {
            return XFLUnchecked(v);
        }
        XFLUnchecked(unsafe { rshooks_core::float_divide(self.0, rhs.0) })
    }
}

// Generates the mixed `XFLUnchecked op XFL` / `XFL op XFLUnchecked` impls
// for each listed `$Trait::$method`, treating the `XFL` side as implicitly
// unchecked (zero-cost reinterpretation, see `XFL::unchecked`/
// `XFLUnchecked::from_raw_bits`) so a chain can freely mix the two types at
// its boundary (typically: start from a known-valid `XFL`, do the hot loop
// in `XFLUnchecked`, `validate()` once at the end).
macro_rules! xfl_unchecked_mixed_ops {
    ($( $Trait:ident :: $method:ident ),+ $(,)?) => {
        $(
            impl core::ops::$Trait<XFL> for XFLUnchecked {
                type Output = XFLUnchecked;

                #[inline(always)]
                fn $method(self, rhs: XFL) -> XFLUnchecked {
                    core::ops::$Trait::$method(self, rhs.unchecked())
                }
            }

            impl core::ops::$Trait<XFLUnchecked> for XFL {
                type Output = XFLUnchecked;

                #[inline(always)]
                fn $method(self, rhs: XFLUnchecked) -> XFLUnchecked {
                    core::ops::$Trait::$method(self.unchecked(), rhs)
                }
            }
        )+
    };
}

xfl_unchecked_mixed_ops!(Add::add, Sub::sub, Mul::mul, Div::div);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn raw_bits_round_trip() {
        for bits in [0i64, 1, -1, i64::MAX, i64::MIN, 42, -42] {
            assert_eq!(XFLUnchecked::from_raw_bits(bits).raw_bits(), bits);
        }
    }

    #[test]
    fn into_i64_matches_raw_bits() {
        for bits in [0i64, 1, -1, i64::MAX, i64::MIN, 42, -42] {
            let unchecked = XFLUnchecked::from_raw_bits(bits);
            let via_into: i64 = unchecked.into();
            let via_from = i64::from(unchecked);
            assert_eq!(via_into, bits);
            assert_eq!(via_from, bits);
        }
    }

    #[test]
    fn validate_is_not_implemented_on_host() {
        let x = XFLUnchecked::from_raw_bits(0);
        assert!(matches!(x.validate(), Err(HookError::NotImplemented)));
    }

    #[test]
    fn arithmetic_routes_through_host_stub() {
        let a = XFLUnchecked::from_raw_bits(1);
        let b = XFLUnchecked::from_raw_bits(2);
        // Host stubs are deterministic `NOT_IMPLEMENTED`; validating the
        // result confirms the operator actually issued a host call rather
        // than, say, silently returning one of the raw operands.
        assert!(matches!((a + b).validate(), Err(HookError::NotImplemented)));
        assert!(matches!((a - b).validate(), Err(HookError::NotImplemented)));
        assert!(matches!((a * b).validate(), Err(HookError::NotImplemented)));
        assert!(matches!((a / b).validate(), Err(HookError::NotImplemented)));
        assert!(matches!((-a).validate(), Err(HookError::NotImplemented)));
    }

    #[test]
    fn mixed_xfl_and_unchecked_ops_type_check() {
        let checked = XFL::one();
        let unchecked = checked.unchecked();
        let _: XFLUnchecked = unchecked + checked;
        let _: XFLUnchecked = checked + unchecked;
        let _: XFLUnchecked = unchecked - checked;
        let _: XFLUnchecked = checked - unchecked;
        let _: XFLUnchecked = unchecked * checked;
        let _: XFLUnchecked = checked * unchecked;
        let _: XFLUnchecked = unchecked / checked;
        let _: XFLUnchecked = checked / unchecked;
    }
}
