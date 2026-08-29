//! Typed chain-declaration handles: [`State`]/[`HookParam`]/[`OtxnParam`].
//!
//! This module is the runtime counterpart to the (forthcoming) `#[hooks]`
//! struct/impl macros: a `#[hooks]`-annotated struct declares its fields as
//! [`State<V, S>`]/[`HookParam<V, S>`]/[`OtxnParam<V, S>`] — zero-sized
//! handle types whose second generic parameter `S` is a macro-generated
//! marker ZST implementing [`StateSpec`]/[`ParamSpec`] (+ optionally
//! [`ParamDefault`]/[`ParamRequired`] for parameters) — and the generic
//! inherent impls below turn that declaration into `.get()`/`.set()`/... at
//! the field's own call site, with no per-field boilerplate written by
//! hand.
//!
//! # Why the accessors live here, not on the spec traits themselves
//!
//! [`StateSpec`]/[`ParamSpec`] are implemented by types *outside* this
//! crate (macro-generated marker structs in a downstream hook crate), and
//! Rust's orphan rules mean a downstream crate cannot add an inherent impl
//! to [`State`]/[`HookParam`]/[`OtxnParam`] itself, nor a blanket trait impl
//! that would give every `StateSpec`/`ParamSpec` implementor its own
//! `.get()` method (that would require the *trait itself* to declare
//! `.get()`, at which point every future accessor added here becomes a
//! breaking change for any downstream `impl StateSpec for ...`). Instead,
//! the marker traits declare only the *data* a spec carries (the value
//! type, the key/name argument shape, and how to compute a key/name from
//! those arguments) — [`State`]/[`HookParam`]/[`OtxnParam`]'s generic
//! inherent impls, defined once in this crate, are what turn that data into
//! callable methods. This is exactly why the ZST handle types
//! ([`State`]/[`HookParam`]/[`OtxnParam`]) exist at all rather than calling
//! `MySpec::get()` directly on the marker type.
//!
//! # `State` vs. [`mod@crate::state`]
//!
//! [`State<V, S>`]'s accessors are thin forwards onto [`mod@crate::state`]'s
//! internal `_encoded`-suffixed funnels (`state_get_encoded`,
//! `state_set_encoded`, `state_update_encoded`, `state_delete_encoded`,
//! `state_foreign_get_encoded`, `state_foreign_set_encoded`) — this module
//! adds no new decode logic of its own. [`StateSpec::with_key`] produces an
//! already-encoded key once (via [`StateSpec::encode_key`] by default, or a
//! `'static`-promoted literal for a macro-generated constant key — see
//! [`StateSpec::with_key`]'s doc comment) and hands it straight to the
//! matching funnel, so no [`StateKeyEncode::encode`] call happens on this
//! path at all — [`StateEntry`] (bound via [`State::at`]) calls the same
//! funnels directly with its own already-encoded key.
//!
//! # Params: absence vs. decode failure
//!
//! [`HookParam`]/[`OtxnParam`]'s `.get()` is built on the new
//! [`crate::api::hook_ctx::hook_param_opt`]/[`crate::api::otxn::otxn_param_opt`]
//! absence-aware reads, not the older [`crate::api::hook_ctx::hook_param_exact`]/
//! [`crate::api::otxn::otxn_param_exact`] — see those functions' doc
//! comments for the exact "absent is `Ok(None)`, a present-but-malformed
//! value is still `Err`, never confused" contract every accessor in this
//! module preserves. `.get_or_default()` only ever substitutes
//! [`ParamDefault::default_value`] for the *absent* case; a decode failure
//! still propagates as `Err`. `.get_required()` maps absence specifically
//! to [`crate::error::HookError::DoesntExist`].

use core::marker::PhantomData;

use crate::api::hook_ctx::hook_param_opt;
use crate::api::otxn::otxn_param_opt;
use crate::convert::{FixedRead, FromBytes, ToBytes};
use crate::error::{HookError, Result};
use crate::state::EncodedStateKey;

/// A ZST handle for a declared, constant-or-keyed hook state entry.
///
/// `V` is the state entry's value type; `S` is a macro-generated marker ZST
/// implementing [`StateSpec`] that supplies the actual key encoding and
/// pins down `V` as [`StateSpec::Value`]. See the module doc comment for
/// why the accessors live in this crate's generic inherent impls rather
/// than on `S` itself.
///
/// `PhantomData<fn() -> (V, S)>` rather than `PhantomData<(V, S)>` so this
/// handle is `Send`/`Sync`/`Copy` regardless of what `V`/`S` are — see
/// [`crate::types::SField`]'s doc comment for the same rationale: the
/// handle *produces* a `V` (via `S`), it does not hold one.
pub struct State<V, S> {
    _marker: PhantomData<fn() -> (V, S)>,
}

