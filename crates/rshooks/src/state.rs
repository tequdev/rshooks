//! Typed hook state: [`state_get`]/[`state_set_loose`]/[`state_update_loose`]
//! (and their `_foreign` twins) plus [`state_delete`], built over
//! [`mod@crate::api::state`]'s raw
//! caller-buffer functions and the [`crate::convert::ToBytes`]/
//! [`crate::convert::FromBytes`] traits, plus the
//! [`state_keys!`](crate::state_keys) macro for declaring a state-key enum.
//!
//! # This layer vs. `crate::api::state`'s single-value helpers
//!
//! [`mod@crate::api::state`] also has a `state_u32`/`state_i64`/`state_xfl`/
//! `state_update_u64`/... family: fixed-shape wrappers over
//! [`crate::api::state::state_exact`] for the primitive integer/
//! [`crate::xfl::XFL`] cases, each taking a raw `&[u8]` key. This module's
//! [`state_get`]/[`state_set_loose`]/[`state_update_loose`] instead work for
//! *any* type implementing [`crate::convert::ToBytes`]/
//! [`crate::convert::FromBytes`] (every `rshooks::types` newtype does, as
//! can hook-defined types), meant to be paired with
//! [`state_keys!`](crate::state_keys) so the key is a typed enum variant
//! rather than a hand-built byte buffer. Reach for `crate::api::state`'s
//! helpers for a one-off primitive read/write; reach for this module when a
//! hook has more than a couple of distinct state entries and wants the key
//! space and value decoding both checked at compile time.
//!
//! # Why `Ok(None)` for a missing entry
//!
//! [`crate::error::HookError::DoesntExist`] (`state`'s `-5`, "no entry for
//! this key") maps to `Ok(None)` rather than `Err`, the same shape as
//! `HashMap::get`/`BTreeMap::get` — "absent" is ordinary, not exceptional.
//! Every *other* error — including a present-but-undersized entry that fails
//! to decode as `T` — still comes back through `Err`, so a decode failure is
//! never mistaken for "nothing was ever stored here."
//!
//! # `state_keys!`
//!
//! Declares an enum whose variants encode to their own **real** byte length
//! (`<= `[`crate::types::STATE_KEY_LEN`]`, 32) — see "Key length and padding"
//! below for why this is a real length, not always-32-bytes — for use with
//! the functions above:
//!
//! ```
//! use rshooks::prelude::*;
//! use rshooks::state_keys;
//!
//! state_keys! {
//!     /// This hook's persistent data.
//!     enum DataKey {
//!         /// A running counter.
//!         Counter,
//!         /// A per-owner balance, keyed by the owner's account.
//!         Balance(AccountId),
//!     }
//! }
//!
//! // `NotImplemented` here is the host stub every Hook API call returns on
//! // a host build (see `rshooks-core`) — this only proves the generated
//! // `encode()`/typed-storage call chain compiles and runs.
//! assert_eq!(
//!     state_get::<u64>(&DataKey::Counter),
//!     Err(HookError::NotImplemented)
//! );
//! ```
//!
//! Unit variants (`Counter` above) encode to just their 1-byte discriminant
//! — no padding at all — entirely at compile time. Tuple variants
//! (`Balance` above) carry exactly one [`crate::convert::ToBytes`] payload,
//! encoded at runtime as "discriminant byte + payload," with **no trailing
//! padding**: the real, sent-to-the-host length is `1 +
//! Payload::MAX_LEN`. The macro rejects (at compile time) a payload whose
//! [`crate::convert::ToBytes::MAX_LEN`] does not leave room for the
//! discriminant byte in the 32-byte key.
//!
//! # Key length and padding: rshooks sends the real length, the host pads
//!
//! See also DESIGN.md §5.7 ("Hook state key encoding: real length, not
//! local zero-padding") for the external, table-based write-up this
//! section summarizes.
//!
//! A hook-state key need not be a full 32 bytes — the Hook API itself
//! accepts any key from 1 to 32 bytes (`state`/`state_set`/`state_foreign(_set)`'s
//! `kread_len`), and **left**-zero-pads a shorter key internally to its own
//! fixed-width storage slot. This is exactly the C hook idiom
//! `state(&v, 8, "RR", 2)` — a 2-byte literal key, unpadded, handed straight
//! to the host.
//!
//! [`StateKeyEncode::encode`] reflects this: it returns an
//! [`EncodedStateKey`] carrying **exactly `self`'s own natural length** —
//! never locally zero-padded up to 32 bytes. This module does not
//! right-pad a short key the way [`crate::pad!`] does for other uses (see
//! that macro's doc comment) — doing so here would silently point at a
//! *different* state slot than the host's own left-pad convention (or than
//! a C hook passing the same short key directly) would reach, and would
//! also make "how many of these 32 bytes does this key actually use" opaque
//! at the call site. Every [`StateKeyEncode`] impl in this crate — `[u8; N]`
//! (`1 <= N <= `[`crate::types::STATE_KEY_LEN`]), [`crate::HookKey`]-derived
//! structs, and `state_keys!` enums — follows this: the bytes handed to the
//! host are exactly as many as the key's own encoding actually needs, and
//! the host is trusted to left-pad. [`crate::types::StateKey`] (a full,
//! already-32-byte key) is the one exception with nothing to shorten: it
//! passes all 32 bytes through unchanged.
//!
//! # Struct keys (`#[derive(crate::HookKey)]`) vs. `state_keys!`
//!
//! [`state_keys!`](crate::state_keys) suits a **small, fixed set** of
//! distinct state entries: every variant is a separate, independently named
//! case (`Counter`, `Balance(AccountId)`, ...), each carrying at most one
//! [`crate::convert::ToBytes`] payload. A key that is itself a **composite of
//! several fields** — a tag byte plus an `AccountId` plus a `u32` sequence
//! number, say — does not fit that shape because a tuple variant takes one
//! payload.
//!
//! [`crate::HookKey`] closes that gap: derive it on an ordinary named-field
//! struct (every field a fixed-size type — see its doc comment for the exact
//! grammar), and the struct becomes directly usable as a `state_get`/
//! `state_set_loose` key, via the [`StateKeyEncode`] impl it generates —
//! `state_get(&MyKey { .. })` works with no `state_keys!` declaration at all.
//! The two are complementary, not competing: `state_keys!` for a handful of
//! named, independent key *cases*; a `#[derive(HookKey)]` struct for one key
//! shape built out of several *fields* — and nothing stops a `state_keys!`
//! tuple variant's single payload from itself being a `HookKey`-derived
//! struct, for a hybrid of both. See [`crate::HookKey`]'s doc comment for why
//! this is a *separate* derive from [`crate::HookData`] (the state-*value*
//! counterpart) rather than one derive serving both roles.
//!
//! Only a [`crate::HookKey`]-derived struct, a
//! [`state_keys!`](crate::state_keys) enum, or [`crate::types::StateKey`]
//! itself implements [`StateKeyEncode`] — an ordinary
//! [`crate::convert::ToBytes`] type (a plain `#[derive(HookData)]` value
//! struct included) does **not** automatically qualify, so a state *value*
//! type can never be passed where a key is expected by accident.
//! [`crate::HookKey`]'s derive checks at compile time that the struct fits
//! the 32-byte key space, rather than silently truncating.
//!
//! # Pairing a key with its value type: [`TypedStateKey`]
//!
//! [`state_get`]/[`state_set_loose`]/[`state_update_loose`] take the key and
//! the value type `T` as *independent* generic parameters — nothing at the
//! type level stops calling `state_get::<WrongValue>(&key)` for a
//! `key`/`WrongValue` combination that was never meant to go together, as
//! long as `WrongValue: FromBytes`. [`TypedStateKey`] closes that gap:
//! implement it for a key type to declare its one paired value type, then
//! use [`state_get_typed`]/[`state_set_typed`]/[`state_update_typed`] (+
//! `_foreign` twins) — these read `K::Value` off the key's own type, so a
//! mismatched value type has no generic parameter left to hide in; it's a
//! compile error instead of a latent bug. Prefer these whenever a key type
//! only ever pairs with one value type (every `HookKey` key in practice).
//! [`state_delete`] completes the set: it takes only a key (there is no
//! value to type-check) — see its own doc comment.
//!
//! A `#[hooks]`-declared [`crate::State`] field gets the same typed pairing
//! for free, plus `.get()`/`.set()`/`.update()`/`.delete()` accessors
//! generated for that field — see [`crate::decl`]'s module doc comment.
//!
//! # Relationship to the `hook_param`/`otxn_param` typed layer
//!
//! [`crate::convert::TypedParamName`] is this module's counterpart for Hook
//! API parameters:
//!
//! | | this module (hook state) | [`crate::convert::TypedParamName`] (params) |
//! |---|---|---|
//! | declare the pairing | `impl TypedStateKey for Key { type Value = Ty; }` | `impl TypedParamName for Name { type Value = Ty; }` |
//! | safe accessor(s) | `state_get_typed`/`state_set_typed`/`state_update_typed`/`state_delete` | `hook_param_typed`/`otxn_param_typed` |
//! | loose escape hatch | [`state_get`]/[`state_set_loose`]/[`state_update_loose`] (independent `T`) | `hook_param_exact`/`otxn_param_exact` (independent `T`) |
//! | shared foundation | [`crate::convert::ToBytes`]/[`crate::convert::FromBytes`] via [`crate::HookKey`]/[`crate::HookData`] | via [`crate::ParamName`]/[`crate::ParamValue`] |
//!
//! Both follow the same shape: declare a pairing once, then call an accessor
//! that takes a reference to a key/name value and resolves the paired type
//! from it — no turbofish; the argument, not the call site's inferred return
//! type, picks `Value`. The one real mechanism difference: a
//! `hook_param`/`otxn_param` is read-only from the reading hook's own
//! perspective (`hook_param_set` writes a *different* hook's parameter, not
//! this one), so `TypedParamName` only ever has a "get" shape, never
//! "set"/"update"/"delete".
//!
//! # Endianness
//!
//! Every value encoded/decoded through this module's [`crate::convert::ToBytes`]/
//! [`crate::convert::FromBytes`] traits is little-endian — see
//! [`crate::convert`]'s module doc comment and DESIGN.md §5.6 ("Endianness
//! conventions") for the full two-world rule this is one half of, and
//! [`crate::api::state::state_u64`] for the big-endian counterpart this
//! module's typed layer deliberately does not use.

