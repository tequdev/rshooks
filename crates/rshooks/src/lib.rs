//! Ergonomic, `no_std` wrappers and macros for writing Xahau Hooks.
//!
//! Use [`api`] for typed Hook API calls, [`state`] for typed state access,
//! [`types`] for protocol values, and [`raw`] for the underlying FFI.

#![no_std]

pub mod api;
pub mod buf_eq;
pub mod convert;
pub mod decl;
pub mod error;
mod errors;
pub mod exit;
#[cfg(any(
    feature = "unstable-param-sig-interface",
    feature = "unstable-state-interface"
))]
mod interface_name;
pub mod ledger_entry_type;
mod macros;
pub mod sfield;
#[cfg(feature = "unstable-state-interface")]
pub mod si;
#[cfg(feature = "unstable-param-sig-interface")]
pub mod sig;
pub mod slot_obj;
pub mod state;
pub mod static_cell;
pub mod sto_writer;
#[cfg(all(not(target_arch = "wasm32"), feature = "testenv"))]
pub(crate) mod testenv_bridge;
pub mod tx_type;
pub mod txn;
pub mod types;
pub mod views;
pub mod xfl;
pub mod xfl_unchecked;

// `pad!`/`pad_left!` expand to `$crate::padded_bytes(...)`/
// `$crate::padded_bytes_left(...)`; re-exported (hidden) here since the
// helpers live in the private `macros` module.
#[doc(hidden)]
pub use macros::padded_bytes;
#[doc(hidden)]
pub use macros::padded_bytes_left;

pub use macros::no_unroll;

/// Re-export of `rshooks-core`: the raw Hook API declarations and every
/// C-verbatim constant, unwrapped.
pub use rshooks_core as raw;

/// Re-export of [`decl`]'s ZST chain-declaration handle types — the field
/// types a `#[hooks]`-annotated struct declares against. The rest of
/// [`decl`] (`StateEntry`, `HookParamAt`, `OtxnParamAt`, the `*Spec` traits)
/// is the macro-generated side of the handshake, not something a hook
/// author names directly, so it stays reachable only at `decl::`.
pub use decl::{HookParam, OtxnParam, State};

/// Implementation-detail handshake between the (forthcoming) `#[hooks]`
/// struct macro and `#[hooks]` impl macro.
#[doc(hidden)]
pub mod __internal {
    /// Implemented (by generated code) on a chain-struct type by the
    /// `#[hooks]` attribute on its inherent `impl` block; asserted (by
    /// generated code) by the `#[hooks]` attribute on the struct itself, so
    /// a struct annotated with `#[hooks]` that never gets a matching
    /// `#[hooks]` impl block fails to compile instead of silently
    /// generating a hook chain with no entrypoints.
    pub trait HookChainImpl {}
}

/// Declares a multi-hook chain: a `#[hooks]`-annotated struct declares the
/// chain's shared `State`/`HookParam`/`OtxnParam` schema (see [`decl`]),
/// and a `#[hooks]`-annotated inherent `impl` block on that same struct
/// declares its `#[hook(<index>, ..)]`/`#[cbak(<index>)]` entries, along
/// with each entry's trigger set and descriptive metadata (see
/// `docs/MULTI_HOOK_STRUCT_DESIGN.md` for the full grammar and semantics).
///
/// This is a declaration macro, not a value or type most hook code names
/// directly, so it is deliberately left out of [`prelude`]; import it
/// explicitly (`use rshooks::hooks;`).
pub use rshooks_macros::hooks;

/// Decodes a classic XRPL/Xahau r-address into an [`types::AccountId`] at
/// compile time.
///
/// Invalid characters, version bytes, lengths, and checksums produce a
/// `compile_error!`. The expansion is an `AccountId` literal and can be used
/// in `const` and `static` items.
///
/// # Examples
///
/// ```
/// use rshooks::account_id;
/// use rshooks::types::AccountId;
///
/// const OWNER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");
/// assert_eq!(
///     OWNER.0,
///     [
///         0xB5, 0xF7, 0x62, 0x79, 0x8A, 0x53, 0xD5, 0x43, 0xA0, 0x14, 0xCA, 0xF8, 0xB2, 0x97,
///         0xCF, 0xF8, 0xF2, 0xF9, 0x37, 0xE8
///     ]
/// );
///
/// static ACCOUNT_ZERO: AccountId = account_id!("rrrrrrrrrrrrrrrrrrrrrhoLvTp");
/// assert_eq!(ACCOUNT_ZERO.0, [0u8; 20]);
/// ```
///
/// A checksum mismatch fails to compile:
/// ```compile_fail
/// rshooks::account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTH");
/// ```
///
/// A wrong version byte fails to compile:
/// ```compile_fail
/// rshooks::account_id!("sJHw2iRxXngPFKZvYbjkfifqt8CJghksMM");
/// ```
///
/// A truncated address fails to compile:
/// ```compile_fail
/// rshooks::account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdty");
/// ```
///
/// An invalid character fails to compile:
/// ```compile_fail
/// rshooks::account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyT0");
/// ```
pub use rshooks_macros::account_id;