impl<V, S> State<V, S> {
    /// Creates a new handle. Zero-sized — this does not touch hook state by
    /// itself; call one of the accessor methods (available when `S`
    /// implements [`StateSpec`]) to actually read or write.
    #[inline(always)]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<V, S> Clone for State<V, S> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<V, S> Copy for State<V, S> {}

impl<V, S> Default for State<V, S> {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

/// A ZST handle for a declared, constant-or-keyed hook parameter (a Hook
/// API parameter attached to *this* hook — see [`crate::api::hook_ctx`]).
///
/// `V` is the parameter's decoded value type; `S` is a macro-generated
/// marker ZST implementing [`ParamSpec`] (+ optionally [`ParamDefault`]/
/// [`ParamRequired`]) that supplies the actual name encoding and pins down
/// `V` as [`ParamSpec::Value`]. See the module doc comment for why the
/// accessors live in this crate's generic inherent impls rather than on
/// `S` itself.
///
/// `PhantomData<fn() -> (V, S)>` rather than `PhantomData<(V, S)>` so this
/// handle is `Send`/`Sync`/`Copy` regardless of what `V`/`S` are — see
/// [`crate::types::SField`]'s doc comment for the same rationale.
pub struct HookParam<V, S> {
    _marker: PhantomData<fn() -> (V, S)>,
}

impl<V, S> HookParam<V, S> {
    /// Creates a new handle. Zero-sized — this does not read anything by
    /// itself; call one of the accessor methods (available when `S`
    /// implements [`ParamSpec`]) to actually read.
    #[inline(always)]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<V, S> Clone for HookParam<V, S> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<V, S> Copy for HookParam<V, S> {}

impl<V, S> Default for HookParam<V, S> {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

/// A ZST handle for a declared, constant-or-keyed originating-transaction
/// parameter (a Hook API parameter read via [`crate::api::otxn`]).
///
/// Identical shape to [`HookParam`], reading through
/// [`crate::api::otxn::otxn_param_opt`] instead of
/// [`crate::api::hook_ctx::hook_param_opt`] — see [`HookParam`]'s doc
/// comment for the full story.
///
/// `PhantomData<fn() -> (V, S)>` rather than `PhantomData<(V, S)>` so this
/// handle is `Send`/`Sync`/`Copy` regardless of what `V`/`S` are — see
/// [`crate::types::SField`]'s doc comment for the same rationale.
pub struct OtxnParam<V, S> {
    _marker: PhantomData<fn() -> (V, S)>,
}

impl<V, S> OtxnParam<V, S> {
    /// Creates a new handle. Zero-sized — this does not read anything by
    /// itself; call one of the accessor methods (available when `S`
    /// implements [`ParamSpec`]) to actually read.
    #[inline(always)]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<V, S> Clone for OtxnParam<V, S> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<V, S> Copy for OtxnParam<V, S> {}

impl<V, S> Default for OtxnParam<V, S> {
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

/// Declares how a [`State`] marker `S` encodes its hook-state key.
///
/// Implemented by a macro-generated marker ZST, never by hand in an
/// ordinary hook crate. [`Self::KeyArgs`] is `()` for a constant key (the
/// `#[state(key = ...)]` field form) or a caller-supplied type for a keyed
/// family (`#[state(key_by = ...)]`) — [`Self::encode_key`] computes the
/// actual [`EncodedStateKey`] from those arguments, once per call, so
/// [`State`]'s generic inherent impls never need to know which case they
/// are in.
pub trait StateSpec {
    /// The state entry's decoded value type.
    type Value: ToBytes + FromBytes;

    /// `()` for a constant key; a [`StateKeyEncode`](crate::state::StateKeyEncode)
    /// key type for a keyed family.
    type KeyArgs;

    /// Computes this spec's [`EncodedStateKey`] from `args`.
    fn encode_key(args: &Self::KeyArgs) -> EncodedStateKey;

