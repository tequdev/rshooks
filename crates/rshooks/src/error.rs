//! Hook API error model.
//!
//! Every Hook API function returns an `i64`: non-negative values are success
//! payloads (often "bytes written" or a slot/field-pointer value), negative
//! values are one of the error codes from `hook/error.h`. [`HookError`] is a
//! transparent newtype over that raw code: constructing one from a Hook API
//! return, and reading the code back out, is the identity. [`HookErrorKind`]
//! is the decoded, exhaustive-by-construction view, computed on demand by
//! [`HookError::kind`].

/// A Hook API error: the raw negative `i64` return of a `rshooks-core` call.
///
/// `HookError` is `#[repr(transparent)]` over that `i64`, so building one
/// and reading [`HookError::code`] back out are both identity operations —
/// no decode. Each named Hook API error has an associated constant, in
/// PascalCase (`HookError::DoesntExist`, `HookError::TooBig`, …), usable as
/// a value (`Err(HookError::X)`) and in comparisons (`err == HookError::X`).
///
/// `HookError` is a struct, not an enum, so its constants are not patterns: a
/// `match` on a specific error is written as a guard, or dispatches on
/// [`HookError::kind`] instead.
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
///
/// Matching a specific error:
///
/// ```
/// use rshooks::error::HookError;
///
/// fn describe(err: HookError) -> &'static str {
///     match err {
///         e if e == HookError::DoesntExist => "missing",
///         e if e == HookError::TooBig => "oversized",
///         _ => "other",
///     }
/// }
/// assert_eq!(describe(HookError::DoesntExist), "missing");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct HookError(i64);

/// Builds a `HookError` from an `rshooks_core` error code.
const fn of(code: i64) -> HookError {
    HookError(code)
}