/// Encodes a numeric literal into a Xahau [`xfl::XFL`] value at compile
/// time, bit-exact — entirely via integer/string arithmetic, never `f64`.
///
/// # Why this exists
///
/// XFL is a decimal floating-point format (see [`xfl`]'s module doc for the
/// bit layout); every decimal value has exactly one correct XFL encoding,
/// and hand-computing its raw bit pattern is opaque and error-prone. `XFL!`
/// takes the decimal value directly, parsing its literal text by hand
/// instead of routing it through `f64` — `f64` cannot represent `0.1` (or
/// most decimal fractions) exactly and would silently reintroduce the
/// rounding error XFL exists to avoid. Every digit up to XFL's
/// 16-significant-digit limit is preserved exactly:
///
/// ```
/// use rshooks::XFL;
/// use rshooks::xfl::XFL as XflType;
///
/// const DEFAULT_REWARD_RATE: XflType = XFL!(0.003333333333333333);
/// assert_eq!(DEFAULT_REWARD_RATE.raw_bits(), 6_038_156_834_009_797_973);
/// ```
///
/// # Grammar
///
/// An optional leading `-`, then exactly one numeric literal token:
///
/// - a plain integer (`0`, `123456789`), optionally with `_` digit
///   separators (`1_000_000`)
/// - a decimal (`0.1`, `1.`, `1.50`)
/// - either of the above with a decimal exponent (`1e-5`, `2.6E6`,
///   `1e+3`), which may itself contain `_` separators
///
/// Trailing zeros are normalized away rather than counted against the
/// digit limit below — `1.50`, `1_000`, and `2600000` all encode exactly
/// as if written `1.5`, `1e3`, and `2.6e6`.
///
/// # What gets rejected
///
/// Every rejection is a `compile_error!` at the macro invocation — never a
/// panic:
///
/// - anything that is not a single numeric literal token: missing input,
///   extra tokens, a string/char/byte literal, or a hexadecimal/octal/
///   binary integer (`0x..`/`0o..`/`0b..`)
/// - a numeric type suffix (`1i64`, `1.0f64`) — `XFL!` always produces its
///   own `i64` expansion, so a suffix can only ever be a mistake
/// - more than 16 significant decimal digits (after trailing-zero
///   normalization) — XFL's mantissa cannot hold them, and this macro
///   never silently rounds; round the literal explicitly instead
/// - a magnitude outside XFL's representable range: roughly `1e-81` to
///   `1e96` (see [`xfl`]'s module doc comment for the exact unbiased
///   exponent bounds, `-96..=80`) — reported as a distinct "too small" or
///   "too large" message
///
/// # Examples
///
/// The reference vectors below, verified against xahaud's own `float.c`-
/// equivalent encoder (`hook_float::make_float` in
/// `src/xrpld/app/hook/HookAPI.h`):
///
/// ```
/// use rshooks::XFL;
///
/// assert_eq!(XFL!(0).raw_bits(), 0);
/// assert_eq!(XFL!(0.0).raw_bits(), 0);
/// assert_eq!(XFL!(-0).raw_bits(), 0);
/// assert_eq!(XFL!(-0.0).raw_bits(), 0);
/// assert_eq!(XFL!(1).raw_bits(), 6_089_866_696_204_910_592);
/// assert_eq!(XFL!(-1).raw_bits(), 1_478_180_677_777_522_688);
/// assert_eq!(XFL!(0.1).raw_bits(), 6_071_852_297_695_428_608);
/// assert_eq!(XFL!(123456789).raw_bits(), 6_234_216_452_170_766_464);
/// assert_eq!(
///     XFL!(0.003333333333333333).raw_bits(),
///     6_038_156_834_009_797_973
/// );
/// assert_eq!(XFL!(2600000).raw_bits(), 6_199_553_087_261_802_496);
/// ```
///
/// Used directly in a `const`, since [`xfl::XFL::from_raw_bits`] (what
/// this macro expands to) is a `const fn`:
///
/// ```
/// use rshooks::XFL;
/// use rshooks::xfl::XFL as XflType;
///
/// const ONE: XflType = XFL!(1);
/// static REWARD_DELAY: XflType = XFL!(2600000);
/// assert_eq!(ONE.raw_bits(), 6_089_866_696_204_910_592);
/// assert_eq!(REWARD_DELAY.raw_bits(), 6_199_553_087_261_802_496);
/// ```
///
/// More than 16 significant digits fails to compile:
/// ```compile_fail
/// rshooks::XFL!(1.2345678901234567);
/// ```
///
/// A magnitude too large to represent fails to compile:
/// ```compile_fail
/// rshooks::XFL!(1e96);
/// ```
pub use rshooks_macros::XFL;

