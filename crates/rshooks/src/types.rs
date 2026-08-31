//! Fixed-size protocol buffer newtypes.
//!
//! Each type below (`AccountId`, `Hash`, `Keylet`, ...) is a
//! `#[repr(transparent)]` tuple struct wrapping a `[u8; N]`, not a bare type
//! alias — `#[repr(transparent)]` guarantees the wrapper has exactly the
//! inner array's layout, size, and alignment (zero-cost, FFI-compatible
//! with a raw `[u8; N]`), so the newtype only adds type-level distinctness
//! (an `AccountId` and a `Hash` can no longer be passed to each other's
//! slots by accident).
//!
//! The inner field is `pub` (`AccountId(pub [u8; 20])`) and every type
//! implements [`core::ops::Deref`]/[`core::ops::DerefMut`] (target
//! `[u8; N]`), [`AsRef<[u8]>`]/[`AsMut<[u8]>`], and
//! `From<[u8; N]>`/`Into<[u8; N]>`: method calls (`.as_ptr()`, `.len()`,
//! indexing, `.starts_with(..)`, ...) reach the inner array via auto-deref
//! exactly as they did as plain array aliases. Passing a newtype by
//! reference where a *bare* `&[u8]`/`&mut [u8]` parameter is expected does
//! need an explicit conversion (`value.as_ref()`/`value.as_mut()`) — Rust
//! does not chain a user `Deref` impl with the array-to-slice unsized
//! coercion at a call site — but every `rshooks::api` wrapper that takes a
//! caller buffer or key/value byte slice (`state`, `otxn_field`,
//! `hook_param`, `hook_account`, `ledger_last_hash`, `util_accid`,
//! `etxn_details`, `slot`, ...) bounds the parameter with
//! `AsRef<[u8]>`/`AsMut<[u8]>` instead of a bare `&[u8]`/`&mut [u8]`, so
//! `otxn_field(&mut sender, sfAccount)` works as-is, no conversion needed
//! (see `api::state`'s module doc for the `ForeignRef` exception —
//! `state_foreign`'s `Option`-shaped `namespace`/`account`). The explicit
//! conversion is only needed outside that wrapper layer (a hook's own
//! helper function, `core`/`alloc` APIs, ...).
//!
//! Every type also implements [`crate::convert::ToBytes`]/
//! [`crate::convert::FromBytes`] (a fixed-length passthrough to/from its
//! inner array), so all ten work directly as
//! [`crate::state::state_get`]/[`crate::state::state_set_loose`] value
//! types, and [`crate::convert::FixedRead`], so a type can be the return
//! type of `otxn_field_exact`/`hook_param_exact`/`slot_exact`/`state_exact`
//! — `let sender: AccountId = otxn_field_exact(sfAccount)?;`, no turbofish.
//!
//! All of these are always zero-initialized as `[0u8; N]` at call sites,
//! never via `MaybeUninit` (see `macros.rs` for why `uninit_buf!` is
//! deliberately not provided). Every type provides two equivalent ways to
//! get that zero value: [`Default::default`] for ordinary `let` bindings,
//! and a `const fn zeroed() -> Self` for `const`/`static` contexts (where
//! `Default::default` can't be called — `Default` cannot be a `const fn`
//! trait method on stable Rust). Both produce the identical all-zero value;
//! the typical use is a fixed-size scratch buffer for a host call's
//! caller-buffer output parameter, e.g. `let mut sender =
//! AccountId::default(); otxn_field(&mut sender, sfAccount)?;`.
//!
//! `==`/`!=` on any of these ten types is loop-free: `PartialEq` is a
//! hand-written impl delegating to the matching [`crate::buf_eq`] function
//! rather than a derive (derived `==` on a `[u8; N]`-backed type compiles
//! to a `memcmp` loop on `wasm32v1-none`). That drops `StructuralPartialEq`,
//! so a `const`/`static` of one of these types can't be used as a `match`
//! pattern (`match account { OWNER => ..., _ => ... }` — rewrite as
//! `if account == OWNER { ... }`); the same loss propagates to any
//! downstream struct/enum holding one of these types as a field, which
//! likewise can't be matched against a `const`/`static` pattern. Values
//! themselves (`==`, `HashMap` keys, ...) are unaffected. [`AccountId`]
//! additionally implements
//! `Ord`/`PartialOrd`, also loop-free (via [`crate::buf_eq::buf_cmp_20`]),
//! giving the canonical 160-bit big-endian ordering XRPL/Xahau uses to pick
//! the high/low account of a pair (e.g. a `RippleState` trustline keylet).

