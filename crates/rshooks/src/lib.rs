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
mod macros;
pub mod sfield;
pub mod slot_obj;
pub mod state;
pub mod static_cell;
pub mod tx_type;
pub mod txn;
pub mod types;
pub mod xfl;
pub mod xfl_unchecked;

// `pad!` expands to `$crate::padded_bytes(...)`; the helper lives in the
// private `macros` module, so re-export it (hidden) at the crate root.
#[doc(hidden)]
pub use macros::padded_bytes;

// `pad_left!` expands to `$crate::padded_bytes_left(...)` — same reasoning
// as `padded_bytes` above.
#[doc(hidden)]
pub use macros::padded_bytes_left;

/// Direct re-export of `rshooks-core`: raw Hook API declarations and every
/// C-verbatim constant. See the crate doc comment for why this is a plain
/// alias rather than a re-exporting wrapper module.
pub use rshooks_core as raw;

/// Convenience re-export of [`decl`]'s ZST chain-declaration handle types —
/// the field types a `#[hooks]`-annotated struct declares against. See
/// [`decl`]'s module doc comment for the full story; the rest of that
/// module (`StateEntry`, `HookParamAt`, `OtxnParamAt`, the `*Spec` traits)
/// stays reachable at `decl::` rather than being re-exported here, since
/// those are the macro-generated side of the handshake, not something a
/// hook author names directly.
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

/// Turns a plain, argument-less `fn name() -> i64` into the Hook host's
/// required `hook` export (`#[unsafe(no_mangle)] pub extern "C" fn hook(
/// _reserved: u32) -> i64`). See `rshooks_macros::hook`'s doc comment for the
/// exact requirements and generated shape.
pub use rshooks_macros::hook;

/// Like [`hook`], but exports `cbak` instead — for the optional callback a
/// Hook module can export, invoked when a transaction it previously emitted
/// settles.
pub use rshooks_macros::cbak;

/// Declares a multi-hook chain: a `#[hooks]`-annotated struct declares the
/// chain's shared `State`/`HookParam`/`OtxnParam` schema (see [`decl`]),
/// and a `#[hooks]`-annotated inherent `impl` block on that same struct
/// declares its `#[hook(<index>, ..)]`/`#[cbak(<index>)]` entries — the v0.2
/// replacement for the top-level [`hook`]/[`cbak`]/`metadata!`/
/// `hook_state!`/`hook_parameter!`/`otxn_parameter!` combination (which stay
/// available for single-hook crates; see `docs/MULTI_HOOK_STRUCT_DESIGN.md`
/// for the full grammar, semantics, and migration guidance).
///
/// This is a declaration macro, not a value or type most hook code names
/// directly, so — like [`hook`]/[`cbak`]/`metadata!` — it is deliberately
/// left out of [`prelude`]; import it explicitly (`use rshooks::hooks;`).
pub use rshooks_macros::hooks;