use crate::convert::{FromBytes, ToBytes};
use crate::error::{HookError, Result, res};
use crate::types::{STATE_KEY_LEN, StateKey};

/// Forms a `&mut [u8]` view over `buf`'s storage without requiring it to be
/// initialized first — lets [`state_get_encoded`]/[`state_foreign_get_encoded`]
/// hand the `state`/`state_foreign` host calls a scratch buffer without
/// zero-initializing it, since the host always fully determines which
/// prefix of it [`decode_read`] ever reads.
///
/// # Safety
///
/// The returned slice must only ever be read over the range a prior write
/// into it actually covered — here, no further than [`decode_read`]'s own
/// `n = res(code)?` prefix. `decode_read` enforces that bound itself:
/// `raw.get(..n).ok_or(HookError::TooSmall)?` errors rather than reading
/// past the buffer if the host ever reports a larger `n` than the buffer
/// holds. What remains is the FFI trust boundary common to this whole
/// crate — the host must actually have written the `n` bytes it reports,
/// since [`crate::api::state::state_raw_code`]/
/// [`crate::api::state::state_foreign_raw_code`] are `unsafe` `extern` calls
/// already fully trusted by every other line here.
///
/// No bit pattern is invalid for `u8`, so forming the `&mut [u8]` here is
/// the standard pre-`BorrowedBuf` I/O shape over `MaybeUninit` storage —
/// only reading through it before it is written would be unsound, a
/// pattern this crate relies on throughout rather than one the language
/// unconditionally guarantees.
#[inline(always)]
unsafe fn uninit_slice_mut<const N: usize>(buf: &mut core::mem::MaybeUninit<[u8; N]>) -> &mut [u8] {
    // SAFETY: `buf` is `N` bytes of live, properly aligned storage (a
    // `MaybeUninit<[u8; N]>` has the same size and alignment as `[u8; N]`).
    // `u8` has no invalid bit patterns and no padding, so a `&mut [u8]` over
    // that storage is well-formed the instant it is created, whether or not
    // the storage has been written to yet — only reading through it before
    // it is written would be unsound, and this function's own safety
    // contract puts that burden on the caller.
    unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), N) }
}

/// Maximum byte length of any value [`state_get`]/[`state_set_loose`]/
/// [`state_update_loose`] (and their `_foreign` twins) read or write.
///
/// 32, **not** picked to fit the largest type this crate provides
/// ([`crate::types::IouAmount`] is 48 bytes and does not fit) — picked
/// because it is the largest local `[0u8; N]` zero-init this toolchain's
/// wasm32v1-none codegen still lowers to a handful of inlined stores at
/// this crate's release profile (`opt-level = "z"`, `lto = "fat"`). Beyond
/// it (empirically, 34 bytes and up), rustc instead emits a call to the
/// shared `memset` builtin — a real, unguarded wasm `loop` that the Hook
/// API's guard checker rejects (see DESIGN.md §2 C2). Covers every
/// fixed-size type this crate provides up to [`crate::types::NameSpace`]/
/// [`crate::types::Nonce`]/[`crate::types::StateKey`]/
/// [`crate::types::Hash`] (32 bytes); a hook that needs a bigger typed
/// value — [`crate::types::PublicKey`] (33), [`crate::types::Keylet`] (34),
/// [`crate::types::IouAmount`] (48), or a custom type — should call
/// [`crate::api::state`]'s raw, caller-buffer functions directly instead of
/// this module.
const MAX_TYPED_STATE_LEN: usize = 32;

