//! The Hook Parameter Signature Interface: [`SigParamType`] and the
//! low-level name-building escape hatch.
//!
//! See `docs/PARAM_SIGNATURE_DESIGN.md` for the full design. The interface
//! draft defines a `HookParameterName` wire convention that turns a Hook's
//! declared parameters into a machine-readable, typed function signature:
//!
//! ```text
//! HookParameterName = 0x5F 0x50 0x53        ; "_PS" interface identifier
//!                   | 0x00                  ; version
//!                   | index    (1 byte, 0x00..=0x0F, raw binary)
//!                   | type     (1 byte, an STI_* code, raw binary)
//!                   | name_len (1 byte, 0x01..=0x10 = name.len())
//!                   | name     (1..=16 bytes, [A-Za-z][A-Za-z0-9]*)
//! ```
//!
//! Total 8..=23 octets — well under [`crate::convert::PARAM_NAME_MAX_LEN`]
//! (32), so no separate bound check against that constant is needed here;
//! it falls out of the `index`/`name` bounds this module already enforces.
//!
//! Two independent layers:
//!
//! - [`SigParamType`]: pairs a Rust type with its `STI_*` type byte and
//!   invocation-value decode, mirroring [`crate::convert::FixedRead`]'s
//!   closure-based shape but decoding integers **big-endian** (see "Why
//!   big-endian" below), unlike this crate's hook-private little-endian
//!   `FromBytes` convention.
//! - [`sig_param_name`]/[`sig_name!`](crate::sig_name): builds a declared
//!   name at compile time, with every MUST of the wire format checked as a
//!   `const`-evaluable assert, so a malformed name is a compile error
//!   rather than a wrong runtime read.
//!
//! # Why big-endian
//!
//! A signature parameter's value crosses the same protocol boundary a raw
//! `otxn_field`/`otxn_param` read does — see DESIGN.md §5.6 ("Endianness
//! conventions"): Xahau Binary integers are big-endian. This crate's own
//! [`crate::convert::FromBytes`] little-endian convention only applies to
//! state/param values this crate's own typed layer wrote — a different
//! domain. [`SigParamType`]'s integer impls decode big-endian for the same
//! reason [`crate::api::otxn::otxn_field_typed`]'s `u64` impl does: the
//! invocation `HookParameterValue` was put on the wire by something outside
//! this crate's control (the transaction submitter), so it is Xahau Binary.
//!
//! # `SigName<V>`: read helpers, not a [`crate::convert::TypedParamName`] impl
//!
//! [`crate::convert::TypedParamName::Value`] requires
//! [`crate::convert::FixedRead`], but [`SigParamType`] decoding uses its own
//! [`SigParamType::read_sig`] — `u8`/`u16`/`u32`/`u64` have no `FixedRead`
//! impl at all (they're hook-private little-endian types there), so
//! `SigName<V>` cannot honestly implement `TypedParamName<Value = V>` for
//! every `V: SigParamType`. Instead [`SigName`] carries its prebuilt name
//! bytes and exposes [`SigName::otxn_read`]/[`SigName::hook_read`] directly
//! — the same "resolve the value type from the name" ergonomics without
//! requiring the trait. [`otxn_sig_param`]/[`hook_sig_param`] are the
//! plain, name-as-`&[u8]` counterparts for a caller that would rather not
//! build a `SigName` at all.

use core::marker::PhantomData;

use crate::api::hook_ctx::{hook_param, hook_param_raw_code};
use crate::api::otxn::{otxn_param, otxn_param_raw_code};
use crate::buf_eq::buf_eq_20;
use crate::convert::FixedRead;
use crate::error::{HookError, Result, res};
use crate::slot_obj::{AmountBytes, ISSUE_MAX_READ_LEN, classify_amount};
use crate::types::{ACC_ID_LEN, AccountId, CURRENCY_CODE_LEN, CurrencyCode, Hash, IssuedAsset};

// ---------------------------------------------------------------------------
// Wire-format constants and the const-fn name builder
// ---------------------------------------------------------------------------

/// The identifier, version, `index`/`type` octets, and name-length byte
/// every declared name carries, before the variable-length display name:
/// `0x5F 0x50 0x53 | 0x00 | index | type | name_len` is 7 bytes.
const FIXED_LEN: usize = 7;

/// The twelve `STI_*` type bytes [`SigParamType`] has an impl for — see
/// `docs/PARAM_SIGNATURE_DESIGN.md` §2's table.
#[inline(always)]
const fn is_supported_type_byte(b: u8) -> bool {
    matches!(
        b,
        0x01 // STI_UINT16 (u16)
        | 0x02 // STI_UINT32 (u32)
        | 0x03 // STI_UINT64 (u64)
        | 0x04 // STI_UINT128 ([u8; 16])
        | 0x05 // STI_UINT256 ([u8; 32] / Hash)
        | 0x06 // STI_AMOUNT (AmountBytes)
        | 0x07 // STI_VL (Blob<N>)
        | 0x08 // STI_ACCOUNT (AccountId)
        | 0x10 // STI_UINT8 (u8)
        | 0x11 // STI_UINT160 ([u8; 20])
        | 0x18 // STI_ISSUE (IssueBytes)
        | 0x1A // STI_CURRENCY (CurrencyCode)
    )
}

