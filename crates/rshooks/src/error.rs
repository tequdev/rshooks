//! Hook API error model.
//!
//! Every Hook API function returns an `i64`: non-negative values are success
//! payloads (often "bytes written" or a slot/field-pointer value), negative
//! values are one of the 45 error codes from `hook/error.h`. [`HookError`] is
//! a typed, exhaustive-by-construction mirror of those codes (plus
//! [`HookError::Unknown`] for forward-compatibility with codes this crate
//! does not yet know about).

/// A Hook API error, decoded from the negative `i64` return of a raw
/// `rshooks-core` call.
///
/// # Examples
///
/// ```
/// use rshooks::error::HookError;
///
/// let err = HookError::from(-5);
/// assert_eq!(err, HookError::DoesntExist);
/// assert_eq!(err.code(), -5);
/// ```
// `#[repr(u8)]` is load-bearing for `code()` below: it lets the discriminant
// be read via unsafe pointer casting even though `Unknown(i64)` carries data
// (see the Rust reference, "Casting" > "Pointer casting" for enums with a
// primitive representation). Declaration order 0..=44 must stay in exact
// sync with `code()`'s `TABLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HookError {
    /// `OUT_OF_BOUNDS` (-1): memory access out of bounds.
    OutOfBounds,
    /// `INTERNAL_ERROR` (-2): unexpected internal error.
    InternalError,
    /// `TOO_BIG` (-3): data is too large.
    TooBig,
    /// `TOO_SMALL` (-4): buffer is too small.
    TooSmall,
    /// `DOESNT_EXIST` (-5): requested item does not exist.
    DoesntExist,
    /// `NO_FREE_SLOTS` (-6): no available slots.
    NoFreeSlots,
    /// `INVALID_ARGUMENT` (-7): argument is invalid.
    InvalidArgument,
    /// `ALREADY_SET` (-8): already configured.
    AlreadySet,
    /// `PREREQUISITE_NOT_MET` (-9): prerequisite condition not satisfied.
    PrerequisiteNotMet,
    /// `FEE_TOO_LARGE` (-10): fee is too large.
    FeeTooLarge,
    /// `EMISSION_FAILURE` (-11): transaction emission failed.
    EmissionFailure,
    /// `TOO_MANY_NONCES` (-12): nonce generation limit exceeded.
    TooManyNonces,
    /// `TOO_MANY_EMITTED_TXN` (-13): emitted transaction limit exceeded.
    TooManyEmittedTxn,
    /// `NOT_IMPLEMENTED` (-14): feature not implemented (also what every
    /// host-build stub returns).
    NotImplemented,
    /// `INVALID_ACCOUNT` (-15): account ID is invalid.
    InvalidAccount,
    /// `GUARD_VIOLATION` (-16): infinite-loop guard violation.
    GuardViolation,
    /// `INVALID_FIELD` (-17): field ID is invalid.
    InvalidField,
    /// `PARSE_ERROR` (-18): failed to parse data.
    ParseError,
    /// `RC_ROLLBACK` (-19): hook execution terminated via rollback.
    RcRollback,
    /// `RC_ACCEPT` (-20): hook execution terminated via accept.
    RcAccept,
    /// `NO_SUCH_KEYLET` (-21): keylet not found.
    NoSuchKeylet,
    /// `NOT_AN_ARRAY` (-22): object is not an array.
    NotAnArray,
    /// `NOT_AN_OBJECT` (-23): object is not an object.
    NotAnObject,
    /// `INVALID_FLOAT` (-10024, verbatim from the header — not -24): XFL
    /// value is invalid.
    InvalidFloat,
    /// `DIVISION_BY_ZERO` (-25): division by zero.
    DivisionByZero,
    /// `MANTISSA_OVERSIZED` (-26): XFL mantissa too large.
    MantissaOversized,
    /// `MANTISSA_UNDERSIZED` (-27): XFL mantissa too small.
    MantissaUndersized,
    /// `EXPONENT_OVERSIZED` (-28): XFL exponent too large.
    ExponentOversized,
    /// `EXPONENT_UNDERSIZED` (-29): XFL exponent too small.
    ExponentUndersized,
    /// `XFL_OVERFLOW` (-30): XFL arithmetic overflow.
    XflOverflow,
    /// `NOT_IOU_AMOUNT` (-31): not an IOU amount.
    NotIouAmount,
    /// `NOT_AN_AMOUNT` (-32): not an amount type.
    NotAnAmount,
    /// `CANT_RETURN_NEGATIVE` (-33): cannot return a negative value.
    CantReturnNegative,
    /// `NOT_AUTHORIZED` (-34): no access permission.
    NotAuthorized,
    /// `PREVIOUS_FAILURE_PREVENTS_RETRY` (-35): a previous failure prevents
    /// retrying this operation.
    PreviousFailurePreventsRetry,
    /// `TOO_MANY_PARAMS` (-36): parameter limit exceeded.
    TooManyParams,
    /// `INVALID_TXN` (-37): transaction is invalid.
    InvalidTxn,
    /// `RESERVE_INSUFFICIENT` (-38): reserve insufficient for the operation.
    ReserveInsufficient,
    /// `COMPLEX_NOT_SUPPORTED` (-39): complex-domain result not supported.
    ComplexNotSupported,
    /// `DOES_NOT_MATCH` (-40): values do not match.
    DoesNotMatch,
    /// `INVALID_KEY` (-41): key is invalid.
    InvalidKey,
    /// `NOT_A_STRING` (-42): value is not a string.
    NotAString,
    /// `MEM_OVERLAP` (-43): memory regions overlap.
    MemOverlap,
    /// `TOO_MANY_STATE_MODIFICATIONS` (-44): state modification limit
    /// exceeded.
    TooManyStateModifications,
    /// `TOO_MANY_NAMESPACES` (-45): namespace limit exceeded.
    TooManyNamespaces,
    /// A negative code this version of rshooks does not recognize yet.
    /// Carries the raw code for forward-compatibility.
    Unknown(i64),
}