/// Declares descriptive and SetHook-facing metadata for a Hook binary.
///
/// `rshooks build` extracts this declaration from cargo's raw wasm,
/// combines it with build-derived values such as `HookHash` and the
/// `hook`/`cbak` worst-case instruction counts, and writes a sibling JSON
/// artifact. The declaration itself is build-only: the macro carries compact
/// JSON in the name of a dead wasm export, and the normal cleaner removes that
/// export before hashing, validating, or writing the deployable wasm. It adds
/// no data segment, runtime code, import, or byte to the final Hook binary.
///
/// # Grammar
///
/// ```text
/// metadata! {
///     name: "non-empty display name",                 // required
///     description: "free-form description",          // optional
///     HookOn: [Payment, Invoke],                       // optional form 1
///     HookCanEmit: [Payment],                          // optional
///     HookName: "pay-hook",                           // optional
/// }
/// ```
///
/// `HookOn` is mutually exclusive with the directional form, in which both
/// arrays are required when either is present:
///
/// ```text
/// metadata! {
///     name: "directional hook",
///     IncomingHookOn: [Payment, Invoke],
///     OutgoingHookOn: [Payment],
/// }
/// ```
///
/// All three trigger fields may be omitted. In that case, the generated
/// sidecar represents the all-zero raw `HookOn` value as `null`.
///
/// Every transaction entry is a bare [`tx_type::TxType`] variant name (for
/// example `Payment`, not `TxType::Payment` or `ttPAYMENT`). The generated
/// code references the actual enum variant, so a misspelling is a compile
/// error. Duplicate entries are rejected. The incoming and outgoing arrays
/// must describe different sets, matching SetHook's directional-HookOn rule;
/// use `HookOn` when both directions are the same.
/// Transaction variants use Xahau's canonical `TransactionType` spellings,
/// including `SetHook`, `SetRegularKey`, and `AMMCreate`.
///
/// `HookName` is a Rust UTF-8 string containing 2 through 8 Unicode scalar
/// values. This metadata-level rule deliberately counts characters, not
/// encoded bytes. Note that the current xahaud SetHook validator independently
/// applies its ledger rule of 4 through 16 UTF-8 bytes; a name intended for
/// direct submission must satisfy both limits. `rshooks-build` warns when that
/// additional byte-oriented rule is not met.
///
/// # Example
///
/// ```
/// use rshooks::metadata;
///
/// metadata! {
///     name: "payment observer",
///     description: "Observes incoming and outgoing payments.",
///     IncomingHookOn: [Payment, Invoke],
///     OutgoingHookOn: [Payment],
///     HookName: "pay-hook",
/// }
/// ```
///
/// Only one declaration should be present in a Hook crate. It may appear at
/// module scope in the crate's `lib.rs`; it does not need to be referenced by
/// `hook` or `cbak`.
pub use rshooks_macros::metadata;

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
/// XFL is a decimal floating-point format (see [`xfl`]'s module doc
/// comment for the full bit layout): every value expressible in decimal —
/// `0.1`, `2600000`, `0.003333333333333333` — has exactly one correct XFL
/// encoding, and hand-computing that encoding's raw bit pattern (as a
/// `const DEFAULT_REWARD_RATE_BITS: i64 = 6_038_156_834_009_797_973;`,
/// say) is both opaque at the call site and exactly the kind of manual
/// arithmetic a compile-time macro exists to replace. `XFL!` takes the
/// decimal value itself:
///
/// ```
/// use rshooks::XFL;
/// use rshooks::xfl::XFL as XflType;
///
/// const DEFAULT_REWARD_RATE: XflType = XFL!(0.003333333333333333);
/// assert_eq!(DEFAULT_REWARD_RATE.raw_bits(), 6_038_156_834_009_797_973);
/// ```
///
/// Bit-exactness is the entire reason this macro parses the literal's text
/// by hand instead of routing it through `f64`: `f64` cannot represent
/// `0.1` (or most decimal fractions) exactly, so a `f64`-based encoder
/// would silently reintroduce the very rounding error XFL exists to avoid.
/// Every digit of the literal, up to XFL's 16-significant-digit limit, is
/// preserved exactly.
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
/// A hook-state *key* and a hook-state *value* share the same "fixed-offset,
/// named-field struct" shape, but play genuinely different roles, so this
/// crate keeps them as two narrower derives instead of one derive (and one
/// blanket impl) covering both:
///
/// - A key is only ever **encoded outward** — handed to `state`/
///   `state_foreign` to *locate* an entry — never read back and decoded as
///   itself (the *value* stored at that key is what gets read back).
///   `HookKey` reflects that by generating only [`convert::ToBytes`] plus
///   [`state::StateKeyEncode`]: no `FromBytes`, no `FixedRead`, no inherent
///   `LEN` const.
/// - A key has a bound the Hook API's fixed key space imposes — its real
///   encoded length must fit within 32 bytes — distinct from a value's lack
///   of any size cap (beyond this crate's own `MAX_TYPED_STATE_LEN`
///   convenience limit — see [`state`]'s module doc comment). `HookKey`
///   checks that bound **at derive time**, unconditionally: a
///   `#[derive(HookKey)]` struct that encodes to 33+ bytes fails to compile
///   at its own definition, before it is used as a key. The encoded key sent
///   to the host is **not** locally zero-padded up to 32 bytes — it is exactly
///   the struct's own natural
///   length, e.g. 2 bytes for a `{ tag: u8, small: u8 }`, and the host
///   itself left-pads a key shorter than 32 bytes (see [`state`]'s module
///   doc comment, "Key length and padding").
/// - Only a `#[derive(HookKey)]` struct, a [`state_keys!`](crate::state_keys)
///   enum, or [`types::StateKey`] itself implements
///   [`state::StateKeyEncode`] — an ordinary `#[derive(HookData)]` *value*
///   struct does **not** automatically qualify as a key, so a state value
///   can never be passed where a key is expected by accident.
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
///   order — identical codegen to [`HookData`]'s `ToBytes` impl (see its doc
///   comment's "What gets generated"/"Zero-cost by construction" sections);
///   this derive only adds the `StateKeyEncode` impl on top.
/// - `impl state::StateKeyEncode for Name`: encodes `self` via the `ToBytes`
///   impl above into an [`state::EncodedStateKey`] carrying exactly `Name`'s
///   own real encoded length (never padded to 32), with a compile-time
///   (monomorphized) assert that the length fits within 32 bytes.
///
/// # Real length, not zero-padded: the exact byte image sent to the host
///
/// The bytes [`state::StateKeyEncode::encode`] produces are `Name`'s fields,
/// little-endian, back-to-back — nothing more. A `{ tag: u8, small: u16 }`
/// key encodes to exactly 3 bytes, not 32:
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
/// composite state value via [`hook_state!`] and used with
/// [`state::state_get_typed`]/[`state::state_set_typed`] — no `state_keys!`
/// declaration, no hand-packed byte buffer, and (unlike the loose
/// [`state::state_get`]/[`state::state_set_loose`], which take the value
/// type as an independent generic parameter — see
/// [`state::TypedStateKey`]'s doc comment) no way to accidentally read/write
/// `DepositKey`'s entry as some other struct's value type:
///
/// ```
/// use rshooks::{HookData, HookKey};
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
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
/// hook_state!(DepositState, DepositKey => DepositValue);
///
/// assert_eq!(DepositValue::LEN, 8 + 4 + 1);
///
/// let key = DepositKey {
///     tag: 1,
///     owner: AccountId::default(),
/// };
///
/// // `NotImplemented` here is the host stub every Hook API call returns on
/// // a host build (see `rshooks-core`) — this only proves the generated
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
/// a value type as independent generic parameters, so nothing there stops
/// pairing a key with the *wrong* value type — [`hook_state!`] plus
/// [`state::state_set_typed`] closes that: `value`'s type is checked
/// against the key's own declared [`state::TypedStateKey::Value`], so
/// passing a value meant for a different key is a compile error (see
/// [`state::TypedStateKey`]'s doc comment for the full rationale):
///
/// ```compile_fail
/// use rshooks::{HookData, HookKey};
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
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
/// hook_state!(StateA, KeyA => ValueA);
///
/// // ERROR: `ValueB` is not `KeyA`'s declared `Value` (`ValueA`).
/// let _ = state_set_typed(&KeyA { tag: 0 }, &ValueB { amount: 0 });
/// ```
pub use rshooks_macros::HookKey;