/// Whether `name` matches the interface draft's charset:
/// `[A-Za-z][A-Za-z0-9]*`, 1..=16 bytes. Every caller is `const { .. }`
/// -evaluated, so this never compiles into hook wasm — it only runs during
/// `rustc`'s own const evaluator.
#[allow(clippy::indexing_slicing)] // in-bounds by the `i < name.len()` loop condition, const-evaluated only
const fn is_valid_name(name: &[u8]) -> bool {
    if name.is_empty() || name.len() > 16 {
        return false;
    }
    let mut i = 0;
    while i < name.len() {
        let b = name[i];
        let ok = if i == 0 {
            b.is_ascii_alphabetic()
        } else {
            b.is_ascii_alphanumeric()
        };
        if !ok {
            return false;
        }
        i = i.wrapping_add(1);
    }
    true
}

/// Builds one declared `HookParameterName` at compile time:
/// `0x5F 0x50 0x53 | 0x00 | index | type_byte | name.len() | name`.
///
/// `N` must equal `7 + name.len()` — [`sig_name!`](crate::sig_name)
/// computes it for you; called directly, get `N` right or hit the assert.
/// Every MUST of the wire format is a `const`-evaluable `assert!`, so any
/// violation is a compile error at the call site, never a runtime panic or
/// a silently-malformed name:
///
/// - `index <= 0x0F`
/// - `type_byte` is one of the twelve codes [`SigParamType`] implements
/// - `name` is 1..=16 bytes matching `[A-Za-z][A-Za-z0-9]*`
/// - `N == 7 + name.len()`
///
/// # Examples
///
/// ```
/// use rshooks::sig::sig_param_name;
///
/// // account(0): index 0, STI_ACCOUNT (0x08), name b"account" (7 bytes).
/// const ACCOUNT: [u8; 14] = sig_param_name::<14>(0, 0x08, b"account");
/// assert_eq!(
///     ACCOUNT,
///     [0x5F, 0x50, 0x53, 0x00, 0x00, 0x08, 0x07, b'a', b'c', b'c', b'o', b'u', b'n', b't']
/// );
///
/// // count(1): index 1, STI_UINT16 (0x01), name b"count" (5 bytes).
/// const COUNT: [u8; 12] = sig_param_name::<12>(1, 0x01, b"count");
/// assert_eq!(
///     COUNT,
///     [0x5F, 0x50, 0x53, 0x00, 0x01, 0x01, 0x05, b'c', b'o', b'u', b'n', b't']
/// );
/// ```
///
/// An index above `0x0F` fails to compile:
/// ```compile_fail
/// const _BAD: [u8; 12] = rshooks::sig::sig_param_name::<12>(16, 0x01, b"count");
/// ```
///
/// An unsupported type byte fails to compile:
/// ```compile_fail
/// const _BAD: [u8; 12] = rshooks::sig::sig_param_name::<12>(0, 0x09, b"count");
/// ```
///
/// A name containing `_` fails to compile (not `[A-Za-z][A-Za-z0-9]*`):
/// ```compile_fail
/// const _BAD: [u8; 15] = rshooks::sig::sig_param_name::<15>(0, 0x01, b"my_count");
/// ```
#[allow(clippy::indexing_slicing)] // in-bounds by the `N == name.len() + FIXED_LEN` assert, const-evaluated only
#[must_use]
pub const fn sig_param_name<const N: usize>(index: u8, type_byte: u8, name: &[u8]) -> [u8; N] {
    assert!(index <= 0x0F, "rshooks::sig: index must be in 0x00..=0x0F");
    assert!(
        is_supported_type_byte(type_byte),
        "rshooks::sig: unsupported type byte (must be one of the 12 SigParamType codes)"
    );
    assert!(
        is_valid_name(name),
        "rshooks::sig: name must be 1..=16 bytes matching [A-Za-z][A-Za-z0-9]*"
    );
    assert!(
        N == name.len().wrapping_add(FIXED_LEN),
        "rshooks::sig: N must equal 7 + name.len()"
    );

    let mut out = [0u8; N];
    out[0] = 0x5F;
    out[1] = 0x50;
    out[2] = 0x53;
    out[3] = 0x00;
    out[4] = index;
    out[5] = type_byte;
    out[6] = name.len() as u8;
    let mut i = 0;
    while i < name.len() {
        out[FIXED_LEN.wrapping_add(i)] = name[i];
        i = i.wrapping_add(1);
    }
    out
}