impl From<i64> for HookError {
    fn from(code: i64) -> Self {
        // Indexed table instead of a 46-arm match: LLVM lowers the match into a
        // ~45-deep nested-block decision tree (the Unknown(i64) payload prevents an
        // identity mapping), which alone blows the Guard-type nesting limit once
        // inlined into hook/cbak. A table lookup is nesting-depth ~1.
        //
        // `INVALID_FLOAT` is handled *before* the table, not inside it: its value is
        // `-10024`, not the `-24` its declaration-order position would suggest (see
        // `rshooks_core::INVALID_FLOAT`'s own doc comment — "kept verbatim; this is
        // not a typo in this translation"). Folding it into the table naively (by
        // declaration-order position, matching the original match arms' order) would
        // both mis-map a real `-24` return to `InvalidFloat` and, far worse, silently
        // stop recognizing genuine `-10024` returns as `InvalidFloat` (they'd fall
        // through to `Unknown`, breaking any caller that specifically matches on
        // `HookError::InvalidFloat`) -- caught by cross-checking this table against
        // `rshooks_core::error`'s constants sorted by value, not by source order.
        // `-24` itself is not assigned to anything and is left a genuine gap (`None`)
        // below, matching `error.h` upstream.
        if code == rshooks_core::INVALID_FLOAT {
            return HookError::InvalidFloat;
        }
        const TABLE: [Option<HookError>; 45] = [
            Some(HookError::OutOfBounds),
            Some(HookError::InternalError),
            Some(HookError::TooBig),
            Some(HookError::TooSmall),
            Some(HookError::DoesntExist),
            Some(HookError::NoFreeSlots),
            Some(HookError::InvalidArgument),
            Some(HookError::AlreadySet),
            Some(HookError::PrerequisiteNotMet),
            Some(HookError::FeeTooLarge),
            Some(HookError::EmissionFailure),
            Some(HookError::TooManyNonces),
            Some(HookError::TooManyEmittedTxn),
            Some(HookError::NotImplemented),
            Some(HookError::InvalidAccount),
            Some(HookError::GuardViolation),
            Some(HookError::InvalidField),
            Some(HookError::ParseError),
            Some(HookError::RcRollback),
            Some(HookError::RcAccept),
            Some(HookError::NoSuchKeylet),
            Some(HookError::NotAnArray),
            Some(HookError::NotAnObject),
            None, // -24: unassigned upstream (INVALID_FLOAT is -10024, handled above)
            Some(HookError::DivisionByZero),
            Some(HookError::MantissaOversized),
            Some(HookError::MantissaUndersized),
            Some(HookError::ExponentOversized),
            Some(HookError::ExponentUndersized),
            Some(HookError::XflOverflow),
            Some(HookError::NotIouAmount),
            Some(HookError::NotAnAmount),
            Some(HookError::CantReturnNegative),
            Some(HookError::NotAuthorized),
            Some(HookError::PreviousFailurePreventsRetry),
            Some(HookError::TooManyParams),
            Some(HookError::InvalidTxn),
            Some(HookError::ReserveInsufficient),
            Some(HookError::ComplexNotSupported),
            Some(HookError::DoesNotMatch),
            Some(HookError::InvalidKey),
            Some(HookError::NotAString),
            Some(HookError::MemOverlap),
            Some(HookError::TooManyStateModifications),
            Some(HookError::TooManyNamespaces),
        ];
        let idx = code.wrapping_neg().wrapping_sub(1);
        match usize::try_from(idx).ok().and_then(|i| TABLE.get(i)).copied().flatten() {
            Some(e) => e,
            None => HookError::Unknown(code),
        }
    }
}