    /// Computes this spec's key (via [`Self::encode_key`] by default) and
    /// hands it to `f`, returning whatever `f` returns. [`State`]'s
    /// constant-key accessors all route through this method rather than
    /// [`Self::encode_key`] directly, so a macro-generated literal
    /// `#[state(key = b"...")]` marker can override it to hand `f` a
    /// compile-time-computed, `'static`-promoted [`EncodedStateKey`]
    /// instead of re-encoding the same literal at runtime on every call —
    /// see [`crate::state::EncodedStateKey::from_short`].
    #[inline(always)]
    fn with_key<R>(args: &Self::KeyArgs, f: impl FnOnce(&EncodedStateKey) -> R) -> R {
        f(&Self::encode_key(args))
    }
}

/// Declares how a [`HookParam`]/[`OtxnParam`] marker `S` encodes its
/// parameter name.
///
/// Implemented by a macro-generated marker ZST, never by hand in an
/// ordinary hook crate. [`Self::NameArgs`] is `()` for a literal name (the
/// `#[hook_param(name = ...)]`/`#[otxn_param(name = ...)]` field form) or a
/// caller-supplied type for a name family
/// (`#[hook_param(name_by = ...)]`/`#[otxn_param(name_by = ...)]`) —
/// [`Self::with_name_bytes`] hands the encoded name bytes to a closure
/// (mirroring [`crate::convert::TypedParamName::with_name_bytes`]'s
/// zero-copy-for-literals shape) so [`HookParam`]/[`OtxnParam`]'s generic
/// inherent impls never need to know which case they are in.
pub trait ParamSpec {
    /// The parameter's decoded value type.
    type Value: FixedRead;

    /// `()` for a literal name; a [`crate::convert::ToBytes`] name type for
    /// a name family.
    type NameArgs;