/// Builds a declared `HookParameterName` as a `const` `[u8; N]`, resolving
/// `N` and the type byte for you: `sig_name!(0, u16, b"count")` expands to
/// [`sig_param_name`] called with `N = 7 + b"count".len()` and
/// `type_byte = <u16 as SigParamType>::TYPE_BYTE`. The result is a `[u8; N]`
/// value, usable directly with [`crate::api::otxn::otxn_param_exact`]/
/// [`crate::api::hook_ctx::hook_param_exact`], or as the argument to
/// [`SigName::new`].
///
/// # Examples
///
/// ```
/// use rshooks::sig_name;
///
/// const COUNT: [u8; 12] = sig_name!(1, u16, b"count");
/// assert_eq!(
///     COUNT,
///     [0x5F, 0x50, 0x53, 0x00, 0x01, 0x01, 0x05, b'c', b'o', b'u', b'n', b't']
/// );
/// ```
#[macro_export]
macro_rules! sig_name {
    ($index:expr, $ty:ty, $name:expr) => {
        const {
            $crate::sig::sig_param_name::<{ 7 + $name.len() }>(
                $index,
                <$ty as $crate::sig::SigParamType>::TYPE_BYTE,
                $name,
            )
        }
    };
}

// ---------------------------------------------------------------------------
// SigParamType
// ---------------------------------------------------------------------------

/// A Rust type readable as one signature parameter's invocation value —
/// pairs an `STI_*` wire type byte with a decode of the type-delimited
/// payload the host hands back (already length-delimited by
/// `otxn_param`/`hook_param`'s own "bytes written" return, exactly like
/// [`crate::convert::FixedRead::read_exact`]'s `read` closure).
///
/// See the module doc's "Why big-endian" section for why this trait's
/// integer impls decode big-endian, unlike [`crate::convert::FromBytes`].
pub trait SigParamType: Sized {
    /// The `STI_*` type byte advertised in the declared name (see
    /// `docs/PARAM_SIGNATURE_DESIGN.md` §2's table).
    const TYPE_BYTE: u8;

    /// Decodes the invocation value payload. `read` is handed a buffer and
    /// must report how many bytes it wrote — the same shape
    /// [`crate::convert::FixedRead::read_exact`] uses, so a caller-buffer
    /// Hook API wrapper (`otxn_param`, `hook_param`) can be passed directly.
    ///
    /// # Errors
    ///
    /// Propagates any error `read` returns. Returns an error variant
    /// appropriate to the wrong-length/undecodable payload otherwise — see
    /// each impl's doc comment for its own classification.
    fn read_sig(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self>;
}

/// Generates a big-endian-decoding [`SigParamType`] impl for a narrow
/// unsigned integer, via `$ty::from_be_bytes`.
macro_rules! be_int_sig {
    ($ty:ty, $len:literal, $type_byte:literal) => {
        impl SigParamType for $ty {
            const TYPE_BYTE: u8 = $type_byte;

            #[inline(always)]
            fn read_sig(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
                let mut buf = [0u8; $len];
                let written = read(&mut buf)?;
                if written == $len {
                    Ok(<$ty>::from_be_bytes(buf))
                } else {
                    Err(HookError::TooSmall)
                }
            }
        }
    };
}

impl SigParamType for u8 {
    /// `STI_UINT8`.
    const TYPE_BYTE: u8 = 0x10;

    #[inline(always)]
    fn read_sig(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
        let mut buf = [0u8; 1];
        let written = read(&mut buf)?;
        if written == 1 {
            buf.first().copied().ok_or(HookError::TooSmall)
        } else {
            Err(HookError::TooSmall)
        }
    }
}

be_int_sig!(u16, 2, 0x01); // STI_UINT16
be_int_sig!(u32, 4, 0x02); // STI_UINT32
be_int_sig!(u64, 8, 0x03); // STI_UINT64

/// Generates a [`SigParamType`] impl for a type that already implements
/// [`crate::convert::FixedRead`] with the exact same "exactly N bytes or
/// `TooSmall`" contract [`SigParamType::read_sig`] needs, by reusing
/// [`FixedRead::read_exact`] directly.
macro_rules! fixed_read_sig {
    ($ty:ty, $type_byte:literal) => {
        impl SigParamType for $ty {
            const TYPE_BYTE: u8 = $type_byte;

            #[inline(always)]
            fn read_sig(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
                <$ty as FixedRead>::read_exact(read)
            }
        }
    };
}

fixed_read_sig!([u8; 16], 0x04); // STI_UINT128
fixed_read_sig!([u8; 32], 0x05); // STI_UINT256
fixed_read_sig!(Hash, 0x05); // STI_UINT256
fixed_read_sig!(AccountId, 0x08); // STI_ACCOUNT
fixed_read_sig!([u8; 20], 0x11); // STI_UINT160
fixed_read_sig!(CurrencyCode, 0x1A); // STI_CURRENCY

impl SigParamType for AmountBytes {
    /// `STI_AMOUNT`.
    const TYPE_BYTE: u8 = 0x06;

