//! Information about the originating transaction (the transaction that
//! triggered this hook invocation).
//!
//! # The typed path is the norm for a field with a modeled type
//!
//! [`otxn_field_typed`] is the default way to read a field the generated
//! [`crate::sfield`] table gives a value type to (`sfAccount`, `sfSequence`,
//! `sfAmount`, ...): the field constant itself pins down what comes back, no
//! turbofish and no separate decode step. [`otxn_field`] (raw bytes) and
//! [`otxn_field_exact`] (fixed-length, `T`-inferred) remain the escape
//! hatches — for a field this crate models no typed read for
//! ([`crate::types::Opaque`], `STObject`, `STArray`), or when the caller
//! wants the wire bytes directly.
//!
//! # Decoding a raw protocol field: use `from_be_bytes`, not `FromBytes`
//!
//! [`otxn_field_exact`]'s raw bytes are Xahau Binary — the protocol's own
//! **big-endian** wire format (see DESIGN.md §5.6, "Endianness
//! conventions") — never this crate's little-endian [`crate::convert::FromBytes`]
//! trait, which is the convention for *hook-private* data (state/param
//! values this crate's own typed layer wrote), a completely different
//! domain. A numeric protocol field read through the raw escapes must be
//! decoded with an explicit `u64::from_be_bytes(...)` call at the use site
//! (`examples/03_hook-params/src/lib.rs`'s `hook_param_exact` read of `MIN`
//! is exactly that). [`otxn_field_typed`] does this decoding itself for
//! every field it models, so a field with a modeled type no longer needs the
//! idiom at all — see its doc comment.

use crate::convert::{FixedRead, TypedParamName};
use crate::error::{HookError, Result, res};
use crate::slot_obj::{self, AmountBytes, IssueData};
use crate::tx_type::TxType;
use crate::types::{AccountId, Amount, CurrencyCode, Hash, Issue, SField};

/// Burden of the originating transaction: `1` for a normal transaction, or
/// the `sfEmitBurden` value for an emitted transaction. A natural (unsigned)
/// magnitude despite the Hook API's `i64` wire type.
#[inline(always)]
pub fn otxn_burden() -> u64 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.otxn_burden()) {
        return v as u64;
    }
    unsafe { rshooks_core::otxn_burden() as u64 }
}

/// Read a field from the originating transaction into `out`. Returns the
/// number of bytes written.
#[inline(always)]
pub fn otxn_field<B: AsMut<[u8]> + ?Sized>(out: &mut B, field_id: impl Into<u32>) -> Result<usize> {
    let out = out.as_mut();
    let field_id = field_id.into();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.otxn_field(field_id)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe { rshooks_core::otxn_field(out.as_mut_ptr() as u32, out.len() as u32, field_id) })
        .map(|v| v as usize)
}

/// Read a field from the originating transaction as a big-endian `u64`
/// ("as-int64" mode: `write_ptr = 0, write_len = 0`; only for fields of at
/// most 8 bytes with the top bit clear, else
/// [`crate::error::HookError::TooBig`] — see `state_u64` for details).
#[inline(always)]
pub fn otxn_field_u64(field_id: impl Into<u32>) -> Result<u64> {
    let field_id = field_id.into();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.otxn_field(field_id)) {
        return res(crate::testenv_bridge::as_int64_code(r)).map(|v| v as u64);
    }
    res(unsafe { rshooks_core::otxn_field(0, 0, field_id) }).map(|v| v as u64)
}

/// Read field `field_id` from the originating transaction, requiring it to
/// be exactly `T`'s length — any [`crate::convert::FixedRead`] type, most
/// commonly a `rshooks::types` newtype or a raw `[u8; N]`. A field longer
/// than that already fails as [`crate::error::HookError::TooSmall`] from the
/// underlying host call (the buffer `T::read_exact` allocates has exactly
/// that capacity); a field shorter is caught by `T::read_exact` itself and
/// mapped to the same variant — see `state_exact` (`state.rs`) for the
/// identical pattern and rationale. No loop, no panic.
///
/// `T` is inferred from context (a `let` binding's type annotation, a
/// function's declared return type, ...), not a turbofish — e.g.
/// `let sender: AccountId = otxn_field_exact(sfAccount)?;`. A call site
/// with no way to infer `T` is a compile error; annotate it (or use
/// `otxn_field_exact::<AccountId>(field_id)`/`::<[u8; 20]>(field_id)`
/// explicitly).
///
/// # Examples
///
/// ```
/// use rshooks::api::otxn::otxn_field_exact;
/// use rshooks::error::{HookError, Result};
///
/// let sender: Result<[u8; 20]> = otxn_field_exact(0u32);
/// assert_eq!(sender, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn otxn_field_exact<T: FixedRead>(field_id: impl Into<u32>) -> Result<T> {
    T::read_exact(|buf| otxn_field(buf, field_id))
}