    /// Encodes this spec's name bytes (from `args`) and hands them to `f`,
    /// returning whatever `f` returns.
    fn with_name_bytes<R>(args: &Self::NameArgs, f: impl FnOnce(&[u8]) -> R) -> R;
}

/// Extends [`ParamSpec`] with a default value substituted for an absent
/// parameter — backs [`HookParam::get_or_default`]/[`OtxnParam::get_or_default`]
/// (and their `.at(...)`-bound [`HookParamAt`]/[`OtxnParamAt`] twins).
/// Implemented by a macro-generated marker ZST for a field carrying
/// `#[hook_param(default = ...)]`/`#[otxn_param(default = ...)]`.
pub trait ParamDefault: ParamSpec {
    /// The value substituted when the parameter is absent. Never consulted
    /// when the parameter is present, even if present-but-undecodable (that
    /// case is still `Err` — see the module doc comment's "Params: absence
    /// vs. decode failure" section).
    fn default_value() -> Self::Value;
}

/// Extends [`ParamSpec`] with "absence is an error" semantics — backs
/// [`HookParam::get_required`]/[`OtxnParam::get_required`] (and their
/// `.at(...)`-bound [`HookParamAt`]/[`OtxnParamAt`] twins). Implemented by a
/// macro-generated marker ZST for a field carrying
/// `#[hook_param(required)]`/`#[otxn_param(required)]`. Carries no
/// additional data — a marker trait purely to gate which accessor a given
/// `S` supports.
pub trait ParamRequired: ParamSpec {}

/// One `#[hook]`/`#[cbak]` entry of a `#[hooks]`-declared chain, exposed to
/// native (non-wasm) code with the exact call shape the wasm `extern "C"`
/// wrapper uses (`fn(u32) -> i64`) — generated by the `#[hooks]` impl
/// macro into a [`HookChainEntries::ENTRIES`] table, one row per declared
/// entry. Not feature-gated: generated code cannot see a downstream
/// crate's Cargo features, so this type (and [`HookChainEntries`]) are
/// unconditionally present on every non-wasm target — inert items with no
/// wasm presence at all. `rshooks-testenv`'s `TestEnv::invoke` looks up an
/// entry in this table and calls its `hook`/`cbak` function pointer
/// directly, the same plain-Rust-ABI function the wasm wrapper forwards
/// to.
#[cfg(not(target_arch = "wasm32"))]
pub struct NativeEntry {
    /// The entry's declared index (the `#[hook(<index>, ..)]` argument).
    pub index: u32,
    /// The entry's declared name (the annotated method's identifier).
    pub name: &'static str,
    /// The entry's plain-Rust-ABI body — identical to the function the
    /// wasm `hook`/discovery wrapper forwards to.
    pub hook: fn(u32) -> i64,
    /// The entry's `#[cbak(<index>)]` body, if one was declared for this
    /// index.
    pub cbak: Option<fn(u32) -> i64>,
    /// The transaction types this entry's `#[hook(.., can_emit = [..])]`
    /// declares it may emit — the on-chain `HookCanEmit` semantics,
    /// preserved through the three-state distinction: `None` means no
    /// `can_emit` was declared at all (unrestricted — matches an absent
    /// on-chain `HookCanEmit`, which imposes no restriction), while
    /// `Some(&[])` means `can_emit = []` was declared explicitly (deny-all:
    /// this entry may emit nothing). `Some(&[Tx, ..])` is the ordinary
    /// declared-list case. Never conflate `None` with `Some(&[])` — a
    /// downstream strict-mode check (`rshooks-testenv`'s
    /// `TestEnv::strict_can_emit`) relies on this distinction to tell
    /// "unrestricted" apart from "declared to emit nothing".
    pub can_emit: Option<&'static [crate::tx_type::TxType]>,
}

/// Implemented by generated code on a `#[hooks]`-annotated chain struct:
/// the native (non-wasm) counterpart to the wasm `extern "C"` entry
/// exports, listing every declared entry as a [`NativeEntry`]. See
/// [`NativeEntry`]'s doc comment for why this is unconditional rather than
/// `testenv`-gated.
#[cfg(not(target_arch = "wasm32"))]
pub trait HookChainEntries {
    /// One row per declared `#[hook(<index>, ..)]` entry, in no
    /// particular order — look up by [`NativeEntry::index`].
    const ENTRIES: &'static [NativeEntry];
}

impl<V, S> State<V, S>
where
    V: ToBytes + FromBytes,
    S: StateSpec<Value = V, KeyArgs = ()>,
{
    /// Reads this entry, decoded as `V`. `Ok(None)` means no entry exists —
    /// see [`crate::state::state_get`]'s doc comment for the full "absent
    /// vs. decode failure" contract this forwards to unchanged.
    #[inline(always)]
    pub fn get(&self) -> Result<Option<V>> {
        S::with_key(&(), crate::state::state_get_encoded::<V>)
    }

    /// Writes this entry, encoding `value` as `V`. Returns the number of
    /// bytes written.
    #[inline(always)]
    pub fn set(&self, value: &V) -> Result<usize> {
        S::with_key(&(), |key| crate::state::state_set_encoded::<V>(key, value))
    }

    /// Read-modify-writes this entry: reads the current value (or `None` if
    /// absent), calls `f` to compute the next value, writes it back, and
    /// returns the number of bytes written.
    #[inline(always)]
    pub fn update(&self, f: impl FnOnce(Option<V>) -> V) -> Result<usize> {
        S::with_key(&(), |key| {
            crate::state::state_update_encoded::<V, _>(key, f)
        })
    }

    /// Deletes this entry. See [`crate::state::state_delete`]'s doc comment
    /// for why deletion has no distinct "not found" failure.
    #[inline(always)]
    pub fn delete(&self) -> Result<()> {
        S::with_key(&(), crate::state::state_delete_encoded)
    }

    /// Reads this entry belonging to another namespace/account, decoded as
    /// `V`. `ns`/`acct` follow [`crate::api::state::state_foreign`]'s
    /// `Option` convention. `Ok(None)` means no entry exists.
    #[inline(always)]
    pub fn get_foreign(&self, ns: Option<&[u8]>, acct: Option<&[u8]>) -> Result<Option<V>> {
        S::with_key(&(), |key| {
            crate::state::state_foreign_get_encoded::<V>(key, ns, acct)
        })
    }

    /// Writes this entry belonging to another namespace/account, encoding
    /// `value` as `V`. `ns`/`acct` follow
    /// [`crate::api::state::state_foreign`]'s `Option` convention. Returns
    /// the number of bytes written.
    #[inline(always)]
    pub fn set_foreign(&self, value: &V, ns: Option<&[u8]>, acct: Option<&[u8]>) -> Result<usize> {
        S::with_key(&(), |key| {
            crate::state::state_foreign_set_encoded::<V>(key, value, ns, acct)
        })
    }
}

impl<V, S> State<V, S>
where
    S: StateSpec<Value = V>,
{
    /// Binds key arguments, returning an [`StateEntry`] with the same
    /// accessor set. On a constant-key spec (`S::KeyArgs = ()`), this is
    /// also reachable as `.at(())`.
    #[inline(always)]
    pub fn at(&self, args: S::KeyArgs) -> StateEntry<V> {
        StateEntry {
            key: S::encode_key(&args),
            _marker: PhantomData,
        }
    }
}

/// An encoded-key view of a [`State`] entry, bound to concrete key
/// arguments via [`State::at`] — same accessor set as [`State`]'s
/// constant-key inherent impls, forwarding to the same
/// [`mod@crate::state`] free functions through the already-computed
/// [`EncodedStateKey`] this holds.
///
/// `PhantomData<fn() -> V>` rather than `PhantomData<V>` so this view is
/// `Send`/`Sync`/`Copy` regardless of what `V` is — see
/// [`crate::types::SField`]'s doc comment for the same rationale: this
/// holds an already-encoded key, not a `V`.
pub struct StateEntry<V> {
    key: EncodedStateKey,
    _marker: PhantomData<fn() -> V>,
}

impl<V> Clone for StateEntry<V> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for StateEntry<V> {}

impl<V: ToBytes + FromBytes> StateEntry<V> {
    /// Reads this entry, decoded as `V`. `Ok(None)` means no entry exists.
    #[inline(always)]
    pub fn get(&self) -> Result<Option<V>> {
        crate::state::state_get_encoded::<V>(&self.key)
    }