/// Encodes a value into hook-state key bytes: its own real length, `<= `
/// [`crate::types::STATE_KEY_LEN`] (32) — never locally zero-padded up to
/// 32 bytes. See the module doc comment's "Key length and padding" section
/// for why.
///
/// Implemented by `[u8; N]` (`1 <= N <= `[`crate::types::STATE_KEY_LEN`],
/// checked at compile time), by every enum the
/// [`state_keys!`](crate::state_keys) macro generates, by every
/// [`crate::HookKey`]-derived struct, and — as the one
/// already-32-byte-with-nothing-to-shorten case — by
/// [`crate::types::StateKey`] itself. Deliberately **not** implemented for
/// every [`crate::convert::ToBytes`] type: an ordinary state *value* struct
/// (a plain `#[derive(HookData)]`) has no business also being usable as a
/// key by accident.
pub trait StateKeyEncode {
    /// `self`'s own real-length key encoding — see this trait's doc comment.
    fn encode(&self) -> EncodedStateKey;
}

/// The real-length byte encoding [`StateKeyEncode::encode`] returns: a
/// fixed 32-byte buffer plus the number of leading bytes actually meaningful
/// (`<= `[`crate::types::STATE_KEY_LEN`]). No heap allocation — a plain
/// `Copy` value, as zero-cost as returning a fixed-size array, just paired
/// with the real length instead of always claiming all 32 bytes.
/// [`AsRef<[u8]>`] exposes only that real-length prefix, so passing
/// `&encoded` to [`crate::api::state::state`]/[`crate::api::state::state_set`]/
/// [`crate::api::state::state_foreign`]/[`crate::api::state::state_foreign_set`]
/// sends exactly that many bytes to the host, which left-pads a short key.
#[derive(Clone, Copy, Debug)]
pub struct EncodedStateKey {
    buf: [u8; STATE_KEY_LEN],
    len: usize,
}

impl EncodedStateKey {
    /// Builds an `EncodedStateKey` from a full 32-byte buffer and the
    /// number of leading bytes that are actually meaningful (the rest of
    /// `buf` is ignored). `len` must be `<= `[`crate::types::STATE_KEY_LEN`]
    /// — every call site enforces that at compile time before calling this,
    /// so it is not re-checked here; a `len` beyond `buf`'s bounds simply
    /// yields an empty [`AsRef<[u8]>`] slice rather than a panic, keeping
    /// this constructor infallible.
    #[inline(always)]
    #[must_use]
    pub const fn new(buf: [u8; STATE_KEY_LEN], len: usize) -> Self {
        Self { buf, len }
    }

    /// Builds an `EncodedStateKey` from a compile-time-sized short key,
    /// usable in a `const` context — the const-evaluable counterpart to the
    /// `[u8; N]` [`StateKeyEncode`] impl below, for callers (the `#[hooks]`
    /// macro's literal `#[state(key = b"...")]` codegen) that need a
    /// `'static`-promotable value rather than a value computed at runtime on
    /// every call. `N` is checked at compile time to be `1..=`
    /// [`crate::types::STATE_KEY_LEN`], the same bound that impl enforces.
    #[inline(always)]
    #[must_use]
    #[allow(clippy::indexing_slicing)] // in-bounds by the assert above; const-evaluated only
    pub const fn from_short<const N: usize>(key: &[u8; N]) -> Self {
        const {
            assert!(
                N >= 1 && N <= STATE_KEY_LEN,
                "rshooks::state: a short state key must be 1..=32 bytes \
                 (the Hook API's own key-length bound)"
            );
        }
        let mut buf = [0u8; STATE_KEY_LEN];
        let mut i = 0;
        while i < N {
            buf[i] = key[i];
            i = i.wrapping_add(1);
        }
        Self { buf, len: N }
    }
}

impl AsRef<[u8]> for EncodedStateKey {
    #[inline(always)]
    fn as_ref(&self) -> &[u8] {
        match self.buf.get(..self.len) {
            Some(s) => s,
            None => &[],
        }
    }
}

/// Identity impl: an already-[`StateKeyEncode::encode`]d key passes through
/// unchanged. Lets a generic `&impl StateKeyEncode` caller pass an
/// `EncodedStateKey` it already holds straight to this module's public free
/// functions without a separate encoding step. [`crate::decl`]'s
/// `StateEntry`/`State::at`-bound accessors — which store a pre-encoded
/// `EncodedStateKey` — skip even this identity copy by calling this
/// module's `_encoded`-suffixed internal funnels directly.
impl StateKeyEncode for EncodedStateKey {
    #[inline(always)]
    fn encode(&self) -> EncodedStateKey {
        *self
    }
}

/// Identity impl: a raw, already-32-byte [`crate::types::StateKey`] passes
/// through unchanged — there is nothing to shorten.
impl StateKeyEncode for StateKey {
    #[inline(always)]
    fn encode(&self) -> EncodedStateKey {
        EncodedStateKey::new(self.0, STATE_KEY_LEN)
    }
}