use core::marker::PhantomData;

use crate::convert::{FixedRead, FromBytes, ToBytes};
use crate::error::{HookError, Result};

/// Length in bytes of an [`AccountId`].
pub const ACC_ID_LEN: usize = 20;
/// Length in bytes of a [`Hash`].
pub const HASH_LEN: usize = 32;
/// Length in bytes of a [`Keylet`].
pub const KEYLET_LEN: usize = 34;
/// Length in bytes of a [`StateKey`].
pub const STATE_KEY_LEN: usize = 32;
/// Length in bytes of a [`NameSpace`].
pub const NAMESPACE_LEN: usize = 32;
/// Length in bytes of a [`Nonce`].
pub const NONCE_LEN: usize = 32;
/// Length in bytes of a [`PublicKey`].
pub const PUB_KEY_LEN: usize = 33;
/// Length in bytes of a [`CurrencyCode`].
pub const CURRENCY_CODE_LEN: usize = 20;
/// Length in bytes of a [`NativeAmount`].
pub const NATIVE_AMOUNT_LEN: usize = 8;
/// Length in bytes of an [`IouAmount`].
pub const IOU_AMOUNT_LEN: usize = 48;
/// Maximum length in bytes of a serialized `EmitDetails` object
/// (`etxn_details` output): 138 bytes when this hook's wasm module exports
/// a `cbak` callback, 116 bytes otherwise (see xahaud's
/// `HookAPI::etxn_details`, `src/xrpld/app/hook/detail/HookAPI.cpp`).
/// `etxn_details` is a caller-buffer/returned-length API (like
/// [`crate::api::hook_ctx::hook_param`]) — size a buffer to this constant
/// and trust the returned length; do not assume it is always fully written.
pub const EMIT_DETAILS_MAX_LEN: usize = 138;

/// Defines one `#[repr(transparent)]` fixed-size buffer newtype, plus its
/// `Deref`/`DerefMut`/`AsRef`/`AsMut`/`From`/`Default`/`zeroed`/`ToBytes`/
/// `FromBytes`/`FixedRead`/`PartialEq`/`Eq` impls. See the module doc
/// comment for the rationale.
///
/// `PartialEq` delegates to `$eq_fn` (one of the loop-free
/// `crate::buf_eq::buf_eq_*` functions) rather than being derived — see the
/// module doc comment for why, and for the `match`-pattern implication.
/// `Eq` is still derived.
macro_rules! fixed_bytes_type {
    ($(#[$meta:meta])* $name:ident, $len:expr, $eq_fn:path) => {
        $(#[$meta])*
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, Eq)]
        pub struct $name(pub [u8; $len]);

        impl PartialEq for $name {
            #[inline(always)]
            fn eq(&self, other: &Self) -> bool {
                $eq_fn(&self.0, &other.0)
            }
        }

        impl $name {
            #[doc = concat!(
                "An all-zero `", stringify!($name), "`, usable in `const`/",
                "`static` contexts (unlike [`Default::default`], which ",
                "this returns the same value as — `Default` cannot be a ",
                "`const fn` on stable Rust). Typical use: a fixed-size ",
                "scratch buffer for a host call's caller-buffer output ",
                "parameter, e.g. `let mut sender = ", stringify!($name),
                "::zeroed();` before `otxn_field(&mut sender, sfAccount)`."
            )]
            #[inline(always)]
            #[must_use]
            pub const fn zeroed() -> Self {
                $name([0u8; $len])
            }
        }

        impl core::ops::Deref for $name {
            type Target = [u8; $len];

            #[inline(always)]
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl core::ops::DerefMut for $name {
            #[inline(always)]
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }

        impl AsRef<[u8]> for $name {
            #[inline(always)]
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        impl AsMut<[u8]> for $name {
            #[inline(always)]
            fn as_mut(&mut self) -> &mut [u8] {
                &mut self.0
            }
        }

        impl From<[u8; $len]> for $name {
            #[inline(always)]
            fn from(value: [u8; $len]) -> Self {
                $name(value)
            }
        }

        impl From<$name> for [u8; $len] {
            #[inline(always)]
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl Default for $name {
            #[inline(always)]
            fn default() -> Self {
                Self::zeroed()
            }
        }

        impl ToBytes for $name {
            const MAX_LEN: usize = $len;

            #[inline(always)]
            fn write(&self, buf: &mut [u8]) -> usize {
                self.0.write(buf)
            }
        }

        impl FromBytes for $name {
            #[inline(always)]
            fn read(buf: &[u8]) -> Result<Self> {
                <[u8; $len]>::read(buf).map($name)
            }
        }

        impl FixedRead for $name {
            #[inline(always)]
            fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
                let mut out = Self::zeroed();
                let written = read(out.as_mut())?;
                if written == $len {
                    Ok(out)
                } else {
                    Err(HookError::TooSmall)
                }
            }
        }
    };
}