/// Derives [`convert::ToBytes`]/[`convert::FromBytes`]/[`convert::FixedRead`]
/// for a fixed-size, named-field struct used as a **composite hook-state
/// value** — read back and decoded by `state_get`/`state_get_typed`, written by
/// `state_set_loose`/`state_set_typed`. See [`HookKey`] for the state-*key*
/// counterpart (and why it's a separate, narrower derive rather than
/// `HookData` also serving as a key), [`ParamName`] for the analogous Hook
/// API parameter-*name* role, and [`ParamValue`] for the analogous
/// parameter-*value* role (a `#[derive(HookData)]` struct also happens to
/// satisfy `ParamValue`'s `FromBytes`/`FixedRead` requirement and so *can*
/// be used as a parameter value directly — [`ParamValue`] is the narrower,
/// intent-revealing choice for a struct that is only ever a parameter
/// payload and never a state value).
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
/// - A plain (non-generic) struct with **named fields only** — no tuple
///   structs, no unit structs, no enums, no unions.
/// - At least one field.
/// - Every field's type must implement [`convert::ToBytes`] +
///   [`convert::FromBytes`]: any of this crate's fixed-size primitives
///   (`u8`/`u16`/`u32`/`u64`/`i64`), [`xfl::XFL`], any `rshooks::types`
///   newtype (`AccountId`, `Hash`, ...), a raw `[u8; N]`, or another
///   `#[derive(HookData)]` struct (nesting composes for free — see below).
///   A field of any other (variable-length) type fails to compile with an
///   ordinary rustc trait-bound error naming the missing `ToBytes`/
///   `FromBytes` impl against the generated code — this derive does not
///   implement its own type checker.
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
/// Every field offset is a compile-time constant (a chain of `const`
/// declarations built from each field's own `ToBytes::MAX_LEN`, resolved at
/// compile time), and every field read/write delegates straight to that
/// field's own `ToBytes::write`/`FromBytes::read` — the identical "fixed,
/// unrolled offsets, no runtime-computed length" shape this crate already
/// hand-writes for [`txn_template!`]'s generated setters. There is no
/// per-field loop, and (for a total size the toolchain still lowers to
/// inlined stores rather than a `memset`/`memcpy` builtin call — empirically
/// up to 32 bytes at this crate's release profile, see
/// [`state`]'s `MAX_TYPED_STATE_LEN` doc comment) no unguarded loop at all.
/// `examples/12_typed-data`'s README measures this directly: a
/// `#[derive(HookData)]` struct and a hand-packed equivalent compile to the
/// same worst-case instruction count.
///
/// # Nesting
///
/// A `#[derive(HookData)]` struct can itself be a field of another — since
/// the derive only ever requires a field's type to implement `ToBytes`/
/// `FromBytes`/`FixedRead`, and every derived struct does, nesting needs no
/// special support:
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
/// (`DepositKey`/`DepositValue`, paired via [`hook_state!`]). A `HookData`
/// struct also works directly as a state value with the loose
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
/// # Full byte image: fields are little-endian, back-to-back, in
/// declaration order
///
/// The examples above only check `Name::LEN`/round-trip-through-itself —
/// this one instead pins down `write()`'s exact output byte-for-byte
/// against a hand-built `expected` array, proving both each field's
/// little-endian encoding (see DESIGN.md §5.6, "Endianness conventions")
/// and its offset directly, not just that encode-then-decode is the
/// identity function:
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
/// unlike the pre-4-derive design, there is no blanket
/// [`state::StateKeyEncode`] impl over every `ToBytes` type, so this fails
/// to compile (use [`HookKey`] instead):
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