/// A short, literal state key — the direct Rust counterpart to the C hook
/// idiom of passing a short key straight to `state`/`state_set`
/// (`state(&v, 8, "RR", 2)`): `state_get::<u64>(b"RR")` reaches the exact
/// same, host-left-padded slot. `N` is checked at compile time (a
/// monomorphized `const` assert, one per concrete `N`) to be `1..=`
/// [`crate::types::STATE_KEY_LEN`] — the Hook API's own bound on a key's
/// length — so an out-of-range `N` fails to compile rather than failing at
/// the first host call. A bare `&[u8]` (runtime-determined length) is
/// deliberately not supported — only a compile-time-sized `[u8; N]` — to
/// keep that bound a compile-time guarantee, not a runtime `TOO_SMALL`/
/// `TOO_BIG` a caller could hit unexpectedly.
///
/// # Examples
///
/// ```
/// use rshooks::prelude::*;
///
/// // `NotImplemented` here is the host stub every Hook API call returns on
/// // a host build — this only proves the `[u8; N]` `StateKeyEncode` call
/// // chain compiles and runs.
/// assert_eq!(state_get::<u64>(b"RR"), Err(HookError::NotImplemented));
/// assert_eq!(state_set_loose(b"RR", &1u64), Err(HookError::NotImplemented));
/// ```
///
/// An empty key (`N = 0`) fails to compile — the Hook API's own lower
/// bound is 1 byte:
///
/// ```compile_fail
/// use rshooks::prelude::*;
///
/// let _ = state_get::<u64>(b"");
/// ```
///
/// A key longer than 32 bytes fails to compile:
///
/// ```compile_fail
/// use rshooks::prelude::*;
///
/// let _ = state_get::<u64>(&[0u8; 33]);
/// ```
impl<const N: usize> StateKeyEncode for [u8; N] {
    #[inline(always)]
    fn encode(&self) -> EncodedStateKey {
        const {
            assert!(
                N >= 1 && N <= STATE_KEY_LEN,
                "rshooks::state: a [u8; N] state key must be 1..=32 bytes \
                 (the Hook API's own key-length bound)"
            );
        }
        let mut buf = [0u8; STATE_KEY_LEN];
        if let Some(dst) = buf.get_mut(..N) {
            dst.copy_from_slice(self);
        }
        EncodedStateKey::new(buf, N)
    }
}

/// A [`StateKeyEncode`] key type bound to exactly one value type — see the
/// module doc comment's "Pairing a key with its value type" section.
pub trait TypedStateKey: StateKeyEncode {
    /// The one value type this key is paired with.
    type Value: ToBytes + FromBytes;
}

/// Shared read path for [`state_get`]/[`state_foreign_get`]: turns a
/// **raw, undecoded** `state`/`state_foreign` host-call `i64` result
/// (`code`; bytes written land in `raw`) into a decoded `Result<Option<T>>`,
/// mapping "doesn't exist" to `Ok(None)` (see the module doc comment).
///
/// Takes the raw `code` directly — compared against
/// [`rshooks_core::DOESNT_EXIST`] *before* any [`HookError`] is ever
/// constructed — rather than an already-decoded `Result<usize>`: matching
/// one specific [`HookError`] variant out of an already-decoded value forces
/// the compiler to keep the full ~44-arm [`HookError::from`] decode
/// resolvable at this call site, and to fold that decode's own block nesting
/// into the caller's once inlined into a large hook (measured: a 24→70
/// nesting-depth blowup, over the Hook API's 32-level guard-checker limit,
/// when tried the other way — see DESIGN.md §5.1's "no specific-variant
/// decode inside rshooks" principle). `res(code)` is still called on the one
/// path that needs a full [`HookError`]; its caller only ever propagates
/// that error onward via `?`, so [`HookError::from`]'s decode optimizes away
/// there too.
///
/// `raw` is only ever read over its `..n` prefix (`n = res(code)?`, the
/// host's own reported write count) — so [`state_get_encoded`]/
/// [`state_foreign_get_encoded`] pass a [`core::mem::MaybeUninit`] scratch
/// buffer viewed through [`uninit_slice_mut`] here rather than a
/// zero-initialized one: nothing beyond that prefix is ever touched, so
/// zeroing the rest first would be dead work the guard checker still
/// charges for.
#[inline(always)]
fn decode_read<T: FromBytes>(code: i64, raw: &[u8]) -> Result<Option<T>> {
    if code == rshooks_core::DOESNT_EXIST {
        return Ok(None);
    }
    let n = res(code)? as usize;
    let src = raw.get(..n).ok_or(HookError::TooSmall)?;
    T::read(src).map(Some)
}

/// Shared write path for [`state_set_loose`]/[`state_foreign_set_loose`]:
/// encodes `value` into a [`MAX_TYPED_STATE_LEN`]-byte scratch buffer.
///
/// A compile-time check (monomorphized per `T`) rejects any `T` whose
/// [`ToBytes::MAX_LEN`] does not fit — see [`MAX_TYPED_STATE_LEN`]'s doc
/// comment for the escape hatch. Without this check a too-large `T` would
/// silently encode to `0` bytes (`ToBytes::write`'s documented short-buffer
/// behavior) and write an empty state entry instead of failing loudly.
#[inline(always)]
fn encode_write<T: ToBytes>(value: &T) -> [u8; MAX_TYPED_STATE_LEN] {
    const {
        assert!(
            T::MAX_LEN <= MAX_TYPED_STATE_LEN,
            "rshooks::state: T::MAX_LEN exceeds the typed-storage buffer \
             — use api::state's raw functions directly for larger values"
        );
    }
    let mut raw = [0u8; MAX_TYPED_STATE_LEN];
    let _ = value.write(&mut raw);
    raw
}

/// Read this hook's own state entry for an already-[`EncodedStateKey`]d
/// `key`, decoded as `T`.
///
/// Internal funnel behind [`state_get`]: a caller that already holds an
/// `EncodedStateKey` (`crate::decl`'s `State`/`StateEntry` accessors) calls
/// this directly, skipping the identity [`StateKeyEncode::encode`] copy
/// [`state_get`] would otherwise perform.
#[inline(always)]
pub(crate) fn state_get_encoded<T: FromBytes>(key: &EncodedStateKey) -> Result<Option<T>> {
    let mut storage = core::mem::MaybeUninit::<[u8; MAX_TYPED_STATE_LEN]>::uninit();
    // SAFETY: see `uninit_slice_mut`'s doc comment; `state_raw_code` cannot
    // report writing more bytes than the buffer it was handed, and
    // `decode_read` only ever reads the `..n` prefix that count reports.
    let buf = unsafe { uninit_slice_mut(&mut storage) };
    let code = crate::api::state::state_raw_code(buf, key);
    decode_read(code, buf)
}

/// Read this hook's own state entry for `key`, decoded as `T`.
///
/// `Ok(None)` means no entry exists for `key` — see the module doc comment.
#[inline(always)]
pub fn state_get<T: FromBytes>(key: &impl StateKeyEncode) -> Result<Option<T>> {
    state_get_encoded(&key.encode())
}

/// Read this hook's own state entry for `key`, decoded as `key`'s own
/// [`TypedStateKey::Value`] — the key/value-pairing-safe counterpart to
/// [`state_get`] (see [`TypedStateKey`]'s doc comment for why). `Ok(None)`
/// means no entry exists — see the module doc comment.
#[inline(always)]
pub fn state_get_typed<K: TypedStateKey>(key: &K) -> Result<Option<K::Value>> {
    state_get::<K::Value>(key)
}