/// Sealing module: see [`crate::slot_obj`]'s `private` module for the
/// identical rationale, applied here to the same end — a downstream impl of
/// [`OtxnFieldValue`] could otherwise claim a wire-type/Rust-type pairing
/// this crate never verified (e.g. reading a `Blob` field as an
/// `AccountId`), and the generated [`crate::sfield`] table is the only thing
/// that is supposed to make that pairing.
mod private {
    pub trait Sealed {}
}

/// Value types readable directly off the originating transaction by
/// [`otxn_field_typed`] — the argument-driven counterpart of
/// [`crate::slot_obj::SlotObject::value`]'s per-type reads, with identical
/// per-type semantics (narrow-int as-int64, `u64`/`Hash`/`AccountId`/
/// `CurrencyCode` as exact wire bytes, `Amount`/`Issue` classified by
/// length) — see that impl family's doc comments for the width/endianness
/// rationale each impl here mirrors.
///
/// [`Self::Output`](OtxnFieldValue::Output) is `Self` for every scalar and
/// fixed-byte type, and the classified [`AmountBytes`]/[`IssueData`] for
/// `Amount`/`Issue`, whose wire encoding is one of two shapes rather than
/// one. **Sealed** (see [`private`]) — implemented for exactly the value
/// types [`crate::sfield`] pairs with a field constant. `STObject`,
/// `STArray`, and [`crate::types::Opaque`] have deliberately no impl: an
/// object or array has no single scalar value, and an opaque field's shape
/// is not known at compile time, so a call naming one of those is a compile
/// error — read it with the raw [`otxn_field`]/[`otxn_field_exact`] escape
/// hatches instead.
pub trait OtxnFieldValue: private::Sealed + Sized {
    /// What the read returns.
    type Output;

    /// Reads `field` and decodes it as [`Self::Output`](Self::Output).
    /// Called by [`otxn_field_typed`]; takes the [`SField`] itself rather
    /// than its raw `u32` code so that calling this directly buys nothing —
    /// an `SField<Self>` can only come from the generated [`crate::sfield`]
    /// table ([`SField`]'s constructor is `pub(crate)` precisely to keep
    /// code/type pairings unforgeable), so this is [`otxn_field_typed`]
    /// under another name, not a bypass around it.
    #[doc(hidden)]
    fn read_otxn_field(field: SField<Self>) -> Result<Self::Output>;
}

/// Generates the [`OtxnFieldValue`] impl for a narrow integer type, read
/// through the host's as-int64 mode (bit 63 is unreachable at these widths,
/// so [`otxn_field_u64`] is safe here — see `slot_obj::read_int` for the
/// identical narrowing).
macro_rules! int_field {
    ($ty:ty) => {
        impl private::Sealed for $ty {}
        impl OtxnFieldValue for $ty {
            type Output = $ty;

            #[inline(always)]
            fn read_otxn_field(field: SField<Self>) -> Result<Self::Output> {
                let raw = otxn_field_u64(field.code())?;
                <$ty>::try_from(raw).map_err(|_| HookError::TooBig)
            }
        }
    };
}

int_field!(u8);
int_field!(u16);
int_field!(u32);

impl private::Sealed for u64 {}
impl OtxnFieldValue for u64 {
    type Output = u64;

    /// Deliberately **not** the as-int64 path the narrower integers use: the
    /// host rejects a value with bit 63 set as `TOO_BIG`, and legitimate
    /// 64-bit fields set it (`sfExchangeRate` among them). Reading the eight
    /// wire bytes and decoding them big-endian here has no such hole — see
    /// `SlotObject<u64>::value`'s doc comment for the identical rationale.
    #[inline(always)]
    fn read_otxn_field(field: SField<Self>) -> Result<Self::Output> {
        otxn_field_exact::<[u8; 8]>(field.code()).map(u64::from_be_bytes)
    }
}

/// Generates the [`OtxnFieldValue`] impl for a fixed-size wire type read
/// verbatim (no interpretation, no byte-swapping) via [`otxn_field_exact`].
macro_rules! bytes_field {
    ($ty:ty) => {
        impl private::Sealed for $ty {}
        impl OtxnFieldValue for $ty {
            type Output = $ty;

            #[inline(always)]
            fn read_otxn_field(field: SField<Self>) -> Result<Self::Output> {
                otxn_field_exact::<Self>(field.code())
            }
        }
    };
}