/// Derives [`convert::ToBytes`] and an explicit [`state::StateKeyEncode`]
/// impl for a fixed-size, named-field struct used as a **composite
/// hook-state key** — a tag byte plus an `AccountId`, say — with no
/// hand-packed byte buffer anywhere. See [`HookData`] for the state-*value*
/// counterpart, [`ParamName`] for the analogous Hook API parameter-*name*
/// role, and [`ParamValue`] for the analogous parameter-*value* role.
///
/// # Why a separate derive from [`HookData`]
///
/// A key and a value share the same "fixed-offset, named-field struct"
/// shape but play different roles:
///
/// - A key is only ever **encoded outward** — handed to `state`/
///   `state_foreign` to *locate* an entry — never read back and decoded as
///   itself. `HookKey` reflects that by generating only
///   [`convert::ToBytes`] plus [`state::StateKeyEncode`]: no `FromBytes`,
///   no `FixedRead`, no inherent `LEN` const.
/// - A key's real encoded length must fit within the Hook API's 32-byte key
///   space (a value has no size cap, beyond this crate's own
///   `MAX_TYPED_STATE_LEN` convenience limit — see [`state`]'s module doc).
///   `HookKey` checks that bound **at derive time**: a struct that encodes
///   to 33+ bytes fails to compile at its own definition. The encoded key
///   sent to the host is **not** locally zero-padded — it is exactly the
///   struct's own length, e.g. 2 bytes for `{ tag: u8, small: u8 }`; the
///   host itself left-pads a key shorter than 32 bytes (see [`state`]'s
///   module doc, "Key length and padding").
/// - Only a `#[derive(HookKey)]` struct, a [`state_keys!`](crate::state_keys)
///   enum, or [`types::StateKey`] itself implements
///   [`state::StateKeyEncode`] — an ordinary `#[derive(HookData)]` value
///   struct does not automatically qualify as a key.
///
/// # Grammar
///
/// Identical field grammar to [`HookData`] (see its doc comment): a plain,
/// non-generic, named-field struct with at least one field, every field a
/// fixed-size type implementing [`convert::ToBytes`] (nesting another
/// `#[derive(HookKey)]` or `#[derive(HookData)]` struct as a field works the
/// same way).
///
/// # What gets generated
///
/// - `impl ToBytes for Name`: fields encoded back-to-back, in declaration
///   order — identical codegen to [`HookData`]'s `ToBytes` impl; this derive
///   only adds the `StateKeyEncode` impl on top.
/// - `impl state::StateKeyEncode for Name`: encodes `self` via the `ToBytes`
///   impl above into an [`state::EncodedStateKey`] carrying exactly `Name`'s
///   own real encoded length (never padded to 32), with a compile-time
///   assert that the length fits within 32 bytes.
///
/// # Byte image sent to the host
///
/// [`state::StateKeyEncode::encode`] produces `Name`'s fields, little-endian,
/// back-to-back — nothing more. A `{ tag: u8, small: u16 }` key encodes to
/// exactly 3 bytes, not 32:
///
/// ```
/// use rshooks::HookKey;
/// use rshooks::state::StateKeyEncode;
///
/// #[derive(HookKey, Clone, Copy)]
/// struct ShortKey {
///     tag: u8,
///     small: u16,
/// }
///
/// let encoded = ShortKey { tag: 0x11, small: 0x2233 }.encode();
/// assert_eq!(encoded.as_ref(), &[0x11, 0x33, 0x22]);
/// ```
///
/// # Examples
///
/// A composite state key (a tag byte plus an `AccountId`) paired with a
/// composite state value via a hand-written [`state::TypedStateKey`] impl,
/// used with [`state::state_get_typed`]/[`state::state_set_typed`] — no
/// `state_keys!` declaration, no hand-packed byte buffer, and (unlike the
/// loose [`state::state_get`]/[`state::state_set_loose`], which take the
/// value type as an independent generic parameter) no way to accidentally
/// read/write `DepositKey`'s entry as some other struct's value type:
///
/// ```
/// use rshooks::{HookData, HookKey};
/// use rshooks::prelude::*;
///
/// #[derive(HookKey, Clone, Copy)]
/// struct DepositKey {
///     tag: u8,
///     owner: AccountId,
/// }
///
/// #[derive(HookData, Clone, Copy, Debug, PartialEq)]
/// struct DepositValue {
///     amount: u64,
///     deadline: u32,
///     flags: u8,
/// }
///
/// impl TypedStateKey for DepositKey {
///     type Value = DepositValue;
/// }
///
/// assert_eq!(DepositValue::LEN, 8 + 4 + 1);
///
/// let key = DepositKey {
///     tag: 1,
///     owner: AccountId::default(),
/// };
///
/// // `NotImplemented` here is the host stub every Hook API call returns on
/// // a host build (see `rshooks-core`) — this only proves the
/// // `TypedStateKey`/`state_get_typed` call chain compiles and runs,
/// // exactly like `state_keys!`'s own doctest.
/// assert_eq!(state_get_typed(&key), Err(HookError::NotImplemented));
/// ```
///
/// An enum is rejected at compile time:
///
/// ```compile_fail
/// use rshooks::HookKey;
///
/// #[derive(HookKey)]
/// enum NotAStruct {
///     A,
///     B,
/// }
/// ```
///
/// A struct whose total encoded length exceeds the 32-byte state-key space
/// is rejected **at its own definition** — unlike [`HookData`], which has no
/// such bound at all (a state *value* has no fixed size cap):
///
/// ```compile_fail
/// use rshooks::HookKey;
///
/// #[derive(HookKey)]
/// struct TooBigForAKey {
///     a: [u8; 20],
///     b: [u8; 20],
/// }
/// ```
///
/// The loose [`state::state_get`]/[`state::state_set_loose`] take a key and
/// value type as independent generic parameters, so nothing stops pairing a
/// key with the *wrong* value type. [`state::state_set_typed`] closes that:
/// `value`'s type is checked against the key's declared
/// [`state::TypedStateKey::Value`], so a mismatched value is a compile error:
///
/// ```compile_fail
/// use rshooks::{HookData, HookKey};
/// use rshooks::prelude::*;
///
/// #[derive(HookKey, Clone, Copy)]
/// struct KeyA {
///     tag: u8,
/// }
///
/// #[derive(HookData, Clone, Copy)]
/// struct ValueA {
///     count: u32,
/// }
///
/// #[derive(HookData, Clone, Copy)]
/// struct ValueB {
///     amount: u64,
/// }
///
/// impl TypedStateKey for KeyA {
///     type Value = ValueA;
/// }
///
/// // ERROR: `ValueB` is not `KeyA`'s declared `Value` (`ValueA`).
/// let _ = state_set_typed(&KeyA { tag: 0 }, &ValueB { amount: 0 });
/// ```
pub use rshooks_macros::HookKey;