/// Write this hook's own state entry for an already-[`EncodedStateKey`]d
/// `key`, encoding `value` as `T`. Internal funnel behind
/// [`state_set_loose`] — see [`state_get_encoded`]'s doc comment for why
/// this exists.
#[inline(always)]
pub(crate) fn state_set_encoded<T: ToBytes>(key: &EncodedStateKey, value: &T) -> Result<usize> {
    let raw = encode_write(value);
    let src = raw.get(..T::MAX_LEN).ok_or(HookError::TooBig)?;
    crate::api::state::state_set(src, key)
}

/// Write this hook's own state entry for `key`, encoding `value` as `T`.
/// Returns the number of bytes written.
#[inline(always)]
pub fn state_set_loose<T: ToBytes>(key: &impl StateKeyEncode, value: &T) -> Result<usize> {
    state_set_encoded(&key.encode(), value)
}

/// Write this hook's own state entry for `key`, encoding `value` as `key`'s
/// own [`TypedStateKey::Value`] — the key/value-pairing-safe counterpart to
/// [`state_set_loose`] (see [`TypedStateKey`]'s doc comment for why):
/// `value`'s type is checked against `K::Value` at the call site, so
/// passing a value meant for a different key is a compile error. Returns
/// the number of bytes written.
#[inline(always)]
pub fn state_set_typed<K: TypedStateKey>(key: &K, value: &K::Value) -> Result<usize> {
    state_set_loose(key, value)
}

/// Read-modify-write this hook's own state entry for an already-
/// [`EncodedStateKey`]d `key`. Internal funnel behind [`state_update_loose`]
/// — see [`state_get_encoded`]'s doc comment for why this exists.
#[inline(always)]
pub(crate) fn state_update_encoded<T, F>(key: &EncodedStateKey, f: F) -> Result<usize>
where
    T: FromBytes + ToBytes,
    F: FnOnce(Option<T>) -> T,
{
    let current = state_get_encoded::<T>(key)?;
    let next = f(current);
    state_set_encoded(key, &next)
}

/// Read-modify-write this hook's own state entry for `key`: reads the
/// current value (or `None` if absent), calls `f` to compute the next
/// value, writes it back, and returns the number of bytes written.
#[inline(always)]
pub fn state_update_loose<T, F>(key: &impl StateKeyEncode, f: F) -> Result<usize>
where
    T: FromBytes + ToBytes,
    F: FnOnce(Option<T>) -> T,
{
    state_update_encoded(&key.encode(), f)
}

/// Read-modify-write this hook's own state entry for `key`, using `key`'s
/// own [`TypedStateKey::Value`] — the key/value-pairing-safe counterpart to
/// [`state_update_loose`] (see [`TypedStateKey`]'s doc comment for why).
#[inline(always)]
pub fn state_update_typed<K, F>(key: &K, f: F) -> Result<usize>
where
    K: TypedStateKey,
    F: FnOnce(Option<K::Value>) -> K::Value,
{
    state_update_loose(key, f)
}

/// Delete this hook's own state entry for `key`.
///
/// # Why deletion needs its own function
///
/// The Hook API has no "delete" call: an entry is deleted by **writing zero
/// bytes to it** (`state` with an empty value), which also refunds the
/// owner reserve that entry was holding. [`state_set_typed`] takes a
/// `&K::Value`, so reaching a zero-length write through it would mean
/// pairing the key with a value type that happens to encode to nothing
/// (`[u8; 0]` does) — spelling "delete" as an accident of the value type
/// rather than an intent at the call site. This function is the explicit
/// spelling instead, independent of any value type, and available to a key
/// with no [`TypedStateKey`] pairing at all.
///
/// Deleting an entry that does not exist has no distinct "not found"
/// failure — the host accepts a delete of an absent entry like any other
/// empty write (xahaud's `set_state_cache` returns `DOESNT_EXIST` only for
/// a missing *account*, never for a missing state entry). Hence `Result<()>`
/// rather than "was there anything to delete" — read first with
/// [`state_get`]/[`state_get_typed`] if that distinction matters.
#[inline(always)]
pub fn state_delete(key: &impl StateKeyEncode) -> Result<()> {
    state_delete_encoded(&key.encode())
}

/// Delete this hook's own state entry for an already-[`EncodedStateKey`]d
/// `key`. Internal funnel behind [`state_delete`] — see
/// [`state_get_encoded`]'s doc comment for why this exists.
#[inline(always)]
pub(crate) fn state_delete_encoded(key: &EncodedStateKey) -> Result<()> {
    crate::api::state::state_set(&[], key).map(|_| ())
}

/// Read a state entry belonging to another namespace/account, decoded as
/// `T`. `namespace`/`account` follow
/// [`crate::api::state::state_foreign`]'s `Option` convention. `Ok(None)`
/// means no entry exists — see the module doc comment.
#[inline(always)]
pub fn state_foreign_get<T: FromBytes>(
    key: &impl StateKeyEncode,
    namespace: Option<&[u8]>,
    account: Option<&[u8]>,
) -> Result<Option<T>> {
    state_foreign_get_encoded(&key.encode(), namespace, account)
}

/// Read a state entry belonging to another namespace/account for an
/// already-[`EncodedStateKey`]d `key`. Internal funnel behind
/// [`state_foreign_get`] — see [`state_get_encoded`]'s doc comment for why
/// this exists.
#[inline(always)]
pub(crate) fn state_foreign_get_encoded<T: FromBytes>(
    key: &EncodedStateKey,
    namespace: Option<&[u8]>,
    account: Option<&[u8]>,
) -> Result<Option<T>> {
    let mut storage = core::mem::MaybeUninit::<[u8; MAX_TYPED_STATE_LEN]>::uninit();
    // SAFETY: see `uninit_slice_mut`'s doc comment; `state_foreign_raw_code`
    // cannot report writing more bytes than the buffer it was handed, and
    // `decode_read` only ever reads the `..n` prefix that count reports.
    let buf = unsafe { uninit_slice_mut(&mut storage) };
    let code = crate::api::state::state_foreign_raw_code(buf, key, namespace, account);
    decode_read(code, buf)
}

/// Write a state entry belonging to another namespace/account, encoding
/// `value` as `T`. `namespace`/`account` follow
/// [`crate::api::state::state_foreign`]'s `Option` convention. Returns the
/// number of bytes written.
#[inline(always)]
pub fn state_foreign_set_loose<T: ToBytes>(
    key: &impl StateKeyEncode,
    value: &T,
    namespace: Option<&[u8]>,
    account: Option<&[u8]>,
) -> Result<usize> {
    state_foreign_set_encoded(&key.encode(), value, namespace, account)
}