/// Declares a hook-state **entity** — the thing a hook operates on — with
/// the key that addresses it and the value it holds.
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// hook_state!(DepositState, DepositKey {tag: u8, owner: AccountId} => Deposit {amount: u64});
///
/// # fn f(owner: AccountId) -> Result<()> {
/// let deposit = DepositState { tag: 1, owner };
/// let current = deposit.get_state()?;
/// deposit.set_state(&Deposit { amount: 1 })?;
/// deposit.delete_state()?;
/// # Ok(())
/// # }
/// ```
///
/// The **entity** (`DepositState`) is the primary surface: it carries the
/// four accessors above, and it implements [`state::TypedStateKey`] itself,
/// so it also works with every free function this crate provides —
/// [`state::state_get_typed`], the loose `state_get`, the `_foreign` twins.
/// The **key** (`DepositKey`) stays declared as a trait carrier with no
/// inherent methods: the identifier component deserves a name, and it is
/// what you hand to those free functions when you want the component rather
/// than the entity.
///
/// A **grammar staircase** of six forms, from a fully-fixed key down to a
/// fully composite, runtime-constructed one — pick the narrowest one that
/// fits. Every form names an entity first, and every form's value side
/// (after `=>`) independently accepts either an already-declared type or an
/// *inline* definition (`=> Name { field: Type, .. }`, generating a fresh
/// `#[derive(HookData)]`-equivalent struct named `Name`).
///
/// | form | key shape | example |
/// |---|---|---|
/// | 1 | fully fixed (a new zero-sized type) | `hook_state!(RewardRate, RewardRateKey = b"RR" => XFL);` |
/// | 2 | struct, with a fixed instance | `hook_state!(Counter, CounterKey {name: [u8; 7]} = {name: *b"counter"} => u64);` |
/// | 3 | struct, constructed per call site | `hook_state!(DepositState, DepositKey {tag: u8, owner: AccountId} => Deposit);` |
/// | 4 | newtype (tuple struct) around one existing type | `hook_state!(AccountState, AccountKey AccountId => AccountData {balance: XFL, sequence: u16});` |
/// | `existing` | key impls on a key type **you** declared | `hook_state!(MyOwnState, existing MyOwnKey = b"MK" => u64);` |
/// | pairing | wraps a key type you declared, that already encodes | `hook_state!(MyState, MyKey => MyValue);` |
///
/// Forms 1–4 declare the key type; `existing` and the pairing form attach to
/// one you declared yourself. **Every** form declares the entity, and the
/// entity always carries the accessors — that is the one thing that does not
/// vary. Generated code never constructs your key type, which is what lets
/// the last two forms work with a key that is non-`Copy`, privately built,
/// or not constructible from the invocation site at all.
///
/// An optional leading visibility token applies to every item the invocation
/// declares (see "Visibility" below).
///
/// Every type name this macro itself *declares* — the entity, a Form 1–4
/// key, an inline value — must be `UpperCamelCase`: first character an
/// uppercase ASCII letter, no underscores, checked at the macro invocation
/// with a `compile_error!` naming the offending identifier. The entity, the
/// key and the value must also be three *different* names.
///
/// ```compile_fail
/// use rshooks::hook_state;
///
/// // ERROR: `reward_rate_key` is not UpperCamelCase.
/// hook_state!(RewardRate, reward_rate_key = b"RR" => u64);
/// ```
///
/// # Form 1: fully fixed key (zero-sized type)
///
/// `hook_state!($Entity, $Name = $bytes => $Value)` declares both `$Entity`
/// and `$Name` as new unit structs — each is a zero-sized marker whose own
/// name *is* its one value, the standard Rust shape (no separate `const`
/// needed, unlike Form 2 below). `$bytes` (typically a byte-string literal)
/// becomes the key's real, unpadded on-the-wire bytes (see [`state`]'s
/// module doc comment, "Key length and padding") — checked at compile time
/// to be `1..=32` bytes, the Hook API's own key-length bound. Both types
/// encode that same literal, so which one you hand to the host is a matter
/// of which reads better.
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// hook_state!(RewardRate, RewardRateKey = b"RR" => XFL);
///
/// // `NotImplemented` here is the host stub every Hook API call returns on
/// // a host build — this only proves the generated call chain compiles.
/// assert_eq!(RewardRate.get_state(), Err(HookError::NotImplemented));
/// // The key component reaches the same entry through the free functions.
/// assert_eq!(state_get_typed(&RewardRateKey), Err(HookError::NotImplemented));
/// ```
///
/// # Form 2: struct key with a fixed instance
///
/// `hook_state!($Entity, $Name { .. } = { .. } => $Value)` declares both as
/// named-field structs (identical codegen to `#[derive(HookKey)]`, applied
/// to each) **plus** a `const` of the *entity's* name holding the one fixed
/// instance — Rust allows this because a type name and a value name live in
/// separate namespaces. Use `$Entity` at the call site, exactly like Form 1:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// hook_state!(Counter, CounterKey {name: [u8; 7]} = {name: *b"counter"} => u64);
///
/// assert_eq!(Counter.get_state(), Err(HookError::NotImplemented));
/// ```
///
/// # Form 3: struct key, constructed per call site
///
/// `hook_state!($Entity, $Name { .. } => $Value)` — like Form 2 minus the
/// fixed instance: both types are declared (identical codegen to
/// `#[derive(HookKey)]`, applied to each) but each call site builds its own
/// `$Entity { .. }` literal, for a key whose fields vary at runtime (e.g.
/// keyed by the calling account):
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// hook_state!(DepositState, DepositKey {tag: u8, owner: AccountId} => Deposit {amount: u64, deadline: u32});
///
/// // `Deposit` (generated by the inline value definition above) derives
/// // nothing extra on its own — `assert!(.is_err())` rather than
/// // `assert_eq!` avoids needing `Debug`/`PartialEq` on it just for this
/// // doctest.
/// let deposit = DepositState { tag: 1, owner: AccountId::default() };
/// assert!(deposit.get_state().is_err());
///
/// // The entity mirrors the key's fields and encodes *itself* through the
/// // identical per-field codegen — same bytes, same cost, and no
/// // `DepositKey` value is built along the way.
/// let key = DepositKey { tag: 1, owner: AccountId::default() };
/// assert!(state_get_typed(&key).is_err());
/// ```
///
/// # Form 4: newtype (tuple struct) around one existing type
///
/// `hook_state!($Entity, $Name $Inner => $Value)` declares both as tuple
/// structs wrapping `$Inner` (`struct $Entity($Inner);`) — sugar for "this
/// key is just an existing type, under a distinct name so it can be paired
/// with exactly one value type." Construct with `$Entity(inner_value)`:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// hook_state!(AccountState, AccountKey AccountId => AccountData {balance: XFL, sequence: u16});
///
/// let account = AccountState(AccountId::default());
/// assert!(account.get_state().is_err());
/// ```
///
/// # Pairing form: an entity over a key you already declared
///
/// `hook_state!($Entity, $Key => $Value)`, `$Key`/`$Value` both types the
/// caller already declared (typically a `#[derive(HookKey)]`/
/// `#[derive(HookData)]` pair, or a [`state_keys!`] enum) — pairs them and
/// declares `struct $Entity($Key);`, whose accessors forward through
/// `&self.0`:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{hook_state, HookData, HookKey};
///
/// #[derive(HookKey, Clone, Copy)]
/// struct MyKey {
///     tag: u8,
/// }
///
/// #[derive(HookData, Clone, Copy, Debug, PartialEq)]
/// struct MyValue {
///     count: u32,
/// }
///
/// hook_state!(MyState, MyKey => MyValue);
///
/// assert_eq!(
///     MyState(MyKey { tag: 0 }).get_state(),
///     Err(HookError::NotImplemented)
/// );
/// ```
///
/// The entity forwards [`state::StateKeyEncode::encode`] straight to
/// `$Key`'s own implementation rather than re-deriving one, so `$Key` needs
/// only the trait it already has — a [`state_keys!`] enum (which has
/// `StateKeyEncode` but no [`convert::ToBytes`]) pairs exactly as well as a
/// `#[derive(HookKey)]` struct, and a non-`Copy` `$Key` works too, since
/// nothing here ever copies or constructs one.
///
/// `$Key` must be **local to your crate, already able to encode itself, and
/// not already paired** — see "What the pairing form requires of `$Key`"
/// below for each, and for why [`types::StateKey`] has to go through Form 4
/// (`hook_state!(MyState, MyKey StateKey => V)`) instead.
///
/// # `existing` form: impls only, on a key type you declared
///
/// `hook_state!($Entity, existing $Name = $bytes => $Value)` attaches
/// everything Form 1 generates for its key — the fixed key bytes, the
/// [`state::StateKeyEncode`] impl and the [`state::TypedStateKey`] pairing —
/// to a type **you** declared, instead of declaring one, and gives the
/// entity the same impls plus the accessors. Reach for it when the key type
/// needs something this macro does not generate: a visibility, a doc
/// comment, extra derives, an attribute:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// /// This hook's reward rate — public, because a sibling module reads it.
/// #[derive(Clone, Copy)]
/// pub struct RewardRateKey;
///
/// hook_state!(RewardRate, existing RewardRateKey = b"RR" => u64);
///
/// assert_eq!(RewardRate.get_state(), Err(HookError::NotImplemented));
/// // Your own type carries the encoding traits, usable as before.
/// assert_eq!(state_get_typed(&RewardRateKey), Err(HookError::NotImplemented));
/// ```
///
/// `existing` is a contextual keyword (a type of your own named `existing`
/// still works in the pairing form) and accepts only the fixed-bytes shape
/// above. Nothing generated ever *constructs* `RewardRateKey` — the entity
/// encodes the same literal itself — so the form works just as well for a
/// key type you cannot build from here.
///
/// Like the pairing form, `existing` is **module position only**: it emits
/// impls for a type the surrounding module owns, which from inside a
/// function body is a non-local definition (rustc's `non_local_definitions`
/// lint). Forms 1–4 declare everything themselves and work in either
/// position.
///
/// # Generated methods: `get_state`/`set_state`/`update_state`/`delete_state`
///
/// **Every** form puts the four state operations on the entity it declares,
/// each an `#[inline(always)]` forward to the free function of the same name
/// — `entity.get_state()` and `state::state_get_typed(&entity)` compile to
/// the same code, so the choice is purely about which reads better:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// hook_state!(BalanceState, BalanceKey {owner: AccountId} => Balance {drops: u64});
///
/// let balance = BalanceState { owner: AccountId::default() };
///
/// // All four are available; every call reaches the host stub here.
/// assert!(balance.get_state().is_err());
/// assert!(balance.set_state(&Balance { drops: 1 }).is_err());
/// assert!(balance.update_state(|_| Balance { drops: 1 }).is_err());
/// assert!(balance.delete_state().is_err());
///
/// // The entity is a first-class key everywhere else too — the loose,
/// // typed and `_foreign` free functions all take it.
/// assert!(state_foreign_get_typed(&balance, None, None).is_err());
/// ```
///
/// The **key type never gets them**. It is a trait carrier: putting four
/// method names on it would claim them on a type you may well own yourself
/// (`existing`, pairing), and would make the address of the thing look like
/// the thing. If your own inherent impl on the *entity* already defines one
/// of these names, that is a duplicate-definition error (`E0592`); if a
/// *trait* of yours defines it, the inherent method silently wins at the
/// call site — these four names are macro-owned API on a declared entity.
///
/// # Visibility
///
/// An optional leading `pub` (or `pub(crate)`, `pub(super)`, `pub(in path)`)
/// applies to **every** item the invocation declares — the entity and its
/// fields, the key struct and its fields, an inline value struct and its
/// fields, a Form 2 `const`. All-or-nothing; the default is private:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// mod ledger {
///     use rshooks::prelude::*;
///     use rshooks::hook_state;
///
///     // One leading `pub` covers the entity, the key and the value.
///     hook_state!(pub DepositState, DepositKeyName {tag: u8} => Deposit {amount: u64});
/// }
///
/// let deposit = ledger::DepositState { tag: 1 };
/// assert!(deposit.set_state(&ledger::Deposit { amount: 1 }).is_err());
/// ```
///
/// Making the *generated* items public does not make a **caller-owned** type
/// public. Every caller-owned type that ends up in a public generated field
/// or associated type — a pairing `$Key`, an already-declared `$Value`, a
/// Form 4 `$Inner`, any field type — must be at least as visible as the
/// invocation, or rustc reports its own `E0446`/`E0445` ("private type in
/// public interface") at the expansion. An `existing` form's key type is the
/// one exception: it never appears in the entity's public API, so a private
/// one under a `pub` invocation is fine.
///
/// # What the pairing form requires of `$Key`
///
/// The pairing form generates `impl TypedStateKey for $Key { .. }`, so
/// `$Key` has to satisfy three things — none of which the macro can check
/// for you, and each of which rustc reports in its own words:
///
/// - **Local to your crate.** Rust's orphan rule requires either the trait
///   or the implementing type to be local, and from a hook crate (never
///   `rshooks` itself) [`state::TypedStateKey`] is foreign. A bare
///   `[u8; N]` is `core`'s, not yours, and fails with `E0117`; so does
///   [`types::StateKey`]. Wrap either in Form 4 instead
///   (`hook_state!(MyState, MyKey [u8; 7] => u64)`), or reach for
///   [`state::state_get`]/[`state::state_set_loose`] — the *loose*,
///   independently-typed accessors — if it has no business being paired
///   with exactly one value type at all.
/// - **Already able to encode itself**: [`state::StateKeyEncode`] for a
///   state key (a `#[derive(HookKey)]` struct or a [`state_keys!`] enum),
///   [`convert::TypedParamName`] for a parameter name. The entity forwards
///   to that impl rather than deriving a second one — which is exactly why
///   a `state_keys!` enum, with no [`convert::ToBytes`] at all, pairs fine.
/// - **Not already paired.** A second pairing for the same `$Key` is a
///   second `impl TypedStateKey for $Key`, i.e. rustc's `E0119`
///   (conflicting implementations). One key, one value type — which is the
///   whole point of the pairing. Give the second entity its own key type,
///   or use Form 4 to wrap the same inner type under a new name.
///
/// ```compile_fail
/// use rshooks::prelude::*;
/// use rshooks::hook_state;
///
/// // ERROR (E0117): neither `TypedStateKey` nor `[u8; 7]` is local to
/// // this crate — wrap the key in a newtype (Form 4) instead.
/// hook_state!(BufState, [u8; 7] => u64);
/// ```
///
/// ```compile_fail
/// use rshooks::prelude::*;
/// use rshooks::{hook_state, HookData, HookKey};
///
/// #[derive(HookKey, Clone, Copy)]
/// struct MyKey {
///     tag: u8,
/// }
///
/// #[derive(HookData, Clone, Copy)]
/// struct MyValue {
///     count: u32,
/// }
///
/// hook_state!(FirstState, MyKey => MyValue);
///
/// // ERROR (E0119): `MyKey` is already paired — one key, one value type.
/// hook_state!(SecondState, MyKey => MyValue);
/// ```
pub use rshooks_macros::hook_state;