/// Derives [`convert::ToBytes`]/[`convert::FromBytes`]/[`convert::FixedRead`]
/// for a fixed-size, named-field struct used as a **composite hook-state
/// value** — read back and decoded by `state_get`/`state_get_typed`, written
/// by `state_set_loose`/`state_set_typed`. See [`HookKey`] for the
/// state-*key* counterpart, [`ParamName`] for the parameter-*name* role, and
/// [`ParamValue`] for the parameter-*value* role (a `#[derive(HookData)]`
/// struct also satisfies `ParamValue`'s `FromBytes`/`FixedRead`
/// requirement and so *can* be used as a parameter value directly —
/// [`ParamValue`] is the narrower, intent-revealing choice for a struct
/// that is only ever a parameter payload).
///
/// # Grammar
///
/// ```text
/// #[derive(HookData)]
/// $vis struct Name {
///     $vis field: FieldType,
///     ...
/// }
/// ```
///
/// A plain, non-generic, named-field struct (no tuple structs, unit
/// structs, enums, or unions) with at least one field. Every field's type
/// must implement [`convert::ToBytes`] + [`convert::FromBytes`]: this
/// crate's fixed-size primitives (`u8`/`u16`/`u32`/`u64`/`i64`),
/// [`xfl::XFL`], any `rshooks::types` newtype (`AccountId`, `Hash`, ...), a
/// raw `[u8; N]`, or another `#[derive(HookData)]` struct (nesting composes
/// for free — see below). A field of any other (variable-length) type fails
/// to compile with an ordinary rustc trait-bound error naming the missing
/// trait.
///
/// # What gets generated
///
/// - `impl ToBytes for Name` / `impl FromBytes for Name` / `impl FixedRead
///   for Name`: fields are encoded **back-to-back, in declaration order**,
///   each contributing exactly its own `ToBytes::MAX_LEN` bytes — no
///   padding, no per-field length prefix, no reordering.
/// - `Name::LEN: usize` — the total encoded length (`Name::MAX_LEN` under
///   another name, as an inherent const so call sites don't need `use
///   rshooks::convert::ToBytes;` just to name it), with a generated
///   rustdoc table listing the field layout.
///
/// # Zero-cost by construction
///
/// Every field offset is a compile-time constant, and every field
/// read/write delegates straight to that field's own
/// `ToBytes::write`/`FromBytes::read` — the same "fixed, unrolled offsets,
/// no runtime-computed length" shape this crate hand-writes for
/// [`txn_template!`]'s generated setters. There is no per-field loop, and
/// (up to 32 bytes at this crate's release profile — see [`state`]'s
/// `MAX_TYPED_STATE_LEN` doc) the toolchain lowers it to inlined stores
/// rather than a `memset`/`memcpy` call, so no unguarded loop at all.
/// `examples/12_typed-data`'s README measures this: a `#[derive(HookData)]`
/// struct and a hand-packed equivalent compile to the same worst-case
/// instruction count.
///
/// # Nesting
///
/// A `#[derive(HookData)]` struct can itself be a field of another, since
/// every derived struct already implements `ToBytes`/`FromBytes`/
/// `FixedRead` — nesting needs no special support:
///
/// ```
/// use rshooks::HookData;
/// use rshooks::prelude::*;
///
/// #[derive(HookData)]
/// struct Inner {
///     count: u32,
/// }
///
/// #[derive(HookData)]
/// struct Outer {
///     tag: u8,
///     inner: Inner,
/// }
///
/// assert_eq!(Outer::LEN, 1 + 4);
/// ```
///
/// # Examples
///
/// See [`HookKey`]'s doc comment for a full key+value worked example
/// (`DepositKey`/`DepositValue`, paired via a hand-written
/// [`state::TypedStateKey`] impl). A `HookData` struct also works directly
/// as a state value with the loose
/// [`state::state_get`]/[`state::state_set_loose`] (no key pairing, `T`
/// named independently at the call site):
///
/// ```
/// use rshooks::HookData;
/// use rshooks::prelude::*;
///
/// #[derive(HookData, Clone, Copy, Debug, PartialEq)]
/// struct DepositValue {
///     amount: u64,
///     deadline: u32,
///     flags: u8,
/// }
///
/// assert_eq!(DepositValue::LEN, 8 + 4 + 1);
///
/// let key = StateKey::from([0u8; 32]);
/// let value: Result<Option<DepositValue>> = state_get(&key);
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
///
/// # Full byte image
///
/// The examples above only check `Name::LEN`/round-trip-through-itself —
/// this one pins down `write()`'s exact output byte-for-byte against a
/// hand-built `expected` array, proving each field's little-endian
/// encoding and offset directly (see DESIGN.md §5.6, "Endianness
/// conventions"):
///
/// ```
/// use rshooks::HookData;
/// use rshooks::convert::ToBytes;
///
/// #[derive(HookData, Clone, Copy)]
/// struct FullImage {
///     a: u8,
///     b: u16,
///     c: u32,
///     d: u64,
/// }
///
/// let value = FullImage {
///     a: 0x11,
///     b: 0x2233,
///     c: 0x4455_6677,
///     d: 0x8899_AABB_CCDD_EEFF,
/// };
///
/// let mut buf = [0u8; 15];
/// assert_eq!(value.write(&mut buf), 15);
/// assert_eq!(FullImage::LEN, 15);
///
/// let mut expected = [0u8; 15];
/// expected[0..1].copy_from_slice(&0x11u8.to_le_bytes());
/// expected[1..3].copy_from_slice(&0x2233u16.to_le_bytes());
/// expected[3..7].copy_from_slice(&0x4455_6677u32.to_le_bytes());
/// expected[7..15].copy_from_slice(&0x8899_AABB_CCDD_EEFFu64.to_le_bytes());
/// assert_eq!(buf, expected);
/// ```
///
/// An enum is rejected at compile time (`HookData` only derives for a named-
/// field struct):
///
/// ```compile_fail
/// use rshooks::HookData;
///
/// #[derive(HookData)]
/// enum NotAStruct {
///     A,
///     B,
/// }
/// ```
///
/// A tuple struct is rejected the same way:
///
/// ```compile_fail
/// use rshooks::HookData;
///
/// #[derive(HookData)]
/// struct NotNamedFields(u32, u64);
/// ```
///
/// A field of a variable-length type (here, a bare slice reference) fails
/// to compile — not with a diagnostic this derive produces itself, but with
/// rustc's own trait-bound error against the generated `ToBytes`/`FromBytes`
/// impls, naming the missing trait:
///
/// ```compile_fail
/// use rshooks::HookData;
///
/// #[derive(HookData)]
/// struct VariableLength<'a> {
///     data: &'a [u8],
/// }
/// ```
///
/// A `HookData` struct does **not** automatically work as a state *key* —
/// there is no blanket [`state::StateKeyEncode`] impl over every `ToBytes`
/// type, so this fails to compile (use [`HookKey`] instead):
///
/// ```compile_fail
/// use rshooks::HookData;
/// use rshooks::prelude::*;
///
/// #[derive(HookData)]
/// struct NotAKey {
///     a: [u8; 20],
/// }
///
/// // ERROR: `NotAKey` has no `StateKeyEncode` impl (`HookData` never
/// // generates one — use `HookKey` for a state key).
/// let _ = state_get::<u64>(&NotAKey { a: [0; 20] });
/// ```
pub use rshooks_macros::HookData;