// The associated constants below are the Hook API's own error names
// (`hook/error.h`), kept in PascalCase so every existing value-position use
// (`Err(HookError::TooSmall)`, `== HookError::DoesntExist`, `?`) compiles
// unchanged; they are not local bindings subject to the usual naming
// convention.
#[allow(non_upper_case_globals)]
impl HookError {
    /// `OUT_OF_BOUNDS` (-1): memory access out of bounds.
    pub const OutOfBounds: HookError = of(rshooks_core::OUT_OF_BOUNDS);
    /// `INTERNAL_ERROR` (-2): unexpected internal error.
    pub const InternalError: HookError = of(rshooks_core::INTERNAL_ERROR);
    /// `TOO_BIG` (-3): data is too large.
    pub const TooBig: HookError = of(rshooks_core::TOO_BIG);
    /// `TOO_SMALL` (-4): buffer is too small.
    pub const TooSmall: HookError = of(rshooks_core::TOO_SMALL);
    /// `DOESNT_EXIST` (-5): requested item does not exist.
    pub const DoesntExist: HookError = of(rshooks_core::DOESNT_EXIST);
    /// `NO_FREE_SLOTS` (-6): no available slots.
    pub const NoFreeSlots: HookError = of(rshooks_core::NO_FREE_SLOTS);
    /// `INVALID_ARGUMENT` (-7): argument is invalid.
    pub const InvalidArgument: HookError = of(rshooks_core::INVALID_ARGUMENT);
    /// `ALREADY_SET` (-8): already configured.
    pub const AlreadySet: HookError = of(rshooks_core::ALREADY_SET);
    /// `PREREQUISITE_NOT_MET` (-9): prerequisite condition not satisfied.
    pub const PrerequisiteNotMet: HookError = of(rshooks_core::PREREQUISITE_NOT_MET);
    /// `FEE_TOO_LARGE` (-10): fee is too large.
    pub const FeeTooLarge: HookError = of(rshooks_core::FEE_TOO_LARGE);
    /// `EMISSION_FAILURE` (-11): transaction emission failed.
    pub const EmissionFailure: HookError = of(rshooks_core::EMISSION_FAILURE);
    /// `TOO_MANY_NONCES` (-12): nonce generation limit exceeded.
    pub const TooManyNonces: HookError = of(rshooks_core::TOO_MANY_NONCES);
    /// `TOO_MANY_EMITTED_TXN` (-13): emitted transaction limit exceeded.
    pub const TooManyEmittedTxn: HookError = of(rshooks_core::TOO_MANY_EMITTED_TXN);
    /// `NOT_IMPLEMENTED` (-14): feature not implemented (also what every
    /// host-build stub returns).
    pub const NotImplemented: HookError = of(rshooks_core::NOT_IMPLEMENTED);
    /// `INVALID_ACCOUNT` (-15): account ID is invalid.
    pub const InvalidAccount: HookError = of(rshooks_core::INVALID_ACCOUNT);
    /// `GUARD_VIOLATION` (-16): infinite-loop guard violation.
    pub const GuardViolation: HookError = of(rshooks_core::GUARD_VIOLATION);
    /// `INVALID_FIELD` (-17): field ID is invalid.
    pub const InvalidField: HookError = of(rshooks_core::INVALID_FIELD);
    /// `PARSE_ERROR` (-18): failed to parse data.
    pub const ParseError: HookError = of(rshooks_core::PARSE_ERROR);
    /// `RC_ROLLBACK` (-19): hook execution terminated via rollback.
    pub const RcRollback: HookError = of(rshooks_core::RC_ROLLBACK);
    /// `RC_ACCEPT` (-20): hook execution terminated via accept.
    pub const RcAccept: HookError = of(rshooks_core::RC_ACCEPT);
    /// `NO_SUCH_KEYLET` (-21): keylet not found.
    pub const NoSuchKeylet: HookError = of(rshooks_core::NO_SUCH_KEYLET);
    /// `NOT_AN_ARRAY` (-22): object is not an array.
    pub const NotAnArray: HookError = of(rshooks_core::NOT_AN_ARRAY);
    /// `NOT_AN_OBJECT` (-23): object is not an object.
    pub const NotAnObject: HookError = of(rshooks_core::NOT_AN_OBJECT);
    /// `INVALID_FLOAT` (-10024, verbatim from the header — not -24): XFL
    /// value is invalid.
    pub const InvalidFloat: HookError = of(rshooks_core::INVALID_FLOAT);
    /// `DIVISION_BY_ZERO` (-25): division by zero.
    pub const DivisionByZero: HookError = of(rshooks_core::DIVISION_BY_ZERO);
    /// `MANTISSA_OVERSIZED` (-26): XFL mantissa too large.
    pub const MantissaOversized: HookError = of(rshooks_core::MANTISSA_OVERSIZED);
    /// `MANTISSA_UNDERSIZED` (-27): XFL mantissa too small.
    pub const MantissaUndersized: HookError = of(rshooks_core::MANTISSA_UNDERSIZED);
    /// `EXPONENT_OVERSIZED` (-28): XFL exponent too large.
    pub const ExponentOversized: HookError = of(rshooks_core::EXPONENT_OVERSIZED);
    /// `EXPONENT_UNDERSIZED` (-29): XFL exponent too small.
    pub const ExponentUndersized: HookError = of(rshooks_core::EXPONENT_UNDERSIZED);
    /// `XFL_OVERFLOW` (-30): XFL arithmetic overflow.
    pub const XflOverflow: HookError = of(rshooks_core::XFL_OVERFLOW);
    /// `NOT_IOU_AMOUNT` (-31): not an IOU amount.
    pub const NotIouAmount: HookError = of(rshooks_core::NOT_IOU_AMOUNT);
    /// `NOT_AN_AMOUNT` (-32): not an amount type.
    pub const NotAnAmount: HookError = of(rshooks_core::NOT_AN_AMOUNT);
    /// `CANT_RETURN_NEGATIVE` (-33): cannot return a negative value.
    pub const CantReturnNegative: HookError = of(rshooks_core::CANT_RETURN_NEGATIVE);
    /// `NOT_AUTHORIZED` (-34): no access permission.
    pub const NotAuthorized: HookError = of(rshooks_core::NOT_AUTHORIZED);
    /// `PREVIOUS_FAILURE_PREVENTS_RETRY` (-35): a previous failure prevents
    /// retrying this operation.
    pub const PreviousFailurePreventsRetry: HookError =
        of(rshooks_core::PREVIOUS_FAILURE_PREVENTS_RETRY);
    /// `TOO_MANY_PARAMS` (-36): parameter limit exceeded.
    pub const TooManyParams: HookError = of(rshooks_core::TOO_MANY_PARAMS);
    /// `INVALID_TXN` (-37): transaction is invalid.
    pub const InvalidTxn: HookError = of(rshooks_core::INVALID_TXN);
    /// `RESERVE_INSUFFICIENT` (-38): reserve insufficient for the operation.
    pub const ReserveInsufficient: HookError = of(rshooks_core::RESERVE_INSUFFICIENT);
    /// `COMPLEX_NOT_SUPPORTED` (-39): complex-domain result not supported.
    pub const ComplexNotSupported: HookError = of(rshooks_core::COMPLEX_NOT_SUPPORTED);
    /// `DOES_NOT_MATCH` (-40): values do not match.
    pub const DoesNotMatch: HookError = of(rshooks_core::DOES_NOT_MATCH);
    /// `INVALID_KEY` (-41): key is invalid.
    pub const InvalidKey: HookError = of(rshooks_core::INVALID_KEY);
    /// `NOT_A_STRING` (-42): value is not a string.
    pub const NotAString: HookError = of(rshooks_core::NOT_A_STRING);
    /// `MEM_OVERLAP` (-43): memory regions overlap.
    pub const MemOverlap: HookError = of(rshooks_core::MEM_OVERLAP);
    /// `TOO_MANY_STATE_MODIFICATIONS` (-44): state modification limit
    /// exceeded.
    pub const TooManyStateModifications: HookError = of(rshooks_core::TOO_MANY_STATE_MODIFICATIONS);
    /// `TOO_MANY_NAMESPACES` (-45): namespace limit exceeded.
    pub const TooManyNamespaces: HookError = of(rshooks_core::TOO_MANY_NAMESPACES);
}