/// Derives [`convert::ToBytes`] (only — no [`convert::FromBytes`]/
/// [`convert::FixedRead`]) for a fixed-size, named-field struct used as a
/// **composite Hook API parameter name** — a name type implementing
/// [`convert::TypedParamName`] (see
/// [`hook_parameter!`](crate::hook_parameter)/
/// [`otxn_parameter!`](crate::otxn_parameter)'s composite form, `$Name =>
/// $Ty`, and [`crate::api::hook_ctx::hook_param_typed`]/
/// [`crate::api::otxn::otxn_param_typed`], which take **a reference to a name
/// value**). See [`HookKey`] for the analogous hook-state *key* role, and
/// [`ParamValue`] for the parameter *value* counterpart (what `$Ty` above
/// must satisfy).
///
/// # Why this derive doesn't implement [`convert::TypedParamName`] itself
///
/// This derive only ever generates [`convert::ToBytes`] for the annotated
/// struct — it does not implement [`convert::TypedParamName`] itself (that
/// trait additionally needs `type Value`, supplied by
/// [`hook_parameter!`](crate::hook_parameter)/
/// [`otxn_parameter!`](crate::otxn_parameter), which pairs the name type
/// with the value type it's read as — exactly like [`crate::hook_state!`]
/// pairs a [`HookKey`] type with its value type). `ParamName` (this derive)
/// and `TypedParamName` (that trait) share "name" in their names only in
/// the sense that both describe "the parameter name" concept from two
/// different angles (derive vs. runtime trait) — Rust's separate macro and
/// type namespaces mean this is not an identifier collision; the names are
/// kept parallel deliberately, for readability at the declaration site.
///
/// # Relationship to [`HookData`]
///
/// A hook-state value and a Hook API parameter *name* share the same
/// "fixed-offset struct" shape, but are genuinely different concepts —
/// `ParamName` is deliberately narrower than `HookData`, not just an alias
/// for it:
///
/// - A parameter name is only ever **written** (handed to
///   `hook_param`/`otxn_param` to locate a value) — never read back and
///   decoded as itself. `ParamName` reflects that by generating only
///   [`convert::ToBytes`]: no [`convert::FromBytes`], no
///   [`convert::FixedRead`], no inherent `LEN` const. Trying to read a
///   `#[derive(ParamName)]` type back as a value (or use it where
///   `hook_param_typed`/`otxn_param_typed` expect a value type) fails to compile
///   with an ordinary rustc trait-bound error naming the missing trait.
/// - A parameter name has its own length bound the Hook API itself
///   enforces — [`convert::PARAM_NAME_MAX_LEN`], **1 to 32 bytes**
///   (`hook_api.h`: `TOO_SMALL` below 1, `TOO_BIG` above 32) — the same
///   upper bound a hook state key's real encoded length is checked against
///   (see [`HookKey`]; a key is sent to the host at its own real length,
///   never locally zero-padded — the Hook API host itself left-pads a
///   shorter key), while a state *value* has no size cap at all.
///   `ParamName` checks this **at derive time**, unconditionally: a
///   `#[derive(ParamName)]` struct that encodes to 0 or to 33+ bytes fails
///   to compile at its own definition, before it's ever used as a
///   parameter name at all (contrast with [`HookKey`]'s analogous
///   derive-time check, which only has an upper bound — a key may be
///   shorter than 32 bytes, but a parameter name may not be shorter than 1
///   byte).
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
/// use rshooks::{ParamName, ParamValue, otxn_parameter};
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
/// otxn_parameter!(SeatVote, SeatParamName => Vote);
///
/// let seat = SeatVote(SeatParamName { topic: b'S', seat: 0 });
/// assert!(seat.get_value().is_err());
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