/// Derives [`convert::ToBytes`] (only — no [`convert::FromBytes`]/
/// [`convert::FixedRead`]) for a fixed-size, named-field struct used as a
/// **composite Hook API parameter name** — a name type implementing
/// [`convert::TypedParamName`] via a hand-written impl pairing it with a
/// value type, then read with
/// [`crate::api::hook_ctx::hook_param_typed`]/
/// [`crate::api::otxn::otxn_param_typed`], which take a reference to a name
/// value. See [`HookKey`] for the analogous hook-state *key* role, and
/// [`ParamValue`] for the parameter *value* counterpart.
///
/// # Why this derive doesn't implement [`convert::TypedParamName`] itself
///
/// This derive only generates [`convert::ToBytes`] for the annotated
/// struct; it does not implement [`convert::TypedParamName`] itself, since
/// that trait additionally needs `type Value`, supplied by a hand-written
/// impl pairing the name type with the value type it's read as — exactly
/// like [`state::TypedStateKey`] pairs a [`HookKey`] type with its value
/// type.
///
/// # Relationship to [`HookData`]
///
/// A hook-state value and a Hook API parameter *name* share the same
/// "fixed-offset struct" shape but are different concepts — `ParamName` is
/// deliberately narrower than `HookData`, not an alias for it:
///
/// - A parameter name is only ever **written** (handed to
///   `hook_param`/`otxn_param` to locate a value) — never read back and
///   decoded as itself. `ParamName` reflects that by generating only
///   [`convert::ToBytes`]: no [`convert::FromBytes`], no
///   [`convert::FixedRead`], no inherent `LEN` const.
/// - A parameter name has its own length bound the Hook API enforces —
///   [`convert::PARAM_NAME_MAX_LEN`], **1 to 32 bytes** (`hook_api.h`:
///   `TOO_SMALL` below 1, `TOO_BIG` above 32) — the same upper bound a hook
///   state key's real encoded length is checked against (see [`HookKey`]),
///   while a state *value* has no size cap at all. `ParamName` checks this
///   **at derive time**: a struct that encodes to 0 or to 33+ bytes fails
///   to compile at its own definition (contrast [`HookKey`]'s check, which
///   only has an upper bound — a key may be shorter than 32 bytes, but a
///   parameter name may not be shorter than 1 byte).
///
/// # Grammar
///
/// Identical field grammar to [`HookData`] (see its doc comment): a plain,
/// non-generic, named-field struct, every field a fixed-size type
/// implementing [`convert::ToBytes`] (nesting another `#[derive(ParamName)]`
/// or `#[derive(HookData)]` struct as a field works the same way).
///
/// # Examples
///
/// A composite parameter name — a topic byte plus a sub-index, the same
/// idea `xahaud`'s own genesis governance hook uses for its `IS0`..`IS19`
/// seat parameters, expressed as a struct instead of a hand-built name:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamName, ParamValue};
///
/// #[derive(ParamName, Clone, Copy)]
/// struct SeatParamName {
///     topic: u8,
///     seat: u8,
/// }
///
/// #[derive(ParamValue)]
/// struct Vote {
///     value: u8,
/// }
///
/// impl TypedParamName for SeatParamName {
///     type Value = Vote;
/// }
///
/// let seat = SeatParamName { topic: b'S', seat: 0 };
/// assert!(otxn_param_typed(&seat).is_err());
/// ```
///
/// An enum, a tuple struct, and a generic struct are all rejected at
/// compile time, exactly like [`HookData`]:
///
/// ```compile_fail
/// use rshooks::ParamName;
///
/// #[derive(ParamName)]
/// enum NotAStruct {
///     A,
///     B,
/// }
/// ```
///
/// A struct that encodes to more than 32 bytes — the Hook API's own
/// parameter-name upper bound — is rejected **at its own definition**,
/// unlike an oversized `HookData` struct (which has no such bound at all —
/// only an oversized `HookKey` struct gets an analogous derive-time check):
///
/// ```compile_fail
/// use rshooks::ParamName;
///
/// #[derive(ParamName)]
/// struct TooBigForAParamName {
///     a: [u8; 20],
///     b: [u8; 20],
/// }
/// ```
///
/// A `#[derive(ParamName)]` type cannot be read back as a value — it has no
/// `FromBytes`/`FixedRead` impl, unlike [`HookData`]/[`ParamValue`]:
///
/// ```compile_fail
/// use rshooks::prelude::*;
/// use rshooks::ParamName;
///
/// #[derive(ParamName)]
/// struct SeatParamName {
///     topic: u8,
///     seat: u8,
/// }
///
/// // ERROR: `SeatParamName` has no `FixedRead` impl (`ParamName` never
/// // generates one — a parameter name is write-only).
/// let _: Result<SeatParamName> = otxn_param_exact(b"S");
/// ```
pub use rshooks_macros::ParamName;