    /// Reuses [`crate::slot_obj::classify_amount`] — the same 8-byte-native/
    /// 48-byte-IOU classification [`crate::api::otxn::otxn_field_typed`]'s
    /// `Amount` impl uses, including its `HookError::ParseError` for any
    /// other length (MPT's 33 bytes among them — out of scope, see
    /// [`AmountBytes`]'s own doc comment).
    #[inline(always)]
    fn read_sig(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
        let mut buf = [0u8; crate::types::IOU_AMOUNT_LEN];
        let written = read(&mut buf)?;
        let bytes = buf.get(..written).ok_or(HookError::TooBig)?;
        classify_amount(bytes)
    }
}

// ---------------------------------------------------------------------------
// Blob<N>
// ---------------------------------------------------------------------------

/// A variable-length signature parameter value (`STI_VL`), up to `N` bytes.
///
/// # Why `N` bounds detection rather than silent truncation
///
/// [`SigParamType::read_sig`] hands `read` a buffer of exactly `N` bytes.
/// Unlike [`crate::api::hook_ctx::hook_param`] (whose host call writes
/// `min(src_len, dst_len)` and reports the truncated length — see
/// [`crate::testenv_bridge::write_bytes_truncate`]), `otxn_param` — the
/// call [`Blob`] reads through — checks the destination length *first* and
/// returns `TOO_SMALL` instead of truncating (the contract
/// [`crate::testenv_bridge::write_bytes`] documents). A `Blob<N>` too small
/// for the actual invocation value therefore surfaces as
/// [`HookError::TooSmall`] from the host call itself, never a
/// silently-truncated read — no separate overflow check is needed here.
///
/// `N` is capped at 256 (`docs/PARAM_SIGNATURE_DESIGN.md` §2's `STI_VL`
/// row: `1..=min(N, 256)`), checked at monomorphization time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blob<const N: usize> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> Blob<N> {
    /// The value's bytes (`self.len()` of them, never more than `N`).
    #[inline(always)]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }

    /// The value's length in bytes (`1..=N`).
    #[inline(always)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the value is empty. Cannot occur through
    /// [`SigParamType::read_sig`] (see [`Blob`]'s doc comment), but kept for
    /// the ordinary `len`/`is_empty` pairing.
    #[inline(always)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> SigParamType for Blob<N> {
    /// `STI_VL`.
    const TYPE_BYTE: u8 = 0x07;

    #[inline(always)]
    fn read_sig(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
        const {
            assert!(
                N >= 1 && N <= 256,
                "rshooks::sig: Blob<N> requires 1 <= N <= 256 (the STI_VL cap)"
            );
        }
        let mut bytes = [0u8; N];
        let written = read(&mut bytes)?;
        if written == 0 {
            // Cannot occur via `otxn_param`/`hook_param` (empty means
            // absent), but decode still guards 1..=N rather than assuming it.
            return Err(HookError::TooSmall);
        }
        Ok(Blob {
            len: written,
            bytes,
        })
    }
}

// ---------------------------------------------------------------------------
// IssueBytes
// ---------------------------------------------------------------------------

/// A serialized `Issue` signature parameter value (`STI_ISSUE`): native (20
/// all-zero bytes) or an issued asset (40 bytes: currency then issuer).
///
/// Distinct from [`crate::slot_obj::IssueData`], which classifies a
/// serialized `Issue` purely by *length* (20 vs. 40 bytes): per the
/// interface draft, a signature parameter's 20-byte form is native **only**
/// if every byte is zero — 20 non-zero bytes is malformed (there is no
/// other 20-byte `Issue` shape) and reported as [`HookError::ParseError`],
/// not silently accepted the way `IssueData::Native` would.
/// [`SigParamType::read_sig`]'s read buffer is sized to 44 bytes (an MPT
/// issue's own length, `ISSUE_MAX_READ_LEN` in `crate::slot_obj`), not just
/// the 40 an IOU issue needs, so a 44-byte MPT-shaped value is *read* and
/// rejected as `ParseError` by this type's own length match rather than
/// surfacing as a host-call [`HookError::TooSmall`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueBytes {
    /// A 20-byte all-zero native issue.
    Native,
    /// A 40-byte IOU issue: currency then issuer.
    Issued(IssuedAsset),
}

impl SigParamType for IssueBytes {
    /// `STI_ISSUE`.
    const TYPE_BYTE: u8 = 0x18;