/// Declares a Hook API parameter name/value pairing, read via
/// [`api::hook_ctx::hook_param_typed`] (this hook's own installed
/// parameters) — implements [`convert::TypedParamName`] for the name side,
/// pairing it with the value side, so `hook_param_typed` can be called with
/// no turbofish and no chance of a name/value mismatch (see
/// [`convert::TypedParamName`]'s doc comment for why, and its comparison
/// table against [`state::TypedStateKey`]/[`hook_state!`](crate::hook_state)
/// for how this mirrors the hook-state side).
///
/// The identical **grammar staircase** [`hook_state!`](crate::hook_state)
/// uses — see its doc comment for the full table, the worked example of
/// each form, the entity's role, the generated accessors, and the optional
/// leading visibility. The only difference on this side is *which*
/// accessors the entity gets: `get_value` (plus `get_name` on the two
/// fixed-byte-string forms) rather than the state quartet, because a
/// parameter is read-only from the reading hook's own perspective.
/// [`otxn_parameter!`](crate::otxn_parameter) is this macro with
/// `otxn_param_typed` in place of `hook_param_typed`.
///
/// | form | name shape | example |
/// |---|---|---|
/// | 1 | fully fixed (a new zero-sized type) | `hook_parameter!(Cfg, CfgName = b"CFG" => Config);` |
/// | 2 | struct, with a fixed instance | see [`hook_state!`](crate::hook_state) — identical shape, applied to a name instead of a key |
/// | 3 | struct, constructed per call site | `hook_parameter!(SeatVote, SeatParamName {topic: u8, seat: u8} => Vote);` |
/// | 4 | newtype (tuple struct) around one existing type | see [`hook_state!`](crate::hook_state) — identical shape |
/// | `existing` | name impls on a name type **you** declared | `hook_parameter!(Cfg, existing CfgName = b"CFG" => Config);` |
/// | pairing | wraps a name type you declared, that already encodes | `hook_parameter!(SeatVote, SeatParamName => Vote);` |
///
/// # Form 1 and the `existing` form both take the zero-copy fast path
///
/// A **plain byte-string name** has nothing to compute: its wire encoding
/// *is* its in-memory representation. Both Form 1 (which additionally
/// declares `$Name` as a new unit struct) and the `existing` form (`$Name`
/// declared separately by the caller) give the entity *and* `$Name` an
/// override of
/// [`convert::TypedParamName::with_name_bytes`] to hand the literal
/// straight to the closure — no copy, no buffer, nothing to encode — at
/// the exact same cost as the loose [`api::hook_ctx::hook_param_exact`]
/// this replaces (see [`convert::TypedParamName`]'s doc comment, "Zero-cost
/// for the plain-byte-string case," and `examples/12_typed-data`'s README,
/// which measures this directly). Those same two forms expose the literal
/// as `$Entity.get_name() -> &'static [u8]`, a `const fn`; composite names
/// (Forms 2–4, pairing) get no `get_name` — encoding one is a runtime
/// computation, and `with_name_bytes` (an exact-size scratch buffer, handed
/// to a closure) is the way to reach those bytes.
///
/// # Every composite form gets a right-sized buffer too
///
/// Forms 2–4 can't skip encoding (a composite name genuinely has more than
/// one field to lay out), but each still overrides `with_name_bytes` to
/// encode into a buffer sized to exactly that name's own
/// [`convert::ToBytes::MAX_LEN`] — not the full 32-byte
/// [`convert::PARAM_NAME_MAX_LEN`] scratch the trait's generic default
/// falls back to — see [`convert::TypedParamName`]'s doc comment,
/// "Near-zero-cost for the composite case too." The **pairing** form
/// forwards to whatever `$Name` already had, so it inherits that name's
/// buffer (or its `'static` literal) rather than deriving a second one.
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamValue, hook_parameter};
///
/// #[derive(ParamValue)]
/// struct Config {
///     min_amount: u64,
/// }
///
/// hook_parameter!(Cfg, CfgName = b"CFG" => Config);
///
/// assert_eq!(Cfg.get_name(), b"CFG".as_slice());
/// assert_eq!(Cfg.get_value().err(), Some(HookError::NotImplemented));
///
/// // The name component reaches the same parameter through the free
/// // function — the entity's method is exactly this, inlined.
/// let cfg = hook_param_typed(&CfgName);
/// assert_eq!(cfg.err(), Some(HookError::NotImplemented));
/// ```
///
/// The `existing` form — `$Name` a marker type the caller declared
/// separately, for when it needs a visibility, a doc comment or derives
/// Form 1 does not generate for it. Nothing generated ever constructs
/// `$Name`; the entity encodes the same literal itself:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamValue, hook_parameter};
///
/// // `pub`, because `CfgName` is: `existing` gives *your* type the
/// // `TypedParamName` impl, and `type Value = Config` on a public type
/// // would otherwise leak a private one (rustc's `E0446`). This is the
/// // caller-owned-visibility precondition in miniature.
/// #[derive(ParamValue)]
/// pub struct Config {
///     min_amount: u64,
/// }
///
/// /// Names this hook's `CFG` install-time parameter.
/// pub struct CfgName;
/// hook_parameter!(Cfg, existing CfgName = b"CFG" => Config);
///
/// assert_eq!(Cfg.get_name(), b"CFG".as_slice());
/// assert_eq!(Cfg.get_value().err(), Some(HookError::NotImplemented));
///
/// // `CfgName` got the name-side impls it was named for.
/// let cfg = hook_param_typed(&CfgName);
/// assert_eq!(cfg.err(), Some(HookError::NotImplemented));
/// ```
///
/// `get_value()` fixes the role at the declaration, so a `hook_parameter!`
/// entity always reads this hook's own installed parameters and an
/// [`otxn_parameter!`](crate::otxn_parameter) entity always reads the
/// originating transaction's:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamValue, hook_parameter};
///
/// hook_parameter!(Cfg, CfgName = b"CFG" => Config {min_amount: u64});
///
/// fn min_amount() -> u64 {
///     Cfg.get_value().map(|c| c.min_amount).unwrap_or(1_000_000)
/// }
///
/// // Nothing is installed on a host build, so the fallback is what comes back.
/// assert_eq!(min_amount(), 1_000_000);
/// ```
///
/// # Form 3: composite name, constructed per call site
///
/// The value type is inferred from the name argument, no annotation
/// needed even inside `.is_err()`:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamValue, hook_parameter};
///
/// hook_parameter!(SeatVote, SeatParamName {topic: u8, seat: u8} => Vote {value: u8});
///
/// let seat = SeatVote { topic: b'S', seat: 0 };
/// assert!(seat.get_value().is_err());
/// ```
///
/// # Pairing form: an entity over a name you already declared
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamName, ParamValue, hook_parameter};
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
/// hook_parameter!(SeatVote, SeatParamName => Vote);
///
/// // The entity forwards `with_name_bytes` to `SeatParamName`'s own
/// // implementation, so the exact-size buffer that name already had is
/// // what the lookup uses.
/// let seat = SeatVote(SeatParamName { topic: b'S', seat: 0 });
/// assert!(seat.get_value().is_err());
/// ```
pub use rshooks_macros::hook_parameter;