impl HookError {
    /// The raw negative `i64` error code this value wraps. Exact inverse of
    /// [`HookError::from`]: `HookError::from(c).code() == c` for every
    /// nonzero `c`.
    #[must_use]
    #[inline(always)]
    pub const fn code(self) -> i64 {
        self.0
    }

    /// The decoded error kind, for exhaustive dispatch. This is the only
    /// place `HookError` performs a decode; it costs one code-to-kind table
    /// lookup (nesting depth 1) and is paid only when called.
    ///
    /// `INVALID_FLOAT` is tested before the table: its value is `-10024`,
    /// not the `-24` its declaration-order position would suggest (kept
    /// verbatim from `rshooks_core::INVALID_FLOAT`). Folding it into the
    /// table instead would mis-map a genuine `-24` error onto
    /// [`HookErrorKind::InvalidFloat`] and stop the table from ever
    /// recognizing the real `-10024` value. `-24` itself is a genuine gap
    /// (maps to [`HookErrorKind::Unknown`]), matching `error.h` upstream.
    #[must_use]
    pub fn kind(self) -> HookErrorKind {
        let code = self.code();
        if code == rshooks_core::INVALID_FLOAT {
            return HookErrorKind::InvalidFloat;
        }
        // Table, not a match over every known code: LLVM lowers a match
        // over this many wide `i64` constants into a deeply nested
        // block-per-arm decision tree, which alone can blow the Guard-type
        // nesting limit once inlined. A table lookup is nesting-depth 1.
        const KINDS: [HookErrorKind; 45] = [
            HookErrorKind::OutOfBounds,
            HookErrorKind::InternalError,
            HookErrorKind::TooBig,
            HookErrorKind::TooSmall,
            HookErrorKind::DoesntExist,
            HookErrorKind::NoFreeSlots,
            HookErrorKind::InvalidArgument,
            HookErrorKind::AlreadySet,
            HookErrorKind::PrerequisiteNotMet,
            HookErrorKind::FeeTooLarge,
            HookErrorKind::EmissionFailure,
            HookErrorKind::TooManyNonces,
            HookErrorKind::TooManyEmittedTxn,
            HookErrorKind::NotImplemented,
            HookErrorKind::InvalidAccount,
            HookErrorKind::GuardViolation,
            HookErrorKind::InvalidField,
            HookErrorKind::ParseError,
            HookErrorKind::RcRollback,
            HookErrorKind::RcAccept,
            HookErrorKind::NoSuchKeylet,
            HookErrorKind::NotAnArray,
            HookErrorKind::NotAnObject,
            HookErrorKind::Unknown, // gap: raw code -24, not InvalidFloat (-10024)
            HookErrorKind::DivisionByZero,
            HookErrorKind::MantissaOversized,
            HookErrorKind::MantissaUndersized,
            HookErrorKind::ExponentOversized,
            HookErrorKind::ExponentUndersized,
            HookErrorKind::XflOverflow,
            HookErrorKind::NotIouAmount,
            HookErrorKind::NotAnAmount,
            HookErrorKind::CantReturnNegative,
            HookErrorKind::NotAuthorized,
            HookErrorKind::PreviousFailurePreventsRetry,
            HookErrorKind::TooManyParams,
            HookErrorKind::InvalidTxn,
            HookErrorKind::ReserveInsufficient,
            HookErrorKind::ComplexNotSupported,
            HookErrorKind::DoesNotMatch,
            HookErrorKind::InvalidKey,
            HookErrorKind::NotAString,
            HookErrorKind::MemOverlap,
            HookErrorKind::TooManyStateModifications,
            HookErrorKind::TooManyNamespaces,
        ];
        let idx = code.wrapping_neg().wrapping_sub(1);
        // `.get` + `unwrap_or`, not `KINDS[..]`: this crate denies
        // `clippy::indexing_slicing` (docs/DESIGN.md §8).
        usize::try_from(idx)
            .ok()
            .and_then(|i| KINDS.get(i))
            .copied()
            .unwrap_or(HookErrorKind::Unknown)
    }
}