/// Derives [`convert::FromBytes`]/[`convert::FixedRead`] (only — no
/// [`convert::ToBytes`]) for a fixed-size, named-field struct used as a
/// **Hook API parameter value** — the [`convert::TypedParamName::Value`] a
/// [`ParamName`] type is paired with via a hand-written
/// [`convert::TypedParamName`] impl, read back and decoded by
/// [`api::hook_ctx::hook_param_typed`]/[`api::otxn::otxn_param_typed`] (and
/// the loose [`api::hook_ctx::hook_param_exact`]/
/// [`api::otxn::otxn_param_exact`]). See [`HookData`] for the hook-state
/// *value* role, and [`ParamName`] for the parameter *name* counterpart.
///
/// # Why this derive generates no [`convert::ToBytes`]
///
/// A parameter value is only ever **read back and decoded** — this hook
/// never writes its *own* parameters (`hook_param_set` writes a *different*
/// hook's parameter, taking a raw `&[u8]`, not a typed value). `ParamValue`
/// reflects that by generating only [`convert::FromBytes`]/
/// [`convert::FixedRead`]: no [`convert::ToBytes`], no inherent `LEN` const.
/// A consequence: a `#[derive(ParamValue)]` struct cannot be used as a
/// [`HookKey`]/[`ParamName`] field, nor as a hook-state value with
/// [`state::state_set_loose`] — both need `ToBytes`, which this derive
/// deliberately does not provide (use [`HookData`] for a struct that needs
/// to go both directions).
///
/// # Grammar
///
/// Identical field grammar to [`HookData`] (see its doc comment), except
/// every field's type need only implement [`convert::FromBytes`] (not also
/// [`convert::ToBytes`] — though every fixed-size type this crate provides
/// implements both, so this distinction rarely matters in practice).
///
/// # What gets generated
///
/// - `impl FromBytes for Name` / `impl FixedRead for Name`: fields are
///   decoded **back-to-back, in declaration order**, each consuming exactly
///   its own `<FieldType as ToBytes>::MAX_LEN` bytes — the same layout
///   [`HookData`] uses, just without the write-side impls or the `LEN`
///   const (there is no `ToBytes::MAX_LEN` on `Self` to name it by; the
///   per-field widths are summed directly in the generated code instead).
///
/// # Examples
///
/// A composite parameter value, paired with a plain byte-string name the
/// caller declared themselves, via a hand-written
/// [`convert::TypedParamName`] impl:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::ParamValue;
///
/// #[derive(ParamValue)]
/// struct Config {
///     min_amount: u64,
///     max_amount: u64,
/// }
///
/// struct CfgName;
///
/// impl ToBytes for CfgName {
///     const MAX_LEN: usize = 3;
///
///     fn write(&self, buf: &mut [u8]) -> usize {
///         buf[..3].copy_from_slice(b"CFG");
///         3
///     }
/// }
///
/// impl TypedParamName for CfgName {
///     type Value = Config;
///
///     // A plain byte-string name has nothing to compute — hand the
///     // literal straight to the closure instead of running it through
///     // `ToBytes::write`.
///     fn with_name_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
///         f(b"CFG")
///     }
/// }
///
/// let cfg = otxn_param_typed(&CfgName);
/// assert_eq!(cfg.err(), Some(HookError::NotImplemented));
/// ```
///
/// An enum, a tuple struct, and a generic struct are all rejected at
/// compile time, exactly like [`HookData`]:
///
/// ```compile_fail
/// use rshooks::ParamValue;
///
/// #[derive(ParamValue)]
/// enum NotAStruct {
///     A,
///     B,
/// }
/// ```
///
/// A `#[derive(ParamValue)]` type cannot be used as a hook-state key — it
/// has no [`state::StateKeyEncode`] impl (nor even [`convert::ToBytes`]),
/// unlike [`HookKey`]:
///
/// ```compile_fail
/// use rshooks::prelude::*;
/// use rshooks::ParamValue;
///
/// #[derive(ParamValue)]
/// struct NotAKey {
///     value: u8,
/// }
///
/// // ERROR: `NotAKey` has no `StateKeyEncode` impl (`ParamValue` never
/// // generates one, nor the `ToBytes` impl such an encoding would need).
/// let _ = state_get::<u64>(&NotAKey { value: 0 });
/// ```
pub use rshooks_macros::ParamValue;