/// Identical grammar to [`macro@hook_parameter`] — see its doc comment for
/// the full writeup and every form's worked example, including the entity's
/// role, the `existing` and pairing forms, the generated
/// `get_name`/`get_value` methods and the optional leading visibility, all
/// of which work here unchanged. Targets
/// [`api::otxn::otxn_param_typed`] (a parameter attached to the
/// *originating transaction*) instead of `hook_param_typed`; kept as a
/// separate macro purely so the declaration site documents which of
/// `hook_param`/`otxn_param` a name is meant for — both implement the
/// exact same [`convert::TypedParamName`] trait.
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamValue, otxn_parameter};
///
/// #[derive(ParamValue)]
/// struct Instruction {
///     action: u8,
/// }
///
/// otxn_parameter!(Ins, InsName = b"INS" => Instruction);
///
/// assert_eq!(Ins.get_name(), b"INS".as_slice());
/// assert_eq!(Ins.get_value().err(), Some(HookError::NotImplemented));
///
/// // The name component reaches the same parameter through the free
/// // function — the entity's method is exactly this, inlined.
/// let ins = otxn_param_typed(&InsName);
/// assert_eq!(ins.err(), Some(HookError::NotImplemented));
/// ```
///
/// A composite (struct-shaped) parameter name:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamName, ParamValue, otxn_parameter};
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
/// otxn_parameter!(SeatVote, SeatParamName => Vote);
///
/// let seat = SeatVote(SeatParamName { topic: b'S', seat: 0 });
/// assert!(seat.get_value().is_err());
/// ```
pub use rshooks_macros::otxn_parameter;