impl From<i64> for HookError {
    /// Builds a `HookError` from a raw Hook API return code: the identity.
    /// Non-negative codes are representable and report
    /// [`HookErrorKind::Unknown`] from [`HookError::kind`]; [`res`] only
    /// constructs a `HookError` from negative values.
    fn from(code: i64) -> Self {
        HookError(code)
    }
}

impl core::fmt::Debug for HookError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind() {
            HookErrorKind::Unknown => f.debug_tuple("Unknown").field(&self.code()).finish(),
            kind => write!(f, "{kind:?}"),
        }
    }
}

/// The decoded kind of a [`HookError`]: one variant per named code in
/// `hook/error.h`, plus [`HookErrorKind::Unknown`] for any code without a
/// named constant (positive, the `-24` gap, or below `-45`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HookErrorKind {
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
    /// Any code without a named constant (positive, `-24`, below `-45`, …).
    Unknown,
}

/// The result type every rshooks wrapper returns: `Ok(payload)` for a
/// non-negative Hook API return, `Err(HookError)` for a negative one.
pub type Result<T> = core::result::Result<T, HookError>;

/// Funnel point every wrapper in `api/*.rs` and `xfl.rs` calls through:
/// convert a raw Hook API `i64` return into a [`Result<i64>`] by sign. This
/// is the identity on the negative branch: no decode.
#[inline(always)]
pub(crate) fn res(code: i64) -> Result<i64> {
    if code < 0 {
        Err(HookError(code))
    } else {
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: &[(i64, HookError, HookErrorKind)] = &[
        (
            rshooks_core::OUT_OF_BOUNDS,
            HookError::OutOfBounds,
            HookErrorKind::OutOfBounds,
        ),
        (
            rshooks_core::INTERNAL_ERROR,
            HookError::InternalError,
            HookErrorKind::InternalError,
        ),
        (
            rshooks_core::TOO_BIG,
            HookError::TooBig,
            HookErrorKind::TooBig,
        ),
        (
            rshooks_core::TOO_SMALL,
            HookError::TooSmall,
            HookErrorKind::TooSmall,
        ),
        (
            rshooks_core::DOESNT_EXIST,
            HookError::DoesntExist,
            HookErrorKind::DoesntExist,
        ),
        (
            rshooks_core::NO_FREE_SLOTS,
            HookError::NoFreeSlots,
            HookErrorKind::NoFreeSlots,
        ),
        (
            rshooks_core::INVALID_ARGUMENT,
            HookError::InvalidArgument,
            HookErrorKind::InvalidArgument,
        ),
        (
            rshooks_core::ALREADY_SET,
            HookError::AlreadySet,
            HookErrorKind::AlreadySet,
        ),
        (
            rshooks_core::PREREQUISITE_NOT_MET,
            HookError::PrerequisiteNotMet,
            HookErrorKind::PrerequisiteNotMet,
        ),
        (
            rshooks_core::FEE_TOO_LARGE,
            HookError::FeeTooLarge,
            HookErrorKind::FeeTooLarge,
        ),
        (
            rshooks_core::EMISSION_FAILURE,
            HookError::EmissionFailure,
            HookErrorKind::EmissionFailure,
        ),
        (
            rshooks_core::TOO_MANY_NONCES,
            HookError::TooManyNonces,
            HookErrorKind::TooManyNonces,
        ),
        (
            rshooks_core::TOO_MANY_EMITTED_TXN,
            HookError::TooManyEmittedTxn,
            HookErrorKind::TooManyEmittedTxn,
        ),
        (
            rshooks_core::NOT_IMPLEMENTED,
            HookError::NotImplemented,
            HookErrorKind::NotImplemented,
        ),
        (
            rshooks_core::INVALID_ACCOUNT,
            HookError::InvalidAccount,
            HookErrorKind::InvalidAccount,
        ),
        (
            rshooks_core::GUARD_VIOLATION,
            HookError::GuardViolation,
            HookErrorKind::GuardViolation,
        ),
        (
            rshooks_core::INVALID_FIELD,
            HookError::InvalidField,
            HookErrorKind::InvalidField,
        ),
        (
            rshooks_core::PARSE_ERROR,
            HookError::ParseError,
            HookErrorKind::ParseError,
        ),
        (
            rshooks_core::RC_ROLLBACK,
            HookError::RcRollback,
            HookErrorKind::RcRollback,
        ),
        (
            rshooks_core::RC_ACCEPT,
            HookError::RcAccept,
            HookErrorKind::RcAccept,
        ),
        (
            rshooks_core::NO_SUCH_KEYLET,
            HookError::NoSuchKeylet,
            HookErrorKind::NoSuchKeylet,
        ),
        (
            rshooks_core::NOT_AN_ARRAY,
            HookError::NotAnArray,
            HookErrorKind::NotAnArray,
        ),
        (
            rshooks_core::NOT_AN_OBJECT,
            HookError::NotAnObject,
            HookErrorKind::NotAnObject,
        ),
        (
            rshooks_core::INVALID_FLOAT,
            HookError::InvalidFloat,
            HookErrorKind::InvalidFloat,
        ),
        (
            rshooks_core::DIVISION_BY_ZERO,
            HookError::DivisionByZero,
            HookErrorKind::DivisionByZero,
        ),
        (
            rshooks_core::MANTISSA_OVERSIZED,
            HookError::MantissaOversized,
            HookErrorKind::MantissaOversized,
        ),
        (
            rshooks_core::MANTISSA_UNDERSIZED,
            HookError::MantissaUndersized,
            HookErrorKind::MantissaUndersized,
        ),
        (
            rshooks_core::EXPONENT_OVERSIZED,
            HookError::ExponentOversized,
            HookErrorKind::ExponentOversized,
        ),
        (
            rshooks_core::EXPONENT_UNDERSIZED,
            HookError::ExponentUndersized,
            HookErrorKind::ExponentUndersized,
        ),
        (
            rshooks_core::XFL_OVERFLOW,
            HookError::XflOverflow,
            HookErrorKind::XflOverflow,
        ),
        (
            rshooks_core::NOT_IOU_AMOUNT,
            HookError::NotIouAmount,
            HookErrorKind::NotIouAmount,
        ),
        (
            rshooks_core::NOT_AN_AMOUNT,
            HookError::NotAnAmount,
            HookErrorKind::NotAnAmount,
        ),
        (
            rshooks_core::CANT_RETURN_NEGATIVE,
            HookError::CantReturnNegative,
            HookErrorKind::CantReturnNegative,
        ),
        (
            rshooks_core::NOT_AUTHORIZED,
            HookError::NotAuthorized,
            HookErrorKind::NotAuthorized,
        ),
        (
            rshooks_core::PREVIOUS_FAILURE_PREVENTS_RETRY,
            HookError::PreviousFailurePreventsRetry,
            HookErrorKind::PreviousFailurePreventsRetry,
        ),
        (
            rshooks_core::TOO_MANY_PARAMS,
            HookError::TooManyParams,
            HookErrorKind::TooManyParams,
        ),
        (
            rshooks_core::INVALID_TXN,
            HookError::InvalidTxn,
            HookErrorKind::InvalidTxn,
        ),
        (
            rshooks_core::RESERVE_INSUFFICIENT,
            HookError::ReserveInsufficient,
            HookErrorKind::ReserveInsufficient,
        ),
        (
            rshooks_core::COMPLEX_NOT_SUPPORTED,
            HookError::ComplexNotSupported,
            HookErrorKind::ComplexNotSupported,
        ),
        (
            rshooks_core::DOES_NOT_MATCH,
            HookError::DoesNotMatch,
            HookErrorKind::DoesNotMatch,
        ),
        (
            rshooks_core::INVALID_KEY,
            HookError::InvalidKey,
            HookErrorKind::InvalidKey,
        ),
        (
            rshooks_core::NOT_A_STRING,
            HookError::NotAString,
            HookErrorKind::NotAString,
        ),
        (
            rshooks_core::MEM_OVERLAP,
            HookError::MemOverlap,
            HookErrorKind::MemOverlap,
        ),
        (
            rshooks_core::TOO_MANY_STATE_MODIFICATIONS,
            HookError::TooManyStateModifications,
            HookErrorKind::TooManyStateModifications,
        ),
        (
            rshooks_core::TOO_MANY_NAMESPACES,
            HookError::TooManyNamespaces,
            HookErrorKind::TooManyNamespaces,
        ),
    ];

    #[test]
    fn round_trips_every_known_code() {
        for &(code, _, _) in KNOWN {
            assert_eq!(
                HookError::from(code).code(),
                code,
                "round-trip failed for {code}"
            );
        }
    }

    #[test]
    fn every_known_code_maps_to_its_named_constant() {
        for &(code, expected, _) in KNOWN {
            assert_eq!(HookError::from(code), expected, "wrong constant for {code}");
        }
    }

    #[test]
    fn every_known_code_reports_its_kind() {
        for &(code, _, kind) in KNOWN {
            assert_eq!(HookError::from(code).kind(), kind, "wrong kind for {code}");
        }
    }

    #[test]
    fn invalid_float_is_irregular() {
        assert_eq!(HookError::from(-10024), HookError::InvalidFloat);
        assert_eq!(HookError::InvalidFloat.code(), -10024);
        assert_eq!(HookError::InvalidFloat.kind(), HookErrorKind::InvalidFloat);
        let gap = HookError::from(-24);
        assert_eq!(gap.code(), -24);
        assert_eq!(gap.kind(), HookErrorKind::Unknown);
    }

    #[test]
    fn out_of_range_codes_round_trip_as_unknown() {
        for code in [i64::MIN, -46, 0, 1, i64::MAX] {
            let err = HookError::from(code);
            assert_eq!(err.code(), code);
            assert_eq!(err.kind(), HookErrorKind::Unknown);
        }
    }

    #[test]
    fn debug_prints_the_constant_name() {
        extern crate std;
        use std::format;

        assert_eq!(format!("{:?}", HookError::DoesntExist), "DoesntExist");
        assert_eq!(format!("{:?}", HookError::from(-9999)), "Unknown(-9999)");
    }

    #[test]
    fn result_is_a_scalar_pair() {
        assert_eq!(core::mem::size_of::<HookError>(), 8);
        assert_eq!(
            core::mem::size_of::<core::result::Result<i64, HookError>>(),
            16
        );
    }

    #[test]
    fn res_splits_on_sign() {
        assert_eq!(res(0), Ok(0));
        assert_eq!(res(32), Ok(32));
        assert_eq!(res(-5), Err(HookError::DoesntExist));
    }
}