// `txn_template!` expands `[<set_ $field>]` splice markers through
// `$crate::__paste!`, its own stable replacement for nightly's
// `${concat(...)}` (see `txn.rs`); re-export it (hidden) at the crate root
// so `$crate::__paste!` resolves regardless of which crate invokes
// `txn_template!`.
#[doc(hidden)]
pub use rshooks_macros::paste as __paste;

/// Common imports for hook developers: `use rshooks::prelude::*;` pulls in
/// the `api::*` wrapper functions, the typed slot layer
/// ([`slot_obj::SlotObject`] and the generated [`sfield`] constants), the
/// fixed-size buffer type aliases, the
/// [`xfl::XFL`]/[`xfl_unchecked::XFLUnchecked`]/[`tx_type::TxType`] types,
/// [`error::HookError`]/[`error::Result`], and the C-verbatim constant
/// families (`sfXxx`, `ttXxx`, `lsfXxx`, `tfXxx`, and `hookapi.h`'s
/// `KEYLET_*`/`COMPARE_*`/... constants). It deliberately does not
/// re-export all of `rshooks_core` — its raw `api::*` functions share names
/// with this crate's own wrappers (both define `state`, say) — only the
/// constant-only modules are pulled in, so there is no ambiguity.
///
/// # Two deliberate absences
///
/// - **The raw `sfcodes` glob.** [`sfield`]'s typed `SField<T>` constants
///   take those names, so `sfSequence` here is an `SField<u32>`, not a
///   `u32`. The raw table is still available at `rshooks::raw::sfcodes::*`
///   for const contexts — [`txn_template!`](crate::txn_template)'s field
///   tables, a `const` header expression — where `Into` cannot be called.
///   [`SField::code()`](slot_obj::SField::code) is the other bridge.
/// - **The numbered slot functions.** `slot_set`/`slot_clear`/
///   `slot_subfield`/`otxn_slot`/... address the same 255 registers
///   [`slot_obj::SlotObject`] manages, and mixing the two silently corrupts
///   handles. They stay public at `rshooks::api::slot::*` (plus
///   `rshooks::api::otxn::otxn_slot`) — see [`mod@api::slot`]'s module doc
///   comment.
pub mod prelude {
    // `api::*` minus the numbered slot functions — see "Two deliberate
    // absences" above.
    pub use crate::api::control::*;
    pub use crate::api::etxn::*;
    pub use crate::api::float::*;
    pub use crate::api::hook_ctx::*;
    pub use crate::api::keylet::*;
    pub use crate::api::ledger::*;
    // Everything `api::otxn` exports except `otxn_slot`, listed by name
    // (rather than globbed-then-shadowed) so adding one upstream is a
    // deliberate act.
    pub use crate::api::otxn::{
        OtxnFieldValue, otxn_burden, otxn_field, otxn_field_exact, otxn_field_typed,
        otxn_field_u64, otxn_generation, otxn_id, otxn_id_buf, otxn_param, otxn_param_exact,
        otxn_param_typed, otxn_type,
    };
    pub use crate::api::state::*;
    pub use crate::api::sto::*;
    pub use crate::api::trace::*;
    pub use crate::api::util::*;
    pub use crate::buf_eq::*;
    pub use crate::convert::{FixedRead, FromBytes, ToBytes, TypedParamName};
    pub use crate::decl::{HookParam, OtxnParam, State};
    pub use crate::error::{HookError, Result};
    pub use crate::exit::{Accept, HookResult, Rollback};
    pub use crate::ledger_entry_type::LedgerEntryType;
    pub use crate::macros::no_unroll;
    pub use crate::sfield::*;
    #[cfg(feature = "unstable-param-sig-interface")]
    pub use crate::sig::{
        Blob, IssueBytes, SigName, SigParamType, hook_sig_param, otxn_sig_param, otxn_sig_param_opt,
    };
    pub use crate::slot_obj::{AmountBytes, CastTarget, IssueData, SlotKey, SlotObject};
    pub use crate::state::{
        StateKeyEncode, TypedStateKey, state_delete, state_foreign_get, state_foreign_get_typed,
        state_foreign_set_loose, state_foreign_set_typed, state_foreign_update_loose,
        state_foreign_update_typed, state_get, state_get_typed, state_set_loose, state_set_typed,
        state_update_loose, state_update_typed,
    };
    pub use crate::static_cell::HookStatic;
    pub use crate::sto_writer::StoWriter;
    pub use crate::tx_type::TxType;
    pub use crate::types::*;
    pub use crate::views::ledger::LedgerEntryCommonFields;
    pub use crate::views::tx::{TransactionCommonFields, TransactionCommonSlotFields};
    pub use crate::xfl::XFL;
    pub use crate::xfl_unchecked::XFLUnchecked;
    // `XFL!` (macro) and `xfl::XFL` (type) live in separate namespaces, so
    // both glob imports coexist without ambiguity.
    pub use rshooks_macros::XFL;
    // `sfcodes::*` is deliberately absent — see "Two deliberate absences"
    // above.
    pub use rshooks_core::{consts::*, lets::*, ls_flags::*, tts::*, tx_flags::*};
}