bytes_field!(Hash);
bytes_field!(AccountId);
bytes_field!(CurrencyCode);

impl private::Sealed for Amount {}
impl OtxnFieldValue for Amount {
    type Output = AmountBytes;

    #[inline(always)]
    fn read_otxn_field(field: SField<Self>) -> Result<Self::Output> {
        let mut buf = [0u8; crate::types::IOU_AMOUNT_LEN];
        let written = otxn_field(&mut buf, field.code())?;
        let bytes = buf.get(..written).ok_or(HookError::TooBig)?;
        slot_obj::classify_amount(bytes)
    }
}

impl private::Sealed for Issue {}
impl OtxnFieldValue for Issue {
    type Output = IssueData;

    /// 44 bytes, not the 40 an IOU issue needs — see
    /// `slot_obj::decode_issue`'s doc comment for why the wider buffer is
    /// what makes a 44-byte MPT issue reach [`slot_obj::classify_issue`] as a
    /// [`HookError::ParseError`] instead of failing the host call first as
    /// `TooSmall`.
    #[inline(always)]
    fn read_otxn_field(field: SField<Self>) -> Result<Self::Output> {
        let mut buf = [0u8; slot_obj::ISSUE_MAX_READ_LEN];
        let written = otxn_field(&mut buf, field.code())?;
        let bytes = buf.get(..written).ok_or(HookError::TooBig)?;
        slot_obj::classify_issue(bytes)
    }
}

/// Reads a field from the originating transaction as its modeled Rust type —
/// the safer default over [`otxn_field_exact`] for exactly the reason
/// [`otxn_param_typed`]'s doc comment gives for parameters: `field`'s own
/// type parameter is what decides [`Self::Output`](OtxnFieldValue::Output),
/// so there is no separate type argument spelled independently of the
/// constant that could name a mismatched type for the field actually
/// intended. No turbofish, no annotation — `otxn_field_typed(sfAccount)`
/// reads back an `AccountId` because `sfAccount: SField<AccountId>` says so.
///
/// Escape hatches: [`otxn_field`] (raw bytes, any field) and
/// [`otxn_field_exact`] (fixed-length, `T` inferred from context) for a
/// field this crate models no typed read for, or when the caller wants the
/// wire bytes directly.
///
/// # Examples
///
/// ```
/// use rshooks::prelude::*;
///
/// let sender = otxn_field_typed(sfAccount);
/// assert_eq!(sender, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn otxn_field_typed<T: OtxnFieldValue>(field: SField<T>) -> Result<T::Output> {
    T::read_otxn_field(field)
}

/// Generation of the originating transaction: `0` for a normal transaction,
/// or the `sfEmitGeneration` value for an emitted transaction.
#[inline(always)]
pub fn otxn_generation() -> u32 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.otxn_generation()) {
        return v as u32;
    }
    unsafe { rshooks_core::otxn_generation() as u32 }
}

/// The ID (hash) of the originating transaction, written into `out`.
/// Returns the number of bytes written. [`otxn_id_buf`] is the fixed-size
/// convenience twin.
#[inline(always)]
pub fn otxn_id<B: AsMut<[u8]> + ?Sized>(out: &mut B, flags: u32) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.otxn_id(flags)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe { rshooks_core::otxn_id(out.as_mut_ptr() as u32, out.len() as u32, flags) })
        .map(|v| v as usize)
}

/// The ID (hash) of the originating transaction. `flags = 0` prefers the
/// emit-failure transaction ID where applicable; other flag values are
/// passed through verbatim (undocumented beyond that in the upstream Hook
/// API reference, so exposed as a plain `u32` rather than an invented enum).
#[inline(always)]
pub fn otxn_id_buf(flags: u32) -> Result<Hash> {
    let mut buf = Hash::default();
    let _ = otxn_id(buf.as_mut(), flags)?;
    Ok(buf)
}

/// The [`TxType`] of the originating transaction.
#[inline(always)]
pub fn otxn_type() -> TxType {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.otxn_type()) {
        return TxType::from(v as u16);
    }
    TxType::from(unsafe { rshooks_core::otxn_type() as u16 })
}

/// Load the originating transaction into a slot. `slot_into = 0` auto-assigns
/// a slot. Returns the assigned slot number.
#[inline(always)]
pub fn otxn_slot(slot_into: u32) -> Result<u32> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.otxn_slot(slot_into)) {
        return res(v).map(|v| v as u32);
    }
    res(unsafe { rshooks_core::otxn_slot(slot_into) }).map(|v| v as u32)
}