/// Derives [`convert::FromBytes`]/[`convert::FixedRead`] (only — no
/// [`convert::ToBytes`]) for a fixed-size, named-field struct used as a
/// **Hook API parameter value** — the `$Ty` in
/// [`hook_parameter!`](crate::hook_parameter)/
/// [`otxn_parameter!`](crate::otxn_parameter), read back and decoded by
/// [`api::hook_ctx::hook_param_typed`]/[`api::otxn::otxn_param_typed`] (and the
/// loose [`api::hook_ctx::hook_param_exact`]/
/// [`api::otxn::otxn_param_exact`]). See [`HookData`] for the analogous
/// hook-state *value* role, and [`ParamName`] for the parameter *name*
/// counterpart (what locates this value).
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
/// caller declared themselves, via
/// [`otxn_parameter!`](crate::otxn_parameter)'s `existing` form:
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::{ParamValue, otxn_parameter};
///
/// #[derive(ParamValue)]
/// struct Config {
///     min_amount: u64,
///     max_amount: u64,
/// }
///
/// struct CfgName;
/// otxn_parameter!(Cfg, existing CfgName = b"CFG" => Config);
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
/// `KEYLET_*`/`COMPARE_*`/... constants). Deliberately does NOT re-export
/// all of `rshooks_core` (its raw `api::*` functions share names with this
/// crate's own wrappers, e.g. both define `state`) — only the constant-only
/// modules are pulled in, so there is no ambiguity between a
/// prelude-imported name and a rshooks wrapper.
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
    // `api::*` minus the numbered slot functions: those address the same 255
    // registers `SlotObject` manages, and a stray `slot_clear(3)` next to a
    // live handle silently corrupts it. They stay public at
    // `rshooks::api::slot::*` (and `rshooks::api::otxn::otxn_slot`), where
    // reaching for them is at least visible at the call site — see
    // `crate::slot_obj`'s module doc comment.
    pub use crate::api::control::*;
    pub use crate::api::etxn::*;
    pub use crate::api::float::*;
    pub use crate::api::hook_ctx::*;
    pub use crate::api::keylet::*;
    pub use crate::api::ledger::*;
    // Everything `api::otxn` exports *except* `otxn_slot`, which is a slot
    // function and leaves for the reason above. Listed by name rather than
    // globbed-then-shadowed so that adding one upstream is a deliberate act:
    // dropping a non-slot API here would be a silent, unrelated break.
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
    pub use crate::sfield::*;
    pub use crate::slot_obj::{AmountBytes, CastTarget, IssueData, SlotKey, SlotObject};
    pub use crate::state::{
        StateKeyEncode, TypedStateKey, state_delete, state_foreign_get, state_foreign_get_typed,
        state_foreign_set_loose, state_foreign_set_typed, state_foreign_update_loose,
        state_foreign_update_typed, state_get, state_get_typed, state_set_loose, state_set_typed,
        state_update_loose, state_update_typed,
    };
    pub use crate::static_cell::HookStatic;
    pub use crate::tx_type::TxType;
    pub use crate::types::*;
    pub use crate::xfl::XFL;
    pub use crate::xfl_unchecked::XFLUnchecked;
    // The `XFL!` macro and the `xfl::XFL` type above share a name but live
    // in different namespaces (macro vs. type — the same relationship as
    // std's `Clone` trait and its `#[derive(Clone)]` macro), so both glob
    // imports coexist here without ambiguity.
    pub use rshooks_macros::XFL;
    // `sfcodes::*` is deliberately absent: `crate::sfield`'s typed `sfXxx`
    // constants take those names. The raw `u32` table is still there for
    // const contexts that need it — `rshooks::raw::sfcodes::*`.
    pub use rshooks_core::{consts::*, ls_flags::*, tts::*, tx_flags::*};
}

/// Distinctive negative code used by the panic handler below when rolling
/// back. Chosen well outside the documented Hook API error-code range
/// (`-1..=-45`, plus the one irregular `-10024` for `INVALID_FLOAT`) so it
/// can never be confused with a real Hook API error.
#[cfg(all(target_arch = "wasm32", feature = "panic-handler"))]
const PANIC_ROLLBACK_CODE: i64 = -999_999;

/// Panic handler for wasm Hook binaries: rolls the hook back with a fixed
/// message instead of leaving an unhandled panic (which has no defined
/// behavior on the Hook host and, per DESIGN.md §2 C7, panic machinery is
/// something rshooks is built to avoid needing in the first place — this
/// handler is the last-resort backstop, not the primary correctness
/// mechanism). Enabled by the default-on `panic-handler` feature; disable it
/// if a hook wants to supply its own.
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
/// rust-analyzer runs for completion and diagnostics) demands a
/// `#[panic_handler]` — but the wasm handler above is target-gated, and
/// rshooks cannot provide one unconditionally on the host: any `std`
/// consumer (like rshooks's own test harness) would then hit a duplicate
/// lang item. Hook crates opt in via
/// `rshooks = { ..., features = ["host-panic-handler"] }`, which makes
/// host analysis work; the handler itself is never reached (host builds of
/// hook crates are for analysis only, not execution).
#[cfg(all(not(target_arch = "wasm32"), feature = "host-panic-handler"))]
#[panic_handler]
#[allow(clippy::empty_loop)] // analysis-only target; never executed
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