/// Distinctive negative code used by the panic handler below when rolling
/// back. Chosen well outside the documented Hook API error-code range
/// (`-1..=-45`, plus the one irregular `-10024` for `INVALID_FLOAT`) so it
/// can never be confused with a real Hook API error.
#[cfg(all(target_arch = "wasm32", feature = "panic-handler"))]
const PANIC_ROLLBACK_CODE: i64 = -999_999;

/// Panic handler for wasm Hook binaries: rolls the hook back with a fixed
/// message instead of leaving an unhandled panic, which has no defined
/// behavior on the Hook host. This is a last-resort backstop, not the
/// primary correctness mechanism (see DESIGN.md §2 C7). Enabled by the
/// default-on `panic-handler` feature; disable it to supply your own.
#[cfg(all(target_arch = "wasm32", feature = "panic-handler"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        let _ = rshooks_core::rollback(b"panic".as_ptr() as u32, 5, PANIC_ROLLBACK_CODE);
    }
    core::arch::wasm32::unreachable()
}

/// Host-target panic handler for `no_std` hook crates, behind the
/// **non-default** `host-panic-handler` feature.
///
/// A hook crate is a `no_std` cdylib, so even a host `cargo check` (what
/// rust-analyzer runs for diagnostics) demands a `#[panic_handler]` — but
/// the wasm handler above is target-gated, and rshooks cannot provide one
/// unconditionally on the host: a `std` consumer (like rshooks's own test
/// harness) would then hit a duplicate lang item. Hook crates opt in via
/// `rshooks = { ..., features = ["host-panic-handler"] }` to make host
/// analysis work; the handler itself is never reached (host builds of hook
/// crates are for analysis only, not execution).
///
/// Additionally gated `not(feature = "testenv")`: a `[dev-dependencies]`
/// setup enabling both `host-panic-handler` and `testenv` unifies both into
/// one build via Cargo feature unification, and `std` already provides a
/// `#[panic_handler]` there (the test harness links `std`) — so this item
/// is simply not emitted rather than colliding with it.
#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "host-panic-handler",
    not(feature = "testenv")
))]
#[panic_handler]
#[allow(clippy::empty_loop)] // analysis-only target; never executed
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