fixed_bytes_type!(
    /// A 20-byte AccountID.
    AccountId,
    ACC_ID_LEN,
    crate::buf_eq::buf_eq_20
);
fixed_bytes_type!(
    /// A 32-byte hash (transaction ID, ledger hash, ...).
    Hash,
    HASH_LEN,
    crate::buf_eq::buf_eq_32
);
fixed_bytes_type!(
    /// A 34-byte Keylet.
    Keylet,
    KEYLET_LEN,
    crate::buf_eq::buf_eq_34
);
fixed_bytes_type!(
    /// A 32-byte hook state key.
    StateKey,
    STATE_KEY_LEN,
    crate::buf_eq::buf_eq_32
);
fixed_bytes_type!(
    /// A 32-byte hook state namespace.
    NameSpace,
    NAMESPACE_LEN,
    crate::buf_eq::buf_eq_32
);
fixed_bytes_type!(
    /// A 32-byte nonce.
    Nonce,
    NONCE_LEN,
    crate::buf_eq::buf_eq_32
);
fixed_bytes_type!(
    /// A 33-byte public key.
    PublicKey,
    PUB_KEY_LEN,
    crate::buf_eq::buf_eq_33
);
fixed_bytes_type!(
    /// A 20-byte currency code.
    ///
    /// Two on-ledger encodings share this 20-byte slot:
    ///
    /// - **Standard 3-character codes** (`USD`, `EUR`, ...) — twelve zero
    ///   bytes, the three ASCII characters, then five trailing zeros.
    ///   Construct with [`CurrencyCode::from_iso`]:
    ///   `const USD: CurrencyCode = CurrencyCode::from_iso(b"USD");`.
    ///   The argument is `&[u8; 3]`, so a byte string of any other length
    ///   (`b"US"`, `b"USDT"`) is a type error, not a runtime check.
    /// - **Non-standard 160-bit codes** — any other 20-byte pattern.
    ///   Construct with the tuple field:
    ///   `CurrencyCode([0xAB; CURRENCY_CODE_LEN])`.
    ///
    /// Native XRP/XAH is not a `CurrencyCode`; it is a native amount, not
    /// `from_iso(b"XRP")`.
    CurrencyCode,
    CURRENCY_CODE_LEN,
    crate::buf_eq::buf_eq_20
);