/// Write a state entry belonging to another namespace/account for an
/// already-[`EncodedStateKey`]d `key`. Internal funnel behind
/// [`state_foreign_set_loose`] — see [`state_get_encoded`]'s doc comment for
/// why this exists.
#[inline(always)]
pub(crate) fn state_foreign_set_encoded<T: ToBytes>(
    key: &EncodedStateKey,
    value: &T,
    namespace: Option<&[u8]>,
    account: Option<&[u8]>,
) -> Result<usize> {
    let raw = encode_write(value);
    let src = raw.get(..T::MAX_LEN).ok_or(HookError::TooBig)?;
    crate::api::state::state_foreign_set(src, key, namespace, account)
}

/// Read-modify-write a state entry belonging to another namespace/account:
/// reads the current value (or `None` if absent), calls `f` to compute the
/// next value, writes it back, and returns the number of bytes written.
/// `namespace`/`account` follow
/// [`crate::api::state::state_foreign`]'s `Option` convention.
#[inline(always)]
pub fn state_foreign_update_loose<T, F>(
    key: &impl StateKeyEncode,
    namespace: Option<&[u8]>,
    account: Option<&[u8]>,
    f: F,
) -> Result<usize>
where
    T: FromBytes + ToBytes,
    F: FnOnce(Option<T>) -> T,
{
    let current = state_foreign_get::<T>(key, namespace, account)?;
    let next = f(current);
    state_foreign_set_loose(key, &next, namespace, account)
}

/// Read a state entry belonging to another namespace/account, decoded as
/// `key`'s own [`TypedStateKey::Value`] — the key/value-pairing-safe
/// counterpart to [`state_foreign_get`] (see [`TypedStateKey`]'s doc
/// comment for why). `namespace`/`account` follow
/// [`crate::api::state::state_foreign`]'s `Option` convention. `Ok(None)`
/// means no entry exists — see the module doc comment.
#[inline(always)]
pub fn state_foreign_get_typed<K: TypedStateKey>(
    key: &K,
    namespace: Option<&[u8]>,
    account: Option<&[u8]>,
) -> Result<Option<K::Value>> {
    state_foreign_get::<K::Value>(key, namespace, account)
}

/// Write a state entry belonging to another namespace/account, encoding
/// `value` as `key`'s own [`TypedStateKey::Value`] — the
/// key/value-pairing-safe counterpart to [`state_foreign_set_loose`] (see
/// [`TypedStateKey`]'s doc comment for why). `namespace`/`account` follow
/// [`crate::api::state::state_foreign`]'s `Option` convention. Returns the
/// number of bytes written.
#[inline(always)]
pub fn state_foreign_set_typed<K: TypedStateKey>(
    key: &K,
    value: &K::Value,
    namespace: Option<&[u8]>,
    account: Option<&[u8]>,
) -> Result<usize> {
    state_foreign_set_loose(key, value, namespace, account)
}

/// Read-modify-write a state entry belonging to another namespace/account,
/// using `key`'s own [`TypedStateKey::Value`] — the key/value-pairing-safe
/// counterpart to [`state_foreign_update_loose`] (see [`TypedStateKey`]'s
/// doc comment for why). `namespace`/`account` follow
/// [`crate::api::state::state_foreign`]'s `Option` convention.
#[inline(always)]
pub fn state_foreign_update_typed<K, F>(
    key: &K,
    namespace: Option<&[u8]>,
    account: Option<&[u8]>,
    f: F,
) -> Result<usize>
where
    K: TypedStateKey,
    F: FnOnce(Option<K::Value>) -> K::Value,
{
    state_foreign_update_loose(key, namespace, account, f)
}