    #[inline(always)]
    fn read_sig(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
        const IOU_LEN: usize = CURRENCY_CODE_LEN + ACC_ID_LEN;
        // Buffer is `ISSUE_MAX_READ_LEN` (44), not the 40 an IOU issue
        // needs — see the doc comment above.
        let mut buf = [0u8; ISSUE_MAX_READ_LEN];
        let written = read(&mut buf)?;
        match written {
            CURRENCY_CODE_LEN => {
                let native: [u8; CURRENCY_CODE_LEN] = buf
                    .get(..CURRENCY_CODE_LEN)
                    .and_then(|s| s.try_into().ok())
                    .ok_or(HookError::TooSmall)?;
                if buf_eq_20(&native, &[0u8; CURRENCY_CODE_LEN]) {
                    Ok(IssueBytes::Native)
                } else {
                    Err(HookError::ParseError)
                }
            }
            IOU_LEN => {
                let currency: [u8; CURRENCY_CODE_LEN] = buf
                    .get(..CURRENCY_CODE_LEN)
                    .and_then(|s| s.try_into().ok())
                    .ok_or(HookError::TooSmall)?;
                let issuer: [u8; ACC_ID_LEN] = buf
                    .get(CURRENCY_CODE_LEN..IOU_LEN)
                    .and_then(|s| s.try_into().ok())
                    .ok_or(HookError::TooSmall)?;
                Ok(IssueBytes::Issued(IssuedAsset {
                    currency: CurrencyCode(currency),
                    issuer: AccountId(issuer),
                }))
            }
            _ => Err(HookError::ParseError),
        }
    }
}

// ---------------------------------------------------------------------------
// otxn_sig_param / hook_sig_param / otxn_sig_param_opt / hook_sig_param_opt
// ---------------------------------------------------------------------------

/// Reads a Hook parameter attached to the originating transaction as a
/// declared signature value — the `SigParamType` counterpart to
/// [`crate::api::otxn::otxn_param_exact`], mirroring it exactly (a plain
/// `Result` passthrough; no absence handling — see [`otxn_sig_param_opt`]
/// for that).
///
/// # Errors
///
/// Propagates the underlying `otxn_param` host-call error, or a decode
/// error from `T::read_sig` — see [`SigParamType`]'s impls for the exact
/// per-type classification.
///
/// # Examples
///
/// ```
/// use rshooks::error::{HookError, Result};
/// use rshooks::sig::otxn_sig_param;
///
/// let value: Result<u16> = otxn_sig_param(&rshooks::sig_name!(0, u16, b"count"));
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn otxn_sig_param<T: SigParamType>(name: &[u8]) -> Result<T> {
    T::read_sig(|buf| otxn_param(buf, name))
}

/// Reads this hook's own parameter as a declared signature value — the
/// `SigParamType` counterpart to
/// [`crate::api::hook_ctx::hook_param_exact`], mirroring
/// [`otxn_sig_param`]'s shape against `hook_param` instead of `otxn_param`.
///
/// # Errors
///
/// See [`otxn_sig_param`]'s doc comment.
///
/// # Examples
///
/// ```
/// use rshooks::error::{HookError, Result};
/// use rshooks::sig::hook_sig_param;
///
/// let value: Result<u16> = hook_sig_param(&rshooks::sig_name!(0, u16, b"count"));
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn hook_sig_param<T: SigParamType>(name: &[u8]) -> Result<T> {
    T::read_sig(|buf| hook_param(buf, name))
}