impl CurrencyCode {
    /// Encode a standard 3-character currency code into the 20-byte wire
    /// form: twelve zero bytes, the three ASCII bytes, then five trailing
    /// zeros.
    ///
    /// `code` is `&[u8; 3]`, so the length is part of the type — a 3-byte
    /// literal `b"USD"` compiles, a 2- or 4-byte literal does not. Usable
    /// in `const`/`static` initializers. Non-standard 160-bit currencies
    /// still go through the [`CurrencyCode`]`( [u8; 20] )` tuple
    /// constructor.
    ///
    /// # Examples
    ///
    /// ```
    /// use rshooks::types::CurrencyCode;
    ///
    /// const USD: CurrencyCode = CurrencyCode::from_iso(b"USD");
    /// assert_eq!(
    ///     USD.0,
    ///     [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0],
    /// );
    /// ```
    ///
    /// A 2-byte literal is a type error:
    /// ```compile_fail
    /// const _: rshooks::types::CurrencyCode =
    ///     rshooks::types::CurrencyCode::from_iso(b"US");
    /// ```
    ///
    /// A 4-byte literal is a type error:
    /// ```compile_fail
    /// const _: rshooks::types::CurrencyCode =
    ///     rshooks::types::CurrencyCode::from_iso(b"USDT");
    /// ```
    #[inline(always)]
    #[must_use]
    pub const fn from_iso(code: &[u8; 3]) -> Self {
        let [a, b, c] = *code;
        Self([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, a, b, c, 0, 0, 0, 0, 0])
    }
}
fixed_bytes_type!(
    /// An 8-byte serialized native (XRP/XAH) amount.
    NativeAmount,
    NATIVE_AMOUNT_LEN,
    crate::buf_eq::buf_eq_8
);
fixed_bytes_type!(
    /// A 48-byte serialized IOU amount.
    IouAmount,
    IOU_AMOUNT_LEN,
    crate::buf_eq::buf_eq_48
);

// ---------------------------------------------------------------------------
// IssuedAsset: the (currency, issuer) identity of a non-native amount
// ---------------------------------------------------------------------------

/// A currency/issuer pair identifying an issued (non-native) asset.
///
/// Distinct from [`Issue`] — that is a wire-type marker for navigating a
/// serialized `Issue` field through [`crate::slot_obj`], not a value.
/// `IssuedAsset` is the decoded identity an application actually compares
/// and stores: [`IouAmount::asset`] produces one from a 48-byte IOU
/// `Amount`, and [`crate::slot_obj::IssueData::Iou`] holds one when a
/// 40-byte IOU `Issue` is decoded through an `Issue`-typed slot. Represents
/// only asset identity: no trust-line, authorization, or freeze state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IssuedAsset {
    /// The asset's currency code.
    pub currency: CurrencyCode,
    /// The asset's issuing account.
    pub issuer: AccountId,
}

impl IouAmount {
    /// Borrows the currency-code component (bytes 8..28 of the 48-byte
    /// layout: 8-byte value, 20-byte currency, 20-byte issuer) without
    /// copying. `None` only if `self` is not that standard layout, which
    /// cannot happen through this crate's own decoders — every `IouAmount`
    /// they produce is exactly 48 bytes.
    #[inline(always)]
    fn currency_ref(&self) -> Option<&[u8; CURRENCY_CODE_LEN]> {
        self.0.get(8..28)?.try_into().ok()
    }

    /// Borrows the issuer component (bytes 28..48). See
    /// [`Self::currency_ref`].
    #[inline(always)]
    fn issuer_ref(&self) -> Option<&[u8; ACC_ID_LEN]> {
        self.0.get(28..48)?.try_into().ok()
    }

    /// The currency code component.
    #[inline(always)]
    #[must_use]
    pub fn currency(&self) -> CurrencyCode {
        CurrencyCode(
            self.currency_ref()
                .copied()
                .unwrap_or([0u8; CURRENCY_CODE_LEN]),
        )
    }

    /// The issuing account component.
    #[inline(always)]
    #[must_use]
    pub fn issuer(&self) -> AccountId {
        AccountId(self.issuer_ref().copied().unwrap_or([0u8; ACC_ID_LEN]))
    }

    /// The (currency, issuer) identity this amount is denominated in.
    #[inline(always)]
    #[must_use]
    pub fn asset(&self) -> IssuedAsset {
        IssuedAsset {
            currency: self.currency(),
            issuer: self.issuer(),
        }
    }

    /// Whether this amount's currency and issuer match `asset`.
    ///
    /// Compares directly against the wire bytes via [`crate::buf_eq`] —
    /// unlike [`Self::asset`], this never constructs an intermediate
    /// [`IssuedAsset`] or copies the 40-byte currency+issuer pair.
    #[inline(always)]
    #[must_use]
    pub fn matches_asset(&self, asset: &IssuedAsset) -> bool {
        let currency_eq = self
            .currency_ref()
            .is_some_and(|c| crate::buf_eq::buf_eq_20(c, &asset.currency.0));
        let issuer_eq = self
            .issuer_ref()
            .is_some_and(|i| crate::buf_eq::buf_eq_20(i, &asset.issuer.0));
        currency_eq && issuer_eq
    }
}