impl HookError {
    /// The raw negative `i64` error code this variant corresponds to. Exact
    /// inverse of [`HookError::from`]: `HookError::from(c).code() == c` for
    /// every code, known or unknown.
    ///
    /// Like `From`'s inverse, this is deliberately NOT a 46-arm match:
    /// matching on the variant to select 1-of-45 distinct wide `i64`
    /// constants requires WASM structured control flow to nest a nearly
    /// equal number of blocks (unlike a native jump table, WASM's
    /// `br_table` still needs one nested block per distinct branch target),
    /// which alone can blow the Guard-type 16-level nesting limit once
    /// inlined. Instead this reads the variant's discriminant directly
    /// (an O(1) memory load, no branching) and indexes a const table.
    #[must_use]
    pub fn code(&self) -> i64 {
        // The one payload-carrying arm is handled by a single two-way
        // branch, not folded into the table: it can't participate in a
        // dense discriminant-indexed lookup since its code is data, not a
        // per-variant constant.
        if let HookError::Unknown(code) = *self {
            return code;
        }
        // SAFETY: `HookError` is `#[repr(u8)]`, so per the Rust reference
        // ("Casting" > "Pointer casting"), the discriminant is reliably
        // readable via this pointer cast even though `Unknown` carries
        // data. `self` is not `Unknown` here (handled above), so this
        // reads the u8 discriminant of one of the 45 fieldless variants,
        // which by declaration order is in `0..45`.
        let tag = unsafe { *(self as *const Self as *const u8) };
        const TABLE: [i64; 45] = [
            rshooks_core::OUT_OF_BOUNDS,
            rshooks_core::INTERNAL_ERROR,
            rshooks_core::TOO_BIG,
            rshooks_core::TOO_SMALL,
            rshooks_core::DOESNT_EXIST,
            rshooks_core::NO_FREE_SLOTS,
            rshooks_core::INVALID_ARGUMENT,
            rshooks_core::ALREADY_SET,
            rshooks_core::PREREQUISITE_NOT_MET,
            rshooks_core::FEE_TOO_LARGE,
            rshooks_core::EMISSION_FAILURE,
            rshooks_core::TOO_MANY_NONCES,
            rshooks_core::TOO_MANY_EMITTED_TXN,
            rshooks_core::NOT_IMPLEMENTED,
            rshooks_core::INVALID_ACCOUNT,
            rshooks_core::GUARD_VIOLATION,
            rshooks_core::INVALID_FIELD,
            rshooks_core::PARSE_ERROR,
            rshooks_core::RC_ROLLBACK,
            rshooks_core::RC_ACCEPT,
            rshooks_core::NO_SUCH_KEYLET,
            rshooks_core::NOT_AN_ARRAY,
            rshooks_core::NOT_AN_OBJECT,
            rshooks_core::INVALID_FLOAT, // tag 23 (InvalidFloat), value -10024 not -24
            rshooks_core::DIVISION_BY_ZERO,
            rshooks_core::MANTISSA_OVERSIZED,
            rshooks_core::MANTISSA_UNDERSIZED,
            rshooks_core::EXPONENT_OVERSIZED,
            rshooks_core::EXPONENT_UNDERSIZED,
            rshooks_core::XFL_OVERFLOW,
            rshooks_core::NOT_IOU_AMOUNT,
            rshooks_core::NOT_AN_AMOUNT,
            rshooks_core::CANT_RETURN_NEGATIVE,
            rshooks_core::NOT_AUTHORIZED,
            rshooks_core::PREVIOUS_FAILURE_PREVENTS_RETRY,
            rshooks_core::TOO_MANY_PARAMS,
            rshooks_core::INVALID_TXN,
            rshooks_core::RESERVE_INSUFFICIENT,
            rshooks_core::COMPLEX_NOT_SUPPORTED,
            rshooks_core::DOES_NOT_MATCH,
            rshooks_core::INVALID_KEY,
            rshooks_core::NOT_A_STRING,
            rshooks_core::MEM_OVERLAP,
            rshooks_core::TOO_MANY_STATE_MODIFICATIONS,
            rshooks_core::TOO_MANY_NAMESPACES,
        ];
        // `.get` + `unwrap_or`, not `TABLE[..]`: this crate denies
        // `clippy::indexing_slicing` (panic-free is enforced, not
        // promised, per `docs/DESIGN.md` §8). `tag` is provably in
        // `0..45` here, so the fallback is unreachable in practice --
        // mirrors `From`'s own `.get(..)` table lookup above.
        TABLE.get(tag as usize).copied().unwrap_or(rshooks_core::INTERNAL_ERROR)
    }
}