/// Read a Hook parameter attached to the originating transaction into `out`.
/// Returns the number of bytes written.
#[inline(always)]
pub fn otxn_param<B: AsMut<[u8]> + ?Sized>(out: &mut B, name: &[u8]) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.otxn_param(name)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe {
        rshooks_core::otxn_param(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            name.as_ptr() as u32,
            name.len() as u32,
        )
    })
    .map(|v| v as usize)
}

/// Read a Hook parameter attached to the originating transaction, requiring
/// it to be exactly `T`'s length — any [`crate::convert::FixedRead`] type,
/// most commonly a `rshooks::types` newtype, a raw `[u8; N]`, or a
/// [`crate::ParamValue`]-derived struct. A parameter longer than that already
/// fails as [`crate::error::HookError::TooSmall`] from the underlying host
/// call; a parameter shorter is caught by `T::read_exact` itself and mapped
/// to the same variant — see [`otxn_field_exact`]/`state_exact` (`state.rs`)
/// for the identical pattern and rationale. No loop, no panic.
///
/// `T` is inferred from context, not a turbofish — see
/// [`otxn_field_exact`]'s doc comment for the full story.
///
/// # Examples
///
/// ```
/// use rshooks::api::otxn::otxn_param_exact;
/// use rshooks::error::{HookError, Result};
///
/// let value: Result<[u8; 4]> = otxn_param_exact(b"CFG");
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn otxn_param_exact<T: FixedRead>(name: &[u8]) -> Result<T> {
    T::read_exact(|buf| otxn_param(buf, name))
}

/// Calls the host `otxn_param` function directly and returns its
/// **undecoded** `i64` result — no [`res`] applied. `#[inline(always)]` and
/// `pub(crate)`: an internal fast path for [`otxn_param_opt`] — see
/// [`crate::api::hook_ctx::hook_param_raw_code`]'s doc comment for the
/// identical rationale (including why this deliberately duplicates
/// [`otxn_param`]'s own call rather than routing through it).
#[inline(always)]
pub(crate) fn otxn_param_raw_code(buf: &mut [u8], name: &[u8]) -> i64 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.otxn_param(name)) {
        return crate::testenv_bridge::write_bytes_code(buf, r);
    }
    unsafe {
        rshooks_core::otxn_param(
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            name.as_ptr() as u32,
            name.len() as u32,
        )
    }
}

/// Read a Hook parameter attached to the originating transaction,
/// distinguishing "parameter is absent" from every other outcome — the
/// absence-aware counterpart to [`otxn_param_exact`], used by
/// [`crate::decl`]'s `OtxnParam`/`OtxnParamAt` accessors. See
/// [`crate::api::hook_ctx::hook_param_opt`]'s doc comment for the exact
/// absence-vs-decode-failure contract this mirrors.
///
/// # Examples
///
/// ```
/// use rshooks::api::otxn::otxn_param_opt;
/// use rshooks::error::{HookError, Result};
///
/// // Host stub: every Hook API call returns `NotImplemented` on a host
/// // build, so this only proves the call chain compiles and runs (it is
/// // not the `DOESNT_EXIST` sentinel `otxn_param_opt` maps to `Ok(None)`).
/// let value: Result<Option<[u8; 4]>> = otxn_param_opt(b"x");
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn otxn_param_opt<T: FixedRead>(name: &[u8]) -> Result<Option<T>> {
    let mut absent = false;
    let r = T::read_exact(|buf| {
        let code = otxn_param_raw_code(buf, name);
        if code == rshooks_core::DOESNT_EXIST {
            absent = true;
            return Ok(0);
        }
        res(code).map(|v| v as usize)
    });
    if absent {
        return Ok(None);
    }
    r.map(Some)
}