/// Reads a Hook parameter attached to the originating transaction as a
/// declared signature value, distinguishing "parameter is absent" from
/// every other outcome — the `SigParamType` counterpart to
/// [`crate::api::otxn::otxn_param_opt`]: `Ok(None)` only for
/// `DOESNT_EXIST`, `Err` for a present-but-undecodable value.
///
/// # Errors
///
/// See [`crate::api::otxn::otxn_param_opt`]'s doc comment for the full
/// absence/error contract.
///
/// # Examples
///
/// ```
/// use rshooks::error::{HookError, Result};
/// use rshooks::sig::otxn_sig_param_opt;
///
/// let value: Result<Option<u16>> = otxn_sig_param_opt(&rshooks::sig_name!(0, u16, b"count"));
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn otxn_sig_param_opt<T: SigParamType>(name: &[u8]) -> Result<Option<T>> {
    let mut absent = false;
    let r = T::read_sig(|buf| {
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

/// Reads this hook's own parameter as a declared signature value,
/// distinguishing "parameter is absent" from every other outcome — the
/// `SigParamType` counterpart to [`crate::api::hook_ctx::hook_param_opt`],
/// mirroring [`otxn_sig_param_opt`]'s shape against `hook_param` instead of
/// `otxn_param`.
///
/// # Errors
///
/// See [`crate::api::hook_ctx::hook_param_opt`]'s doc comment for the full
/// absence/error contract.
///
/// # Examples
///
/// ```
/// use rshooks::error::{HookError, Result};
/// use rshooks::sig::hook_sig_param_opt;
///
/// let value: Result<Option<u16>> = hook_sig_param_opt(&rshooks::sig_name!(0, u16, b"count"));
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn hook_sig_param_opt<T: SigParamType>(name: &[u8]) -> Result<Option<T>> {
    let mut absent = false;
    let r = T::read_sig(|buf| {
        let code = hook_param_raw_code(buf, name);
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

// ---------------------------------------------------------------------------
// SigName<V>
// ---------------------------------------------------------------------------

/// A declared signature parameter name, paired with the [`SigParamType`] it
/// reads as — carries the prebuilt `'static` name bytes (typically a
/// [`sig_name!`](crate::sig_name)-built `const`) so a call site names the
/// parameter once and reads it with [`SigName::otxn_read`]/
/// [`SigName::hook_read`], no repeated name argument. See the module doc's
/// "`SigName<V>`" section for why this does not implement
/// [`crate::convert::TypedParamName`].
pub struct SigName<V: SigParamType> {
    bytes: &'static [u8],
    _value: PhantomData<fn() -> V>,
}

// Hand-written rather than derived: `#[derive(Clone, Copy)]` on a generic
// struct adds a `V: Clone`/`V: Copy` bound, but the phantom carries no `V`
// value, so `SigName<V>` should stay `Clone`/`Copy` unconditionally — same
// reasoning `types.rs`'s `SField<T>` doc comment gives.
impl<V: SigParamType> Clone for SigName<V> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<V: SigParamType> Copy for SigName<V> {}

impl<V: SigParamType> SigName<V> {
    /// Wraps prebuilt declared-name bytes (typically a
    /// [`sig_name!`](crate::sig_name)-built `const`). No validation here —
    /// [`sig_param_name`]'s own `const`-time asserts (via `sig_name!`) are
    /// where a malformed name is rejected; this constructor accepts
    /// whatever bytes it is handed.
    #[inline(always)]
    #[must_use]
    pub const fn new(bytes: &'static [u8]) -> Self {
        Self {
            bytes,
            _value: PhantomData,
        }
    }

    /// The declared name's raw bytes.
    #[inline(always)]
    #[must_use]
    pub const fn as_bytes(&self) -> &'static [u8] {
        self.bytes
    }

    /// [`otxn_sig_param`] against this name.
    ///
    /// # Errors
    ///
    /// See [`otxn_sig_param`]'s doc comment.
    #[inline(always)]
    pub fn otxn_read(&self) -> Result<V> {
        otxn_sig_param(self.bytes)
    }

    /// [`hook_sig_param`] against this name.
    ///
    /// # Errors
    ///
    /// See [`hook_sig_param`]'s doc comment.
    #[inline(always)]
    pub fn hook_read(&self) -> Result<V> {
        hook_sig_param(self.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- sig_param_name / sig_name! name-builder vectors ---------------

    #[test]
    fn name_vector_account() {
        const NAME: [u8; 14] = sig_param_name::<14>(0, 0x08, b"account");
        assert_eq!(
            NAME,
            [
                0x5F, 0x50, 0x53, 0x00, 0x00, 0x08, 0x07, b'a', b'c', b'c', b'o', b'u', b'n', b't'
            ]
        );
    }

    #[test]
    fn name_vector_count() {
        const NAME: [u8; 12] = sig_param_name::<12>(1, 0x01, b"count");
        assert_eq!(
            NAME,
            [
                0x5F, 0x50, 0x53, 0x00, 0x01, 0x01, 0x05, b'c', b'o', b'u', b'n', b't'
            ]
        );
    }

    #[test]
    fn sig_name_macro_matches_direct_call() {
        const VIA_MACRO: [u8; 12] = sig_name!(1, u16, b"count");
        const VIA_FN: [u8; 12] = sig_param_name::<12>(1, 0x01, b"count");
        assert_eq!(VIA_MACRO, VIA_FN);
    }

    #[test]
    fn name_sixteen_byte_name_at_max_index() {
        const NAME: [u8; 23] = sig_param_name::<23>(0x0F, 0x10, b"abcdefghijklmnop");
        let mut expected = [0u8; 23];
        if let Some(dst) = expected.get_mut(0) {
            *dst = 0x5F;
        }
        if let Some(dst) = expected.get_mut(1) {
            *dst = 0x50;
        }
        if let Some(dst) = expected.get_mut(2) {
            *dst = 0x53;
        }
        if let Some(dst) = expected.get_mut(4) {
            *dst = 0x0F;
        }
        if let Some(dst) = expected.get_mut(5) {
            *dst = 0x10;
        }
        if let Some(dst) = expected.get_mut(6) {
            *dst = 0x10;
        }
        if let Some(dst) = expected.get_mut(7..) {
            dst.copy_from_slice(b"abcdefghijklmnop");
        }
        assert_eq!(NAME, expected);
    }

    // --- SigParamType decode round-trips --------------------------------

    #[test]
    fn u8_decodes() {
        let v: Result<u8> = u8::read_sig(|buf| {
            buf.copy_from_slice(&[0x42]);
            Ok(1)
        });
        assert_eq!(v, Ok(0x42));
    }

    #[test]
    fn u8_wrong_length_is_too_small() {
        let v: Result<u8> = u8::read_sig(|_buf| Ok(0));
        assert_eq!(v, Err(HookError::TooSmall));
    }

    #[test]
    fn u16_decodes_big_endian() {
        let v: Result<u16> = u16::read_sig(|buf| {
            buf.copy_from_slice(&[0x00, 0x05]);
            Ok(2)
        });
        assert_eq!(v, Ok(5u16));
    }

    #[test]
    fn u16_wrong_length_is_too_small() {
        let v: Result<u16> = u16::read_sig(|_buf| Ok(1));
        assert_eq!(v, Err(HookError::TooSmall));
    }

    #[test]
    fn u32_decodes_big_endian() {
        let v: Result<u32> = u32::read_sig(|buf| {
            buf.copy_from_slice(&[0x00, 0x00, 0x01, 0x00]);
            Ok(4)
        });
        assert_eq!(v, Ok(256u32));
    }

    #[test]
    fn u64_decodes_big_endian() {
        let v: Result<u64> = u64::read_sig(|buf| {
            buf.copy_from_slice(&[0, 0, 0, 0, 0, 0, 1, 0]);
            Ok(8)
        });
        assert_eq!(v, Ok(256u64));
    }

    #[test]
    fn fixed_16_decodes() {
        let v: Result<[u8; 16]> = <[u8; 16]>::read_sig(|buf| {
            buf.copy_from_slice(&[7u8; 16]);
            Ok(16)
        });
        assert_eq!(v, Ok([7u8; 16]));
    }

    #[test]
    fn fixed_32_decodes() {
        let v: Result<[u8; 32]> = <[u8; 32]>::read_sig(|buf| {
            buf.copy_from_slice(&[9u8; 32]);
            Ok(32)
        });
        assert_eq!(v, Ok([9u8; 32]));
    }

    #[test]
    fn hash_decodes() {
        let v: Result<Hash> = Hash::read_sig(|buf| {
            buf.copy_from_slice(&[1u8; 32]);
            Ok(32)
        });
        assert_eq!(v, Ok(Hash([1u8; 32])));
    }

    #[test]
    fn account_id_decodes() {
        let v: Result<AccountId> = AccountId::read_sig(|buf| {
            buf.copy_from_slice(&[2u8; 20]);
            Ok(20)
        });
        assert_eq!(v, Ok(AccountId([2u8; 20])));
    }

    #[test]
    fn fixed_20_decodes() {
        let v: Result<[u8; 20]> = <[u8; 20]>::read_sig(|buf| {
            buf.copy_from_slice(&[3u8; 20]);
            Ok(20)
        });
        assert_eq!(v, Ok([3u8; 20]));
    }

    #[test]
    fn currency_code_decodes() {
        let v: Result<CurrencyCode> = CurrencyCode::read_sig(|buf| {
            buf.copy_from_slice(&[4u8; 20]);
            Ok(20)
        });
        assert_eq!(v, Ok(CurrencyCode([4u8; 20])));
    }

    #[test]
    fn amount_bytes_native_8() {
        let v: Result<AmountBytes> = AmountBytes::read_sig(|buf| {
            if let Some(dst) = buf.get_mut(..8) {
                dst.copy_from_slice(&[5u8; 8]);
            }
            Ok(8)
        });
        assert_eq!(
            v,
            Ok(AmountBytes::Native(crate::types::NativeAmount([5u8; 8])))
        );
    }

    #[test]
    fn amount_bytes_iou_48() {
        let v: Result<AmountBytes> = AmountBytes::read_sig(|buf| {
            buf.copy_from_slice(&[6u8; 48]);
            Ok(48)
        });
        assert_eq!(v, Ok(AmountBytes::Iou(crate::types::IouAmount([6u8; 48]))));
    }

    #[test]
    fn amount_bytes_wrong_length_is_parse_error() {
        let v: Result<AmountBytes> = AmountBytes::read_sig(|buf| {
            if let Some(dst) = buf.get_mut(..10) {
                dst.fill(1);
            }
            Ok(10)
        });
        assert_eq!(v, Err(HookError::ParseError));
    }

    #[test]
    fn issue_bytes_native_all_zero() {
        let v: Result<IssueBytes> = IssueBytes::read_sig(|buf| {
            if let Some(dst) = buf.get_mut(..20) {
                dst.fill(0);
            }
            Ok(20)
        });
        assert_eq!(v, Ok(IssueBytes::Native));
    }

    #[test]
    fn issue_bytes_twenty_nonzero_is_error() {
        let v: Result<IssueBytes> = IssueBytes::read_sig(|buf| {
            if let Some(dst) = buf.get_mut(..20) {
                dst.fill(1);
            }
            Ok(20)
        });
        assert_eq!(v, Err(HookError::ParseError));
    }

    #[test]
    fn issue_bytes_forty_decodes_currency_then_issuer() {
        let v: Result<IssueBytes> = IssueBytes::read_sig(|buf| {
            if let Some(dst) = buf.get_mut(..20) {
                dst.fill(0xAA);
            }
            if let Some(dst) = buf.get_mut(20..40) {
                dst.fill(0xBB);
            }
            Ok(40)
        });
        assert_eq!(
            v,
            Ok(IssueBytes::Issued(IssuedAsset {
                currency: CurrencyCode([0xAA; 20]),
                issuer: AccountId([0xBB; 20]),
            }))
        );
    }

    #[test]
    fn issue_bytes_other_length_is_parse_error() {
        let v: Result<IssueBytes> = IssueBytes::read_sig(|_buf| Ok(5));
        assert_eq!(v, Err(HookError::ParseError));
    }

    #[test]
    fn issue_bytes_read_buffer_is_forty_four_bytes_so_mpt_shaped_values_are_read_and_rejected() {
        // Confirms the read buffer is 44 bytes wide (not just the 40 an IOU
        // issue needs) — see `IssueBytes`'s doc comment.
        let v: Result<IssueBytes> = IssueBytes::read_sig(|buf| {
            assert_eq!(buf.len(), 44);
            buf.fill(0xCC);
            Ok(44)
        });
        assert_eq!(v, Err(HookError::ParseError));
    }

    #[test]
    fn blob_within_bounds_decodes() {
        let v: Result<Blob<8>> = Blob::<8>::read_sig(|buf| {
            if let Some(dst) = buf.get_mut(..3) {
                dst.copy_from_slice(&[1, 2, 3]);
            }
            Ok(3)
        });
        assert_eq!(
            v,
            Ok(Blob {
                len: 3,
                bytes: [1, 2, 3, 0, 0, 0, 0, 0],
            })
        );
        if let Ok(blob) = v {
            assert_eq!(blob.as_bytes(), &[1, 2, 3]);
            assert!(!blob.is_empty());
        }
    }

    #[test]
    fn blob_full_width_decodes() {
        let v: Result<Blob<4>> = Blob::<4>::read_sig(|buf| {
            buf.copy_from_slice(&[9, 9, 9, 9]);
            Ok(4)
        });
        assert_eq!(
            v,
            Ok(Blob {
                len: 4,
                bytes: [9, 9, 9, 9],
            })
        );
    }

    #[test]
    fn blob_zero_length_is_too_small() {
        let v: Result<Blob<4>> = Blob::<4>::read_sig(|_buf| Ok(0));
        assert_eq!(v, Err(HookError::TooSmall));
    }

    #[test]
    fn blob_overflow_propagates_host_too_small() {
        // The host call itself (not `Blob`'s own decode) is what rejects an
        // over-length value — see `Blob`'s doc comment. This simulates that
        // host behavior directly.
        let v: Result<Blob<4>> = Blob::<4>::read_sig(|_buf| Err(HookError::TooSmall));
        assert_eq!(v, Err(HookError::TooSmall));
    }

    // --- host-call smoke test (host stub returns NotImplemented) --------

    #[test]
    fn smoke_not_implemented_on_host() {
        const NAME: [u8; 12] = sig_name!(0, u16, b"count");
        assert_eq!(otxn_sig_param::<u16>(&NAME), Err(HookError::NotImplemented));
        assert_eq!(hook_sig_param::<u16>(&NAME), Err(HookError::NotImplemented));
        assert_eq!(
            otxn_sig_param_opt::<u16>(&NAME),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            hook_sig_param_opt::<u16>(&NAME),
            Err(HookError::NotImplemented)
        );

        let sig_name_handle: SigName<u16> = SigName::new(&NAME);
        assert_eq!(sig_name_handle.as_bytes(), &NAME);
        assert_eq!(sig_name_handle.otxn_read(), Err(HookError::NotImplemented));
        assert_eq!(sig_name_handle.hook_read(), Err(HookError::NotImplemented));
    }
}

// Const-assert failure cases (bad charset, index > 0x0F, unsupported type
// byte, wrong `N`) cannot be exercised as runtime `#[test]`s — each is a
// `compile_fail` doctest above instead.