/// The result type every rshooks wrapper returns: `Ok(payload)` for a
/// non-negative Hook API return, `Err(HookError)` for a negative one.
pub type Result<T> = core::result::Result<T, HookError>;

/// Funnel point every wrapper in `api/*.rs` and `xfl.rs` calls through:
/// convert a raw Hook API `i64` return into a [`Result<i64>`] by sign.
#[inline(always)]
pub(crate) fn res(code: i64) -> Result<i64> {
    if code < 0 {
        Err(HookError::from(code))
    } else {
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_known_codes() {
        let known: &[i64] = &[
            -1, -2, -3, -4, -5, -6, -7, -8, -9, -10, -11, -12, -13, -14, -15, -16, -17, -18, -19,
            -20, -21, -22, -23, -10024, -25, -26, -27, -28, -29, -30, -31, -32, -33, -34, -35, -36,
            -37, -38, -39, -40, -41, -42, -43, -44, -45,
        ];
        for &code in known {
            assert_eq!(
                HookError::from(code).code(),
                code,
                "round-trip failed for {code}"
            );
        }
    }

    #[test]
    fn invalid_float_is_irregular() {
        assert_eq!(HookError::from(-10024), HookError::InvalidFloat);
        assert_eq!(HookError::InvalidFloat.code(), -10024);
    }

    #[test]
    fn discriminant_matches_declaration_order() {
        // Guards the invariant `code()`'s SAFETY comment relies on: the u8
        // discriminant read via pointer casting must match each variant's
        // position in the enum's declaration (and thus its slot in
        // `code()`'s `TABLE`). Spot-checks the first, last, and one
        // interior fieldless variant, plus `Unknown`'s discriminant (45,
        // even though `code()` never indexes the table with it).
        fn discriminant(e: &HookError) -> u8 {
            // SAFETY: same reasoning as `code()` -- see its doc comment.
            unsafe { *(e as *const HookError as *const u8) }
        }
        assert_eq!(discriminant(&HookError::OutOfBounds), 0);
        assert_eq!(discriminant(&HookError::InvalidFloat), 23);
        assert_eq!(discriminant(&HookError::TooManyNamespaces), 44);
        assert_eq!(discriminant(&HookError::Unknown(-9999)), 45);
    }

    #[test]
    fn unknown_code_round_trips() {
        let err = HookError::from(-9999);
        assert_eq!(err, HookError::Unknown(-9999));
        assert_eq!(err.code(), -9999);
    }

    #[test]
    fn res_splits_on_sign() {
        assert_eq!(res(0), Ok(0));
        assert_eq!(res(32), Ok(32));
        assert_eq!(res(-5), Err(HookError::DoesntExist));
    }
}