/// Read a Hook parameter attached to the originating transaction, named by
/// `name` itself — see [`crate::convert::TypedParamName`]'s doc comment
/// for why this is the safer alternative to [`otxn_param_exact`] when a
/// parameter is always meant to decode as `name`'s one paired value type:
/// there is no separate `name` argument spelled independently of the type
/// that could name a *different* parameter than the one actually
/// intended. `N::Value` (the return type) is inferred from `name`'s own
/// type — no turbofish.
///
/// Costs nothing beyond [`otxn_param_exact`] for the common
/// plain-byte-string-name case (e.g. a hand-written
/// [`crate::convert::TypedParamName`] impl overriding `with_name_bytes` to
/// hand back a `'static` literal) — see
/// [`crate::convert::TypedParamName`]'s "Zero-cost" section. A
/// **composite, struct-shaped** name costs a small, genuine runtime encode
/// instead (unavoidable for an arbitrary type) — see the same doc comment.
///
/// # Examples
///
/// ```
/// use rshooks::api::otxn::otxn_param_typed;
/// use rshooks::convert::TypedParamName;
/// use rshooks::error::{HookError, Result};
///
/// struct Instruction([u8; 9]);
///
/// impl rshooks::convert::FixedRead for Instruction {
///     fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
///         <[u8; 9]>::read_exact(read).map(Instruction)
///     }
/// }
///
/// struct InsName;
///
/// impl rshooks::convert::ToBytes for InsName {
///     const MAX_LEN: usize = 3;
///     fn write(&self, buf: &mut [u8]) -> usize {
///         rshooks::convert::ToBytes::write(b"INS", buf)
///     }
/// }
///
/// impl TypedParamName for InsName {
///     type Value = Instruction;
/// }
///
/// let value = otxn_param_typed(&InsName);
/// assert_eq!(value.err(), Some(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn otxn_param_typed<N: TypedParamName>(name: &N) -> Result<N::Value> {
    name.with_name_bytes(otxn_param_exact::<N::Value>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        assert_eq!(otxn_burden(), rshooks_core::NOT_IMPLEMENTED as u64);
        assert_eq!(otxn_generation(), rshooks_core::NOT_IMPLEMENTED as u32);
        // The host stub's `NOT_IMPLEMENTED` (a negative i64) doesn't match
        // any real `tt*` code once truncated to `u16`, so this decodes to
        // `TxType::Unknown` rather than a specific known variant.
        assert_eq!(
            otxn_type(),
            TxType::Unknown(rshooks_core::NOT_IMPLEMENTED as u16)
        );
        assert_eq!(otxn_slot(0), Err(HookError::NotImplemented));
        assert_eq!(otxn_id_buf(0), Err(HookError::NotImplemented));
        let mut buf = [0u8; 32];
        assert_eq!(otxn_id(&mut buf, 0), Err(HookError::NotImplemented));
        assert_eq!(otxn_field(&mut buf, 0u32), Err(HookError::NotImplemented));
        assert_eq!(otxn_field_u64(0u32), Err(HookError::NotImplemented));
        assert_eq!(
            otxn_field_exact::<[u8; 20]>(0u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_field_typed(crate::sfield::sfSequence),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_field_typed(crate::sfield::sfAccount),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_field_typed(crate::sfield::sfEmitParentTxnID),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_field_typed(crate::sfield::sfAmount),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_field_typed(crate::sfield::sfExchangeRate),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_field_typed(crate::sfield::sfLockingChainIssue),
            Err(HookError::NotImplemented)
        );
        assert_eq!(otxn_param(&mut buf, b"x"), Err(HookError::NotImplemented));
        assert_eq!(
            otxn_param_exact::<[u8; 4]>(b"x"),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_param_opt::<[u8; 4]>(b"x"),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_param_typed(&TestParamName),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            otxn_param_typed(&TestKeyParamName { tag: 1 }),
            Err(HookError::NotImplemented)
        );
    }

    // Simple/static form: `Name` is a marker type, `with_name_bytes`
    // overridden to skip the default (encoding) body entirely.
    #[derive(Debug, PartialEq)]
    struct TestParam([u8; 4]);

    impl FixedRead for TestParam {
        fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
            <[u8; 4]>::read_exact(read).map(TestParam)
        }
    }

    struct TestParamName;

    impl crate::convert::ToBytes for TestParamName {
        const MAX_LEN: usize = 1;

        fn write(&self, buf: &mut [u8]) -> usize {
            crate::convert::ToBytes::write(b"x", buf)
        }
    }

    impl crate::convert::TypedParamName for TestParamName {
        type Value = TestParam;

        fn with_name_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
            f(b"x")
        }
    }

    // Composite form: relies on `TypedParamName::with_name_bytes`'s
    // default body (the genuine runtime encode, exercised here with a
    // trivial one-field name).
    #[derive(Debug, PartialEq)]
    struct TestKeyParam([u8; 4]);

    impl FixedRead for TestKeyParam {
        fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
            <[u8; 4]>::read_exact(read).map(TestKeyParam)
        }
    }

    struct TestKeyParamName {
        tag: u8,
    }

    impl crate::convert::ToBytes for TestKeyParamName {
        const MAX_LEN: usize = 1;

        fn write(&self, buf: &mut [u8]) -> usize {
            crate::convert::ToBytes::write(&self.tag, buf)
        }
    }

    impl crate::convert::TypedParamName for TestKeyParamName {
        type Value = TestKeyParam;
    }
}