    /// Writes this entry, encoding `value` as `V`. Returns the number of
    /// bytes written.
    #[inline(always)]
    pub fn set(&self, value: &V) -> Result<usize> {
        crate::state::state_set_encoded::<V>(&self.key, value)
    }

    /// Read-modify-writes this entry: reads the current value (or `None` if
    /// absent), calls `f` to compute the next value, writes it back, and
    /// returns the number of bytes written.
    #[inline(always)]
    pub fn update(&self, f: impl FnOnce(Option<V>) -> V) -> Result<usize> {
        crate::state::state_update_encoded::<V, _>(&self.key, f)
    }

    /// Deletes this entry.
    #[inline(always)]
    pub fn delete(&self) -> Result<()> {
        crate::state::state_delete_encoded(&self.key)
    }

    /// Reads this entry belonging to another namespace/account, decoded as
    /// `V`. `ns`/`acct` follow [`crate::api::state::state_foreign`]'s
    /// `Option` convention. `Ok(None)` means no entry exists.
    #[inline(always)]
    pub fn get_foreign(&self, ns: Option<&[u8]>, acct: Option<&[u8]>) -> Result<Option<V>> {
        crate::state::state_foreign_get_encoded::<V>(&self.key, ns, acct)
    }

    /// Writes this entry belonging to another namespace/account, encoding
    /// `value` as `V`. `ns`/`acct` follow
    /// [`crate::api::state::state_foreign`]'s `Option` convention. Returns
    /// the number of bytes written.
    #[inline(always)]
    pub fn set_foreign(&self, value: &V, ns: Option<&[u8]>, acct: Option<&[u8]>) -> Result<usize> {
        crate::state::state_foreign_set_encoded::<V>(&self.key, value, ns, acct)
    }
}

impl<V, S> HookParam<V, S>
where
    V: FixedRead,
    S: ParamSpec<Value = V, NameArgs = ()>,
{
    /// Reads this parameter, decoded as `V`. `Ok(None)` means the parameter
    /// is absent; a present-but-undecodable parameter is still `Err` — see
    /// the module doc comment's "Params: absence vs. decode failure"
    /// section.
    #[inline(always)]
    pub fn get(&self) -> Result<Option<V>> {
        S::with_name_bytes(&(), hook_param_opt::<V>)
    }
}

impl<V, S> HookParam<V, S>
where
    V: FixedRead,
    S: ParamSpec<Value = V, NameArgs = ()> + ParamDefault,
{
    /// Reads this parameter, decoded as `V`, substituting
    /// [`ParamDefault::default_value`] when absent. A present-but-
    /// undecodable parameter is still `Err`, never masked by the default.
    #[inline(always)]
    pub fn get_or_default(&self) -> Result<V> {
        match self.get()? {
            Some(value) => Ok(value),
            None => Ok(S::default_value()),
        }
    }
}

impl<V, S> HookParam<V, S>
where
    V: FixedRead,
    S: ParamSpec<Value = V, NameArgs = ()> + ParamRequired,
{
    /// Reads this parameter, decoded as `V`, treating absence as
    /// [`HookError::DoesntExist`] rather than `None`.
    ///
    /// Caveat: this maps *absence* to [`HookError::DoesntExist`]. No
    /// in-tree [`FixedRead`] decoder ever returns that same error code for
    /// a present-but-malformed value (they return [`HookError::TooSmall`]
    /// instead) — but if a value type's own `FixedRead` impl ever did
    /// return `DoesntExist` while decoding a *present* value, that case
    /// would be indistinguishable from true absence here.
    #[inline(always)]
    pub fn get_required(&self) -> Result<V> {
        match self.get()? {
            Some(value) => Ok(value),
            None => Err(HookError::DoesntExist),
        }
    }
}

impl<V, S> HookParam<V, S>
where
    S: ParamSpec<Value = V>,
{
    /// Binds name arguments, returning a [`HookParamAt`] with the same
    /// accessor set. On a literal-name spec (`S::NameArgs = ()`), this is
    /// also reachable as `.at(())`.
    #[inline(always)]
    pub fn at(&self, args: S::NameArgs) -> HookParamAt<V, S> {
        HookParamAt {
            args,
            _marker: PhantomData,
        }
    }
}

/// A name-arguments-bound view of a [`HookParam`], via [`HookParam::at`] —
/// same accessor set as [`HookParam`]'s literal-name inherent impls.
///
/// `PhantomData<fn() -> V>` rather than `PhantomData<V>` so this view does
/// not needlessly restrict `Send`/`Sync`/`Copy` based on `V` — see
/// [`crate::types::SField`]'s doc comment for the same rationale: this
/// holds name arguments, not a `V`.
pub struct HookParamAt<V, S: ParamSpec> {
    args: S::NameArgs,
    _marker: PhantomData<fn() -> V>,
}

impl<V, S> HookParamAt<V, S>
where
    V: FixedRead,
    S: ParamSpec<Value = V>,
{
    /// Reads this parameter, decoded as `V`. `Ok(None)` means the parameter
    /// is absent; a present-but-undecodable parameter is still `Err`.
    #[inline(always)]
    pub fn get(&self) -> Result<Option<V>> {
        S::with_name_bytes(&self.args, hook_param_opt::<V>)
    }

    /// Reads this parameter, decoded as `V`, substituting
    /// [`ParamDefault::default_value`] when absent. A present-but-
    /// undecodable parameter is still `Err`, never masked by the default.
    #[inline(always)]
    pub fn get_or_default(&self) -> Result<V>
    where
        S: ParamDefault,
    {
        match self.get()? {
            Some(value) => Ok(value),
            None => Ok(S::default_value()),
        }
    }

    /// Reads this parameter, decoded as `V`, treating absence as
    /// [`HookError::DoesntExist`] rather than `None`.
    ///
    /// Caveat: this maps *absence* to [`HookError::DoesntExist`]. No
    /// in-tree [`FixedRead`] decoder ever returns that same error code for
    /// a present-but-malformed value (they return [`HookError::TooSmall`]
    /// instead) — but if a value type's own `FixedRead` impl ever did
    /// return `DoesntExist` while decoding a *present* value, that case
    /// would be indistinguishable from true absence here.
    #[inline(always)]
    pub fn get_required(&self) -> Result<V>
    where
        S: ParamRequired,
    {
        match self.get()? {
            Some(value) => Ok(value),
            None => Err(HookError::DoesntExist),
        }
    }
}

impl<V, S> OtxnParam<V, S>
where
    V: FixedRead,
    S: ParamSpec<Value = V, NameArgs = ()>,
{
    /// Reads this parameter off the originating transaction, decoded as
    /// `V`. `Ok(None)` means the parameter is absent; a
    /// present-but-undecodable parameter is still `Err` — see the module
    /// doc comment's "Params: absence vs. decode failure" section.
    #[inline(always)]
    pub fn get(&self) -> Result<Option<V>> {
        S::with_name_bytes(&(), otxn_param_opt::<V>)
    }
}

impl<V, S> OtxnParam<V, S>
where
    V: FixedRead,
    S: ParamSpec<Value = V, NameArgs = ()> + ParamDefault,
{
    /// Reads this parameter, decoded as `V`, substituting
    /// [`ParamDefault::default_value`] when absent. A present-but-
    /// undecodable parameter is still `Err`, never masked by the default.
    #[inline(always)]
    pub fn get_or_default(&self) -> Result<V> {
        match self.get()? {
            Some(value) => Ok(value),
            None => Ok(S::default_value()),
        }
    }
}

impl<V, S> OtxnParam<V, S>
where
    V: FixedRead,
    S: ParamSpec<Value = V, NameArgs = ()> + ParamRequired,
{
    /// Reads this parameter, decoded as `V`, treating absence as
    /// [`HookError::DoesntExist`] rather than `None`.
    ///
    /// Caveat: this maps *absence* to [`HookError::DoesntExist`]. No
    /// in-tree [`FixedRead`] decoder ever returns that same error code for
    /// a present-but-malformed value (they return [`HookError::TooSmall`]
    /// instead) — but if a value type's own `FixedRead` impl ever did
    /// return `DoesntExist` while decoding a *present* value, that case
    /// would be indistinguishable from true absence here.
    #[inline(always)]
    pub fn get_required(&self) -> Result<V> {
        match self.get()? {
            Some(value) => Ok(value),
            None => Err(HookError::DoesntExist),
        }
    }
}

impl<V, S> OtxnParam<V, S>
where
    S: ParamSpec<Value = V>,
{
    /// Binds name arguments, returning an [`OtxnParamAt`] with the same
    /// accessor set. On a literal-name spec (`S::NameArgs = ()`), this is
    /// also reachable as `.at(())`.
    #[inline(always)]
    pub fn at(&self, args: S::NameArgs) -> OtxnParamAt<V, S> {
        OtxnParamAt {
            args,
            _marker: PhantomData,
        }
    }
}

/// A name-arguments-bound view of an [`OtxnParam`], via [`OtxnParam::at`] —
/// same accessor set as [`OtxnParam`]'s literal-name inherent impls.
///
/// `PhantomData<fn() -> V>` rather than `PhantomData<V>` so this view does
/// not needlessly restrict `Send`/`Sync`/`Copy` based on `V` — see
/// [`crate::types::SField`]'s doc comment for the same rationale: this
/// holds name arguments, not a `V`.
pub struct OtxnParamAt<V, S: ParamSpec> {
    args: S::NameArgs,
    _marker: PhantomData<fn() -> V>,
}

impl<V, S> OtxnParamAt<V, S>
where
    V: FixedRead,
    S: ParamSpec<Value = V>,
{
    /// Reads this parameter, decoded as `V`. `Ok(None)` means the parameter
    /// is absent; a present-but-undecodable parameter is still `Err`.
    #[inline(always)]
    pub fn get(&self) -> Result<Option<V>> {
        S::with_name_bytes(&self.args, otxn_param_opt::<V>)
    }

    /// Reads this parameter, decoded as `V`, substituting
    /// [`ParamDefault::default_value`] when absent. A present-but-
    /// undecodable parameter is still `Err`, never masked by the default.
    #[inline(always)]
    pub fn get_or_default(&self) -> Result<V>
    where
        S: ParamDefault,
    {
        match self.get()? {
            Some(value) => Ok(value),
            None => Ok(S::default_value()),
        }
    }

    /// Reads this parameter, decoded as `V`, treating absence as
    /// [`HookError::DoesntExist`] rather than `None`.
    ///
    /// Caveat: this maps *absence* to [`HookError::DoesntExist`]. No
    /// in-tree [`FixedRead`] decoder ever returns that same error code for
    /// a present-but-malformed value (they return [`HookError::TooSmall`]
    /// instead) — but if a value type's own `FixedRead` impl ever did
    /// return `DoesntExist` while decoding a *present* value, that case
    /// would be indistinguishable from true absence here.
    #[inline(always)]
    pub fn get_required(&self) -> Result<V>
    where
        S: ParamRequired,
    {
        match self.get()? {
            Some(value) => Ok(value),
            None => Err(HookError::DoesntExist),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;
    use crate::state::StateKeyEncode as _;

    // `State`/`HookParam`/`OtxnParam` themselves must stay zero-sized — a
    // `#[hooks]`-declared struct field of one of these types adds nothing
    // to the struct's own layout.
    #[test]
    fn handles_are_zero_sized() {
        assert_eq!(core::mem::size_of::<State<u32, MockConstState>>(), 0);
        assert_eq!(core::mem::size_of::<HookParam<u32, MockConstParam>>(), 0);
        assert_eq!(core::mem::size_of::<OtxnParam<u32, MockConstParam>>(), 0);
    }

    #[test]
    // Deliberately calls `.clone()` on a `Copy` type to exercise the
    // hand-written `Clone` impl itself (clippy would otherwise steer this
    // toward a plain copy, which wouldn't prove `Clone` is implemented).
    #[allow(clippy::clone_on_copy)]
    fn handles_are_clone_and_copy() {
        let a: State<u32, MockConstState> = State::new();
        let b = a;
        let _ = a; // still usable: `State` is `Copy`, not moved by `let b = a`.
        let _ = b.clone();
    }

    // --- Mock specs exercising the const-key/const-name convenience path ---

    struct MockConstState;

    impl StateSpec for MockConstState {
        type Value = u32;
        type KeyArgs = ();

        #[inline(always)]
        fn encode_key(_args: &()) -> EncodedStateKey {
            b"MK".encode()
        }
    }

    struct MockConstParam;

    impl ParamSpec for MockConstParam {
        type Value = [u8; 4];
        type NameArgs = ();

        #[inline(always)]
        fn with_name_bytes<R>(_args: &(), f: impl FnOnce(&[u8]) -> R) -> R {
            f(b"MK")
        }
    }

    impl ParamDefault for MockConstParam {
        #[inline(always)]
        fn default_value() -> [u8; 4] {
            [1, 2, 3, 4]
        }
    }

    impl ParamRequired for MockConstParam {}

    #[test]
    fn const_state_smoke_not_implemented_on_host() {
        let entry: State<u32, MockConstState> = State::new();
        assert_eq!(entry.get(), Err(HookError::NotImplemented));
        assert_eq!(entry.set(&1), Err(HookError::NotImplemented));
        assert_eq!(entry.update(|_| 1), Err(HookError::NotImplemented));
        assert_eq!(entry.delete(), Err(HookError::NotImplemented));
        assert_eq!(
            entry.get_foreign(None, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            entry.set_foreign(&1, None, None),
            Err(HookError::NotImplemented)
        );
    }

    #[test]
    fn state_at_unit_matches_const_key_path() {
        let handle: State<u32, MockConstState> = State::new();
        let via_at = handle.at(());
        assert_eq!(via_at.get(), handle.get());
    }

    #[test]
    fn const_hook_param_smoke_not_implemented_on_host() {
        let param: HookParam<[u8; 4], MockConstParam> = HookParam::new();
        assert_eq!(param.get(), Err(HookError::NotImplemented));
        assert_eq!(param.get_or_default(), Err(HookError::NotImplemented));
        assert_eq!(param.get_required(), Err(HookError::NotImplemented));
    }

    #[test]
    fn const_otxn_param_smoke_not_implemented_on_host() {
        let param: OtxnParam<[u8; 4], MockConstParam> = OtxnParam::new();
        assert_eq!(param.get(), Err(HookError::NotImplemented));
        assert_eq!(param.get_or_default(), Err(HookError::NotImplemented));
        assert_eq!(param.get_required(), Err(HookError::NotImplemented));
    }

    // --- Mock specs exercising the keyed/`.at(args)` path ---

    struct MockKeyedState;

    impl StateSpec for MockKeyedState {
        type Value = u32;
        type KeyArgs = u8;

        #[inline(always)]
        fn encode_key(args: &u8) -> EncodedStateKey {
            [*args].encode()
        }
    }

    struct MockKeyedParam;

    impl ParamSpec for MockKeyedParam {
        type Value = [u8; 4];
        type NameArgs = u8;

        #[inline(always)]
        fn with_name_bytes<R>(args: &u8, f: impl FnOnce(&[u8]) -> R) -> R {
            f(&[*args])
        }
    }

    #[test]
    fn keyed_state_at_smoke_not_implemented_on_host() {
        let handle: State<u32, MockKeyedState> = State::new();
        let entry = handle.at(7);
        assert_eq!(entry.get(), Err(HookError::NotImplemented));
        assert_eq!(entry.set(&1), Err(HookError::NotImplemented));
        assert_eq!(entry.update(|_| 1), Err(HookError::NotImplemented));
        assert_eq!(entry.delete(), Err(HookError::NotImplemented));
    }

    #[test]
    fn keyed_hook_param_at_smoke_not_implemented_on_host() {
        let handle: HookParam<[u8; 4], MockKeyedParam> = HookParam::new();
        let bound = handle.at(7);
        assert_eq!(bound.get(), Err(HookError::NotImplemented));
    }

    #[test]
    fn keyed_otxn_param_at_smoke_not_implemented_on_host() {
        let handle: OtxnParam<[u8; 4], MockKeyedParam> = OtxnParam::new();
        let bound = handle.at(7);
        assert_eq!(bound.get(), Err(HookError::NotImplemented));
    }

    #[test]
    fn different_keyed_args_encode_to_different_keys() {
        assert_ne!(
            MockKeyedState::encode_key(&1).as_ref(),
            MockKeyedState::encode_key(&2).as_ref()
        );
    }
}