// Loop-free via `buf_cmp_20` — see the module doc comment for the ordering
// this gives. `<=`/`>=` come free from `Ord`'s default methods.
impl Ord for AccountId {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        crate::buf_eq::buf_cmp_20(&self.0, &other.0)
    }
}

impl PartialOrd for AccountId {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------
// Serialized wire-type markers and typed field codes
// ---------------------------------------------------------------------------
//
// These describe what a *serialized field* holds, which is a property of the
// wire format rather than of any one API that reads it. They live here, next
// to the fixed-size buffer newtypes, so the generated `crate::sfield` table
// depends only on this module: a field constant should not have to know that
// a slot layer exists. `crate::slot_obj` imports them.

/// Wire-type marker: an `STObject` — a ledger object, a transaction, or
/// any nested object field. Navigable by [`SField`] in
/// [`crate::slot_obj`]; has no `value()` of its own (read its fields, or use
/// the raw byte escapes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct STObject;

/// Wire-type marker: an `STArray`. Navigable by `u32` index, and
/// [`crate::slot_obj::SlotObject::count`] reports its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct STArray;

/// Wire-type marker: a serialized `Amount` — 8 bytes native or 48 bytes
/// IOU. See [`crate::slot_obj::SlotObject::as_xfl`] and the `value()` family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount;

/// Wire-type marker: a serialized `Issue` — 20 bytes native or 40 bytes
/// IOU (currency + issuer). Navigates a slot field through
/// [`crate::slot_obj`]; reading it decodes to
/// [`crate::slot_obj::IssueData`] (`Native` or `Iou(`[`IssuedAsset`]`)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Issue;

/// Wire-type marker: a field this crate models no typed read for — a
/// `Blob`, a `PathSet`, a `Hash160` (whose fields mean different things),
/// or a slot whose type was never established. Still fully usable through
/// [`crate::slot_obj`]: navigable by either key kind (the host decides at
/// runtime whether the operation makes sense) and readable through the raw
/// byte escapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opaque;

// ---------------------------------------------------------------------------
// SField<T>
// ---------------------------------------------------------------------------

/// A serialized field code that remembers what its value reads back as.
///
/// The generated constants in [`crate::sfield`] are the ones you use
/// (`sfSequence`, `sfAccount`, ...); this type is what they are. `T` is
/// phantom — an `SField<u32>` is a plain `u32` code at runtime, with the
/// value type carried entirely at compile time.
///
/// `PhantomData<fn() -> T>` rather than `PhantomData<T>` so the field
/// constant is `Send`/`Sync`/`Copy` regardless of what `T` is: the field
/// *produces* a `T`, it does not hold one.
#[derive(Debug)]
pub struct SField<T> {
    code: u32,
    _t: PhantomData<fn() -> T>,
}

// Hand-written rather than derived: `#[derive(Copy)]` on a generic struct
// adds a `T: Copy` bound, which would make `SField<STObject>` non-`Copy` for
// no reason — the phantom carries no data.
impl<T> Clone for SField<T> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for SField<T> {}

// Comparison is on the code alone, across type parameters — hand-written
// rather than derived (which would compare only same-`T` and carry a
// needless `T: PartialEq` bound), so an erased field code (what
// `SlotObject::field_code` hands back, an `SField<Opaque>`) compares
// directly against any generated constant in either direction:
// `slot.field_code()? == sfBalance`. Two `SField`s with the same code
// always name the same field, so equality ignoring `T` is the right
// relation — `T` records how the value reads back, not which field it is.
impl<A, B> PartialEq<SField<B>> for SField<A> {
    #[inline(always)]
    fn eq(&self, other: &SField<B>) -> bool {
        self.code == other.code
    }
}

impl<T> Eq for SField<T> {}

impl<T> SField<T> {
    /// Wraps a raw field code. Called only by the generated constant table
    /// in [`crate::sfield`].
    ///
    /// **`pub(crate)` on purpose.** A public constructor would let safe
    /// downstream code forge any code/type pair it liked — spelling a
    /// 20-byte currency field as an `SField<AccountId>` and having
    /// `.value()` hand back an `AccountId` built from currency bytes,
    /// bypassing both the generated mapping and the deliberate
    /// [`crate::slot_obj::SlotObject::assume_type`] escape. Reading a field as something it
    /// is not stays possible, but only by saying so.
    #[inline(always)]
    #[must_use]
    pub(crate) const fn new(code: u32) -> Self {
        Self {
            code,
            _t: PhantomData,
        }
    }