/// Declares an enum whose variants encode to their own real byte length
/// (discriminant plus payload, `<= `[`crate::types::STATE_KEY_LEN`], never
/// locally padded up to 32), implementing [`StateKeyEncode`] for it. See
/// the module doc comment's "Key length and padding" section for the
/// encoding rules and an example.
///
/// Grammar: unit variants (`Name`) and single-payload tuple variants
/// (`Name(PayloadType)`, `PayloadType: `[`crate::convert::ToBytes`]) may be
/// freely mixed; every variant is assigned a sequential `u8` discriminant
/// by this macro (kept separate from the generated enum's own, ordinary
/// Rust discriminants, since a data-carrying variant cannot have one on
/// stable Rust) — declaration order is significant, and inserting or
/// reordering a variant changes every later variant's encoded key.
///
/// # Pairing with a value type
///
/// A `state_keys!` enum implements [`StateKeyEncode`] but not
/// [`TypedStateKey`] — pair it with a value type via a hand-written
/// [`TypedStateKey`] impl, exactly like a `#[derive(HookKey)]` struct:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::state_keys;
///
/// state_keys! {
///     /// This hook's persistent data.
///     enum DataKey {
///         /// A running counter.
///         Counter,
///         /// A per-owner balance, keyed by the owner's account.
///         Balance(AccountId),
///     }
/// }
///
/// impl TypedStateKey for DataKey {
///     type Value = u32;
/// }
///
/// // `NotImplemented` here is the host stub every Hook API call returns on
/// // a host build — this only proves the `TypedStateKey`/`state_get_typed`
/// // call chain compiles and runs.
/// assert_eq!(
///     state_get_typed(&DataKey::Counter),
///     Err(HookError::NotImplemented)
/// );
/// assert_eq!(
///     state_set_typed(&DataKey::Counter, &1u32),
///     Err(HookError::NotImplemented)
/// );
/// assert_eq!(
///     state_update_typed(&DataKey::Counter, |_| 1u32),
///     Err(HookError::NotImplemented)
/// );
/// assert_eq!(
///     state_foreign_get_typed(&DataKey::Counter, None, None),
///     Err(HookError::NotImplemented)
/// );
/// assert_eq!(
///     state_foreign_set_typed(&DataKey::Counter, &1u32, None, None),
///     Err(HookError::NotImplemented)
/// );
/// assert_eq!(
///     state_foreign_update_typed(&DataKey::Counter, None, None, |_| 1u32),
///     Err(HookError::NotImplemented)
/// );
/// ```
#[macro_export]
macro_rules! state_keys {
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $Name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident $(($payload:ty))?
            ),* $(,)?
        }
    ) => {
        $crate::__state_keys_step! {
            @step
            meta = [$(#[$enum_meta])*], vis = $vis, name = $Name,
            fields = [ $( $(#[$variant_meta])* $variant $(($payload))? ),* ],
            next = 0u8,
            enum_body = [],
            arms = [],
            discs = [],
            fits_checks = []
        }
    };
}

/// Internal recursive tt-muncher backing [`state_keys!`](crate::state_keys).
///
/// `#[doc(hidden)]` but necessarily `#[macro_export]`ed (a macro invoked as
/// `$crate::name!` from another macro's expansion must be exported) —
/// mirrors `txn.rs`'s `__txn_template_step!` split (public entry macro,
/// hidden recursive worker).
///
/// Peels one variant off `fields` per step, appending a complete, already
/// concrete `enum_body`/`arms`/`discs`/`fits_checks` entry for it — the
/// unit-variant and single-payload-tuple-variant cases each get their own
/// matcher arm below, so every generated `arms` entry is a complete,
/// self-contained `pattern => body` unit. Two Rust macro constraints force
/// this shape: a macro invocation cannot expand to a bare match arm, so
/// each `Name::Variant => { .. }` must be written out whole by one macro
/// step rather than spliced from separate pattern/body calls; and a
/// conditionally-shaped pattern (`Name::Variant` vs.
/// `Name::Variant(__payload)`) can't be produced from a single
/// `$(...)?`-gated group without that group referencing the metavariable
/// driving the optionality — dispatching unit vs. tuple to separate matcher
/// arms sidesteps both.
#[doc(hidden)]
#[macro_export]
macro_rules! __state_keys_step {
    // Terminal: every variant has been consumed — emit the enum, the
    // `StateKeyEncode` impl, and the compile-time checks.
    (
        @step
        meta = [$($enum_meta:tt)*], vis = $vis:vis, name = $Name:ident,
        fields = [],
        next = $next:expr,
        enum_body = [$($enum_body:tt)*],
        arms = [$($arms:tt)*],
        discs = [$($discs:tt)*],
        fits_checks = [$($fits_checks:tt)*]
    ) => {
        $($enum_meta)*
        $vis enum $Name {
            $($enum_body)*
        }

        impl $crate::state::StateKeyEncode for $Name {
            #[inline(always)]
            fn encode(&self) -> $crate::state::EncodedStateKey {
                match self {
                    $($arms)*
                }
            }
        }

        // Every payload must leave room for the 1-byte discriminant in the
        // fixed 32-byte key.
        $($fits_checks)*

        // Discriminants must be pairwise distinct.
        #[allow(clippy::indexing_slicing)] // const-evaluated only, bounded by the `while` guards
        const _: () = {
            const DISCS: &[u8] = &[$($discs)*];
            let mut i = 0;
            while i < DISCS.len() {
                let mut j = i.wrapping_add(1);
                while j < DISCS.len() {
                    assert!(DISCS[i] != DISCS[j], "state_keys!: duplicate discriminant");
                    j = j.wrapping_add(1);
                }
                i = i.wrapping_add(1);
            }
        };
    };

    // Unit variant.
    (
        @step
        meta = [$($enum_meta:tt)*], vis = $vis:vis, name = $Name:ident,
        fields = [
            $(#[$variant_meta:meta])* $variant:ident
            $(, $($rest:tt)*)?
        ],
        next = $next:expr,
        enum_body = [$($enum_body:tt)*],
        arms = [$($arms:tt)*],
        discs = [$($discs:tt)*],
        fits_checks = [$($fits_checks:tt)*]
    ) => {
        $crate::__state_keys_step! {
            @step
            meta = [$($enum_meta)*], vis = $vis, name = $Name,
            fields = [ $($($rest)*)? ],
            next = ($next + 1u8),
            enum_body = [
                $($enum_body)*
                $(#[$variant_meta])* $variant,
            ],
            arms = [
                $($arms)*
                $Name::$variant => {
                    let mut __out = [0u8; $crate::types::STATE_KEY_LEN];
                    if let Some(__byte) = __out.get_mut(0) {
                        *__byte = $next;
                    }
                    // Real length: just the 1-byte discriminant — a unit
                    // variant carries no payload, so there is nothing else
                    // to send (see the module doc comment's "Key length
                    // and padding" section).
                    $crate::state::EncodedStateKey::new(__out, 1usize)
                }
            ],
            discs = [ $($discs)* $next, ],
            fits_checks = [ $($fits_checks)* ]
        }
    };

    // Single-payload tuple variant.
    (
        @step
        meta = [$($enum_meta:tt)*], vis = $vis:vis, name = $Name:ident,
        fields = [
            $(#[$variant_meta:meta])* $variant:ident ($payload:ty)
            $(, $($rest:tt)*)?
        ],
        next = $next:expr,
        enum_body = [$($enum_body:tt)*],
        arms = [$($arms:tt)*],
        discs = [$($discs:tt)*],
        fits_checks = [$($fits_checks:tt)*]
    ) => {
        $crate::__state_keys_step! {
            @step
            meta = [$($enum_meta)*], vis = $vis, name = $Name,
            fields = [ $($($rest)*)? ],
            next = ($next + 1u8),
            enum_body = [
                $($enum_body)*
                $(#[$variant_meta])* $variant($payload),
            ],
            arms = [
                $($arms)*
                $Name::$variant(__payload) => {
                    let mut __out = [0u8; $crate::types::STATE_KEY_LEN];
                    if let Some(__byte) = __out.get_mut(0) {
                        *__byte = $next;
                    }
                    if let Some(__rest) = __out.get_mut(1..) {
                        let _ = <$payload as $crate::convert::ToBytes>::write(
                            __payload, __rest,
                        );
                    }
                    // Real length: discriminant byte + payload — no
                    // trailing padding (see the module doc comment's "Key
                    // length and padding" section).
                    $crate::state::EncodedStateKey::new(
                        __out,
                        1usize.wrapping_add(
                            <$payload as $crate::convert::ToBytes>::MAX_LEN,
                        ),
                    )
                }
            ],
            discs = [ $($discs)* $next, ],
            fits_checks = [
                $($fits_checks)*
                const _: () = assert!(
                    <$payload as $crate::convert::ToBytes>::MAX_LEN
                        < $crate::types::STATE_KEY_LEN,
                    "state_keys!: payload too large to leave room for the discriminant byte in a 32-byte key"
                );
            ]
        }
    };
}

#[cfg(test)]
mod tests {
    // Tests are exempt from the panic-freedom lints (see docs/DESIGN.md
    // §8); indexing on known-good, fixed-size local arrays is idiomatic
    // here (matches the convention in `txn.rs`'s test module).
    #![allow(clippy::indexing_slicing)]

    use super::*;
    use crate::error::HookError;
    use crate::types::STATE_KEY_LEN;

    #[test]
    fn state_get_maps_doesnt_exist_to_none() {
        let raw = [0u8; MAX_TYPED_STATE_LEN];
        assert_eq!(
            decode_read::<u32>(rshooks_core::DOESNT_EXIST, &raw),
            Ok(None)
        );
    }

    #[test]
    fn state_get_propagates_other_errors() {
        let raw = [0u8; MAX_TYPED_STATE_LEN];
        assert_eq!(
            decode_read::<u32>(rshooks_core::INTERNAL_ERROR, &raw),
            Err(HookError::InternalError)
        );
    }

    #[test]
    fn state_get_decodes_present_value() {
        let mut raw = [0u8; MAX_TYPED_STATE_LEN];
        raw[0] = 42;
        assert_eq!(decode_read::<u32>(4, &raw), Ok(Some(42u32)));
    }

    #[test]
    fn state_get_propagates_short_decode_as_error_not_none() {
        // 3 bytes written is not enough for a `u32` (needs 4): this must
        // surface as an `Err`, never be confused with "absent."
        let raw = [0u8; MAX_TYPED_STATE_LEN];
        assert_eq!(decode_read::<u32>(3, &raw), Err(HookError::TooSmall));
    }

    #[test]
    fn encode_write_round_trips_through_from_bytes() {
        let raw = encode_write(&0x1122_3344u32);
        assert_eq!(u32::read(&raw), Ok(0x1122_3344));
    }

    #[test]
    fn short_array_key_encodes_to_its_own_real_length_unpadded() {
        // The direct Rust counterpart to the C hook idiom
        // `state(&v, 8, "RR", 2)`: a short literal key, sent to the host
        // exactly as-is — no local zero-padding up to 32 bytes.
        let encoded = b"RR".encode();
        assert_eq!(encoded.as_ref(), b"RR");
        assert_eq!(encoded.as_ref().len(), 2);
    }

    #[test]
    fn single_byte_array_key_encodes_to_one_byte() {
        let encoded = [7u8].encode();
        assert_eq!(encoded.as_ref(), &[7u8]);
    }

    #[test]
    fn full_32_byte_array_key_encodes_unchanged() {
        let raw = [0xABu8; STATE_KEY_LEN];
        let encoded = raw.encode();
        assert_eq!(encoded.as_ref(), &raw[..]);
    }

    #[test]
    fn full_state_key_passes_through_all_32_bytes_unchanged() {
        let raw = [0xCDu8; STATE_KEY_LEN];
        let key = StateKey::from(raw);
        let encoded = key.encode();
        assert_eq!(encoded.as_ref(), &raw[..]);
        assert_eq!(encoded.as_ref().len(), STATE_KEY_LEN);
    }

    /// Asserts [`EncodedStateKey::from_short`] and `<[u8; N] as
    /// StateKeyEncode>::encode` produce byte-identical results for the same
    /// key — the `#[hooks]` macro's literal `#[state(key = b"...")]` codegen
    /// picks `from_short` over `encode` purely as a compile-time-vs-runtime
    /// optimization and must never observe a different result from doing so.
    fn assert_from_short_matches_array_encode<const N: usize>(key: &[u8; N]) {
        let from_short = EncodedStateKey::from_short(key);
        let encoded = key.encode();
        assert_eq!(from_short.as_ref(), encoded.as_ref());
        assert_eq!(from_short.buf, encoded.buf);
        assert_eq!(from_short.len, encoded.len);
    }

    #[test]
    fn from_short_matches_array_encode_byte_identical() {
        assert_from_short_matches_array_encode(&[0xAAu8; 1]);
        assert_from_short_matches_array_encode(&[0u8, 0xFF]);
        assert_from_short_matches_array_encode(&[0u8, 1, 2, 3, 4, 5, 0]);
        assert_from_short_matches_array_encode(&{
            let mut key = [0xCDu8; STATE_KEY_LEN];
            key[0] = 0;
            key[STATE_KEY_LEN - 1] = 0;
            key
        });
    }

    #[test]
    fn different_length_array_keys_encode_to_different_bytes() {
        // `b"RR"` (2 bytes) and a hypothetical zero-padded-to-32 version of
        // the same bytes must NOT compare equal here — this module never
        // performs that local padding (the host does it, on its own left
        // side) — see the module doc comment's "Key length and padding"
        // section.
        let short = b"RR".encode();
        assert_ne!(short.as_ref().len(), STATE_KEY_LEN);
        assert_eq!(short.as_ref(), b"RR");
    }

    #[test]
    fn smoke_not_implemented_on_host() {
        assert_eq!(
            state_get::<u32>(&StateKey::from([0u8; STATE_KEY_LEN])),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_set_loose(&StateKey::from([0u8; STATE_KEY_LEN]), &1u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_update_loose(&StateKey::from([0u8; STATE_KEY_LEN]), |_: Option<u32>| 1u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign_get::<u32>(&StateKey::from([0u8; STATE_KEY_LEN]), None, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign_set_loose(&StateKey::from([0u8; STATE_KEY_LEN]), &1u32, None, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign_update_loose(
                &StateKey::from([0u8; STATE_KEY_LEN]),
                None,
                None,
                |_: Option<u32>| 1u32
            ),
            Err(HookError::NotImplemented)
        );
    }

    state_keys! {
        /// Test-only key space exercising every `state_keys!` variant shape.
        enum TestKey {
            /// Unit variant.
            Counter,
            /// Tuple variant with a fixed-size payload.
            Balance(u32),
        }
    }

    #[test]
    fn unit_variant_encodes_just_the_discriminant_no_padding() {
        // Real length is 1 byte — no local zero-padding up to 32 (the host
        // left-pads; see the module doc comment's "Key length and padding"
        // section).
        let encoded = TestKey::Counter.encode();
        assert_eq!(encoded.as_ref(), &[0u8]);
    }

    #[test]
    fn tuple_variant_encodes_discriminant_plus_payload_no_padding() {
        // Real length is 1 (discriminant) + 4 (u32 payload) = 5 bytes.
        let encoded = TestKey::Balance(0x0102_0304).encode();
        let mut expected = [0u8; 5];
        expected[0] = 1;
        expected[1..5].copy_from_slice(&0x0102_0304u32.to_le_bytes());
        assert_eq!(encoded.as_ref(), &expected[..]);
    }

    #[test]
    fn distinct_variants_encode_to_distinct_keys() {
        assert_ne!(
            TestKey::Counter.encode().as_ref(),
            TestKey::Balance(0).encode().as_ref()
        );
    }

    // `TypedStateKey`: a key type paired with exactly one value type, via
    // the `_typed`-suffixed functions (see their doc comments) — a plain
    // trait impl, so it can be exercised directly here rather than only
    // via a doctest.
    impl TypedStateKey for TestKey {
        type Value = u32;
    }

    #[test]
    fn typed_state_key_pairing_not_implemented_on_host() {
        assert_eq!(
            state_get_typed(&TestKey::Counter),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_set_typed(&TestKey::Counter, &1u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_update_typed(&TestKey::Counter, |_| 1u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign_get_typed(&TestKey::Counter, None, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign_set_typed(&TestKey::Counter, &1u32, None, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign_update_typed(&TestKey::Counter, None, None, |_| 1u32),
            Err(HookError::NotImplemented)
        );
    }
}