    /// The raw `u32` field code.
    ///
    /// `const`, and the reason it exists: `From`/`Into` are not usable in
    /// const contexts, so anywhere a raw code must be a compile-time
    /// constant — [`crate::txn_template!`]'s generated field table, a `match`
    /// arm — this is the bridge. Runtime call sites take `impl Into<u32>`
    /// and need nothing, and [`crate::txn::codec`]'s `const fn`s take the
    /// `SField` itself, so writing `.code()` there is not required either.
    ///
    /// ```
    /// use rshooks::prelude::*;
    ///
    /// const SEQUENCE: u32 = sfSequence.code();
    /// assert_eq!(SEQUENCE, rshooks::raw::sfcodes::sfSequence);
    /// ```
    #[inline(always)]
    #[must_use]
    pub const fn code(self) -> u32 {
        self.code
    }
}

impl<T> From<SField<T>> for u32 {
    #[inline(always)]
    fn from(f: SField<T>) -> u32 {
        f.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deref_reaches_inner_array_methods() {
        let id = AccountId([0xAB; ACC_ID_LEN]);
        assert_eq!(id.len(), ACC_ID_LEN);
        assert!(id.starts_with(&[0xAB]));
    }

    #[test]
    fn as_ref_as_mut_round_trip() {
        let mut id = AccountId::default();
        id.as_mut().copy_from_slice(&[7u8; ACC_ID_LEN]);
        assert_eq!(id.as_ref(), &[7u8; ACC_ID_LEN]);
    }

    #[test]
    fn from_into_array_round_trip() {
        let arr = [9u8; ACC_ID_LEN];
        let id = AccountId::from(arr);
        let back: [u8; ACC_ID_LEN] = id.into();
        assert_eq!(back, arr);
    }

    #[test]
    fn default_is_all_zero() {
        assert_eq!(AccountId::default(), AccountId([0u8; ACC_ID_LEN]));
    }

    #[test]
    fn zeroed_is_all_zero_and_matches_default() {
        assert_eq!(AccountId::zeroed(), AccountId([0u8; ACC_ID_LEN]));
        assert_eq!(AccountId::zeroed(), AccountId::default());
    }

    // `zeroed()` must be usable in a `const`/`static` initializer — the
    // whole reason it exists alongside `Default` (which cannot be `const`
    // on stable Rust). A `const`/`static` that fails to compile would fail
    // the crate build, not this test, but keeping one here documents the
    // guarantee at the call site closest to it.
    const _CONST_ZEROED: AccountId = AccountId::zeroed();
    static _STATIC_ZEROED: Hash = Hash::zeroed();

    // `from_iso` must be usable in a `const`/`static` initializer — the
    // point of taking `&[u8; 3]` rather than a runtime `&[u8]`.
    const _CONST_USD: CurrencyCode = CurrencyCode::from_iso(b"USD");
    static _STATIC_EUR: CurrencyCode = CurrencyCode::from_iso(b"EUR");

    #[test]
    fn zeroed_works_in_const_and_static_context() {
        assert_eq!(_CONST_ZEROED, AccountId([0u8; ACC_ID_LEN]));
        assert_eq!(_STATIC_ZEROED, Hash([0u8; HASH_LEN]));
    }

    #[test]
    fn from_iso_encodes_three_ascii_bytes_at_offset_12() {
        assert_eq!(
            _CONST_USD.0,
            [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0
            ],
        );
        assert_eq!(
            _STATIC_EUR.0,
            [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'E', b'U', b'R', 0, 0, 0, 0, 0
            ],
        );
    }

    #[test]
    fn from_iso_does_not_change_the_20_byte_tuple_constructor() {
        let nonstandard = CurrencyCode([0x11; CURRENCY_CODE_LEN]);
        assert_eq!(nonstandard.0, [0x11; CURRENCY_CODE_LEN]);
        assert_ne!(nonstandard, CurrencyCode::from_iso(b"USD"));
    }

    #[test]
    fn fixed_read_succeeds_on_exact_write() {
        let result: Result<AccountId> = AccountId::read_exact(|buf| {
            buf.copy_from_slice(&[7u8; ACC_ID_LEN]);
            Ok(ACC_ID_LEN)
        });
        assert_eq!(result, Ok(AccountId([7u8; ACC_ID_LEN])));
    }

    #[test]
    fn fixed_read_passes_a_buffer_of_exactly_this_types_length() {
        let _: Result<AccountId> = AccountId::read_exact(|buf| {
            assert_eq!(buf.len(), ACC_ID_LEN);
            Ok(buf.len())
        });
    }

    #[test]
    fn fixed_read_rejects_short_write() {
        let result: Result<AccountId> = AccountId::read_exact(|_buf| Ok(ACC_ID_LEN - 1));
        assert_eq!(result, Err(HookError::TooSmall));
    }

    #[test]
    fn to_bytes_from_bytes_round_trip() {
        let id = AccountId([5u8; ACC_ID_LEN]);
        let mut buf = [0u8; ACC_ID_LEN];
        assert_eq!(id.write(&mut buf), ACC_ID_LEN);
        assert_eq!(AccountId::read(&buf), Ok(id));
    }

    // An erased field code — what `SlotObject::field_code` hands back —
    // compares equal to the generated constant naming the same field, in
    // both directions, even though the two disagree about `T`.
    #[test]
    fn sfield_equality_ignores_the_type_parameter() {
        let erased: SField<Opaque> = SField::new(crate::sfield::sfAccount.code());
        assert!(erased == crate::sfield::sfAccount);
        assert!(crate::sfield::sfAccount == erased);
        assert!(erased != crate::sfield::sfBalance);
    }

    #[test]
    fn account_id_ordering_agrees_with_inner_array_ordering() {
        let low = AccountId([0x10u8; ACC_ID_LEN]);
        let high = AccountId([0x20u8; ACC_ID_LEN]);
        assert_eq!(low.cmp(&high), low.0.cmp(&high.0));
        assert_eq!(high.cmp(&low), high.0.cmp(&low.0));
        assert_eq!(low.cmp(&low), low.0.cmp(&low.0));
    }

    #[test]
    fn account_id_high_low_pair() {
        let low = AccountId([0x01; ACC_ID_LEN]);
        let high = AccountId([0x02; ACC_ID_LEN]);

        assert!(low < high);
        assert!(high > low);
        assert!(low == low);
        assert!(low != high);
    }

    #[test]
    fn account_id_cmp_agrees_with_partial_cmp() {
        let a = AccountId([0x33; ACC_ID_LEN]);
        let b = AccountId([0x44; ACC_ID_LEN]);
        assert_eq!(Some(a.cmp(&b)), a.partial_cmp(&b));
        assert_eq!(Some(b.cmp(&a)), b.partial_cmp(&a));
        assert_eq!(Some(a.cmp(&a)), a.partial_cmp(&a));
    }

    /// A non-uniform pair — the leading byte and the trailing byte disagree
    /// on direction — so this only agrees with the inner-array ordering (and
    /// only picks the right answer) if the leading byte is compared first,
    /// same as [`crate::buf_eq::buf_cmp_20`]'s own such test.
    #[test]
    fn account_id_ordering_first_differing_byte_wins() {
        let mut a = [0x5Au8; ACC_ID_LEN];
        let mut b = [0x5Au8; ACC_ID_LEN];
        a[0] = 0x10;
        b[0] = 0x20;
        a[ACC_ID_LEN - 1] = 0xFF;
        b[ACC_ID_LEN - 1] = 0x00;

        let low = AccountId(a);
        let high = AccountId(b);
        assert_eq!(low.cmp(&high), a.cmp(&b));
        assert_eq!(low.cmp(&high), core::cmp::Ordering::Less);
        assert!(low < high);
        assert!(high > low);
    }

    #[test]
    fn repr_transparent_matches_inner_array_size() {
        assert_eq!(core::mem::size_of::<AccountId>(), ACC_ID_LEN);
        assert_eq!(
            core::mem::align_of::<AccountId>(),
            core::mem::align_of::<[u8; ACC_ID_LEN]>()
        );
    }

    /// Builds a 48-byte `IouAmount` with a distinctive 8-byte value
    /// component, currency, and issuer, so extraction tests can tell the
    /// three components apart. `.get_mut(..)` throughout, never `[..]` —
    /// this workspace denies `clippy::indexing_slicing`, tests included.
    fn sample_iou_amount(currency_fill: u8, issuer_fill: u8) -> IouAmount {
        // Literal bounds, not `NATIVE_AMOUNT_LEN + CURRENCY_CODE_LEN` —
        // this workspace warns on `clippy::arithmetic_side_effects` and
        // `mise run lint` denies warnings.
        let mut bytes = [0u8; IOU_AMOUNT_LEN];
        if let Some(value) = bytes.get_mut(..8) {
            value.copy_from_slice(&[0xAAu8; NATIVE_AMOUNT_LEN]);
        }
        if let Some(currency) = bytes.get_mut(8..28) {
            currency.fill(currency_fill);
        }
        if let Some(issuer) = bytes.get_mut(28..) {
            issuer.fill(issuer_fill);
        }
        IouAmount(bytes)
    }

    #[test]
    fn iou_amount_currency_issuer_and_asset_extraction() {
        let amount = sample_iou_amount(0x11, 0x22);
        assert_eq!(amount.currency(), CurrencyCode([0x11; CURRENCY_CODE_LEN]));
        assert_eq!(amount.issuer(), AccountId([0x22; ACC_ID_LEN]));
        assert_eq!(
            amount.asset(),
            IssuedAsset {
                currency: CurrencyCode([0x11; CURRENCY_CODE_LEN]),
                issuer: AccountId([0x22; ACC_ID_LEN]),
            }
        );
    }

    #[test]
    fn matches_asset_true_for_identical_currency_and_issuer() {
        let amount = sample_iou_amount(0x33, 0x44);
        let asset = IssuedAsset {
            currency: CurrencyCode([0x33; CURRENCY_CODE_LEN]),
            issuer: AccountId([0x44; ACC_ID_LEN]),
        };
        assert!(amount.matches_asset(&asset));
    }

    #[test]
    fn matches_asset_false_for_different_currency() {
        let amount = sample_iou_amount(0x33, 0x44);
        let asset = IssuedAsset {
            currency: CurrencyCode([0x99; CURRENCY_CODE_LEN]), // differs
            issuer: AccountId([0x44; ACC_ID_LEN]),
        };
        assert!(!amount.matches_asset(&asset));
    }

    #[test]
    fn matches_asset_false_for_different_issuer() {
        let amount = sample_iou_amount(0x33, 0x44);
        let asset = IssuedAsset {
            currency: CurrencyCode([0x33; CURRENCY_CODE_LEN]),
            issuer: AccountId([0x99; ACC_ID_LEN]), // differs
        };
        assert!(!amount.matches_asset(&asset));
    }

    #[test]
    fn matches_asset_ignores_the_value_component() {
        // Two amounts with the same asset but different values must both
        // match — `matches_asset` compares identity, not magnitude.
        let low = sample_iou_amount(0x55, 0x66);
        let mut bytes = low.0;
        if let Some(value) = bytes.get_mut(..NATIVE_AMOUNT_LEN) {
            value.copy_from_slice(&[0x01u8; NATIVE_AMOUNT_LEN]);
        }
        let high = IouAmount(bytes);
        let asset = IssuedAsset {
            currency: CurrencyCode([0x55; CURRENCY_CODE_LEN]),
            issuer: AccountId([0x66; ACC_ID_LEN]),
        };
        assert!(low.matches_asset(&asset));
        assert!(high.matches_asset(&asset));
    }
}
