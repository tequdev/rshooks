//! Typed slot handles: [`SlotObject<T>`] and [`SField<T>`].
//!
//! The Hook API's slot family is a set of 255 numbered registers holding
//! deserialized ledger objects and transactions. The raw layer
//! ([`crate::api::slot`]) exposes it exactly as the host does — you pass slot
//! numbers around as `u32` and remember what is in each one. This module
//! puts a type on the handle instead:
//!
//! ```
//! use rshooks::prelude::*;
//!
//! # fn f(accid: &AccountId) -> Result<()> {
//! let account = SlotObject::from_keylet(&keylet_account(accid)?)?;
//! let seq: u32 = account.get(sfSequence)?.value()?;
//! let bal: XFL = account.get(sfBalance)?.as_xfl()?;
//! # Ok(())
//! # }
//! ```
//!
//! Slot numbers are auto-assigned by the host and never appear in hook
//! source. The field constant carries the value type, so `value()` needs no
//! turbofish and `account.get(0)` — indexing an object — is a compile error.
//!
//! Field codes stay [`SField`]s in the other direction too:
//! [`field_code`](SlotObject::field_code) hands back an
//! `SField<Opaque>` — erased, because a slot's field code says nothing this
//! layer can type — which compares directly against any generated constant
//! (`slot.field_code()? == sfBalance`) and unwraps with `.code()` when a raw
//! `u32` is what is wanted.
//!
//! # Affine handles: no `Copy`, no `Clone`, no `Drop`
//!
//! A [`SlotObject`] is an **affine** resource: it can be used at most once
//! in the ways that end its life ([`clear`](SlotObject::clear), a terminal
//! read, a retype), and it cannot be duplicated. That is not ceremony — a
//! `Copy` handle plus a consuming `clear` would let a stale copy read or
//! clear a slot the host has since reassigned to something else entirely,
//! silently operating on an unrelated object.
//!
//! There is deliberately **no `Drop`**. Cleanup is fallible (it is a host
//! call that can report `DOESNT_EXIST`) and would cost instructions on every
//! exit path a hook has, including the ones that never touched a slot. The
//! budget is per-execution — 255 slots, all released by the host when the
//! hook returns — so leaking a slot inside one execution costs nothing but
//! the slot itself. Affinity is what makes that safe: a leaked handle is
//! gone from the type system, not floating around able to alias.
//!
//! # Terminal reads consume, and do not clear
//!
//! [`value`](SlotObject::value), [`as_xfl`](SlotObject::as_xfl),
//! [`raw`](SlotObject::raw) and [`raw_exact`](SlotObject::raw_exact) take
//! `self`. The handle is spent; the slot keeps its contents until the hook
//! ends. That is byte-for-byte the C cost model — a C `slot_subfield`
//! followed by a `slot()` read leaks the slot identically, and an implicit
//! clear would tax every read with a host call the C idiom does not pay.
//!
//! When a loop would otherwise burn through the 255-slot budget, the
//! opt-in [`take_value`](SlotObject::take_value) /
//! [`take_xfl`](SlotObject::take_xfl) /
//! [`take_raw_exact`](SlotObject::take_raw_exact) family reads *and* clears,
//! on both the success and the failure path. Budget math: one slot per
//! `get`, 255 per execution; a 300-iteration loop that derives one child per
//! iteration must use `take_*` (or clear explicitly) or it will run out.
//!
//! # Do not mix this with the numbered slot functions
//!
//! [`crate::api::slot`]'s numbered functions (`slot_set`, `slot_clear`,
//! `slot_subfield`, ...) and this module address the same 255 registers.
//! Calling `slot_clear(3)` while a `SlotObject` happens to hold slot 3 will
//! corrupt that handle's meaning — the handle stays valid-looking and starts
//! describing whatever lands there next. This is a **logic** hazard, not a
//! memory-safety one (no `unsafe` is involved on either side), so it is not
//! prevented, only documented: pick one layer per hook. The numbered
//! functions are deliberately out of the prelude and reachable only through
//! explicit paths (`rshooks::api::slot::slot_clear`,
//! `rshooks::api::otxn::otxn_slot`) so that mixing them is at least always
//! visible at the call site.
//!
//! # Stack buffers and the optimization profile
//!
//! Every **built-in typed decoder** here uses a fixed stack buffer of at most
//! 48 bytes — the widest thing this layer decodes is an IOU amount, and the
//! issue path reads 44. At `opt-level` 1–3 rustc lowers a `[0u8; 48]`
//! zero-init to a handful of inlined stores; below that it can emit a
//! `memset` call, which is an unguarded loop the Hook API's guard checker
//! rejects. Examples in this repo build at `opt-level = 3`; a hook crate
//! that lowers its profile needs to re-check its own output, the same caveat
//! [`crate::api::keylet`] documents.
//!
//! The raw escapes are the caller's responsibility:
//! [`raw_exact::<N>`](SlotObject::raw_exact) and
//! [`take_raw_exact::<N>`](SlotObject::take_raw_exact) allocate `[0u8; N]`
//! for whatever `N` you name, so a large one reintroduces exactly that
//! `memset` and the guard rejection with it. [`raw`](SlotObject::raw) writes
//! into a buffer you supply and so has the same property. Keep `N` small, or
//! check the built output.

use core::marker::PhantomData;

use crate::api;
use crate::convert::FixedRead;
use crate::error::{HookError, Result};
use crate::types::{
    AccountId, Amount, CurrencyCode, Hash, IouAmount, Issue, IssuedAsset, Keylet, NativeAmount,
    Opaque, SField, STArray, STObject,
};
use crate::xfl::XFL;

/// Sealing module: the traits below carry a supertrait that only this crate
/// can name, so downstream crates cannot add implementations.
///
/// This is not stylistic. [`Resolve::resolve`] turns a parent slot number
/// into a child slot number — a downstream implementation could hand back an
/// arbitrary number and forge a [`SlotObject`] aliasing a slot something
/// else already owns, defeating the affinity that makes the handles safe in
/// the first place. Likewise [`CastTargetSealed`] decides which serialized
/// type IDs a retype accepts; a downstream impl could accept everything.
mod private {
    use crate::error::Result;

    /// How a navigation key derives a child slot from a parent slot.
    pub trait Resolve {
        /// Derives the child slot, auto-assigning its number.
        fn resolve(self, parent: u32) -> Result<u32>;
    }

    /// Which serialized type IDs a [`super::CastTarget`] accepts.
    pub trait CastTargetSealed {
        /// Whether `type_id` (a field code's high half) is this target.
        fn accepts(type_id: u32) -> bool;
    }
}

// ---------------------------------------------------------------------------
// Navigation keys
// ---------------------------------------------------------------------------

/// A key that can navigate from a `SlotObject<Parent>` to a child slot.
///
/// Parent-aware on purpose: a field code addresses a field of an object, an
/// index addresses an element of an array, and mixing them is a mistake the
/// type system can catch. `SlotObject<STObject>::get(0)` and
/// `SlotObject<STArray>::get(sfAccount)` do not compile.
///
/// **Sealed** — see [`private`]. Implemented for exactly two key kinds:
/// [`SField<T>`] (over `STObject` and `Opaque`) and `u32` (over `STArray`
/// and `Opaque`).
pub trait SlotKey<Parent>: private::Resolve {
    /// The marker type of the slot this key navigates to.
    type Out;
}

impl<T> private::Resolve for SField<T> {
    #[inline(always)]
    fn resolve(self, parent: u32) -> Result<u32> {
        // `0` asks the host to auto-assign the child slot.
        api::slot::slot_subfield(parent, self.code(), 0)
    }
}

impl<T> SlotKey<STObject> for SField<T> {
    type Out = T;
}

impl<T> SlotKey<Opaque> for SField<T> {
    type Out = T;
}

impl private::Resolve for u32 {
    #[inline(always)]
    fn resolve(self, parent: u32) -> Result<u32> {
        api::slot::slot_subarray(parent, self, 0)
    }
}

impl SlotKey<STArray> for u32 {
    /// An array element is always an object.
    type Out = STObject;
}

impl SlotKey<Opaque> for u32 {
    type Out = STObject;
}

// ---------------------------------------------------------------------------
// Cast targets
// ---------------------------------------------------------------------------

/// A marker type [`SlotObject::try_cast`] can check for at runtime.
///
/// The check compares the slot's **serialized type ID** — the high half of
/// the field code [`crate::api::slot::slot_type`] reports — against what the
/// target accepts. Every target accepts exactly its own ID, with one
/// deliberate exception: [`STObject`] also accepts the high object codes
/// (10001–10004) that root slots report, because a slot loaded straight from
/// a transaction or a ledger entry is an object even though its "field code"
/// names the whole thing rather than a field of something.
///
/// **Sealed** — see [`private`].
pub trait CastTarget: private::CastTargetSealed {}

/// The lowest object code a root slot (`from_otxn`/`from_meta`/
/// `from_keylet`/`from_txn_hash`) reports from `slot_type(no, 0)`.
const ROOT_OBJECT_CODE_MIN: u32 = 10001;
/// The highest such code.
const ROOT_OBJECT_CODE_MAX: u32 = 10004;

macro_rules! cast_target {
    ($ty:ty, $id:expr, $what:literal) => {
        impl private::CastTargetSealed for $ty {
            #[inline(always)]
            fn accepts(type_id: u32) -> bool {
                type_id == $id
            }
        }
        #[doc = concat!("Accepts serialized type ID ", stringify!($id), " (", $what, ").")]
        impl CastTarget for $ty {}
    };
}

impl private::CastTargetSealed for STObject {
    #[inline(always)]
    fn accepts(type_id: u32) -> bool {
        // 14 is a nested object field; the 10001+ codes are what a root slot
        // reports (`sfTransaction`/`sfLedgerEntry` and friends).
        type_id == 14 || (ROOT_OBJECT_CODE_MIN..=ROOT_OBJECT_CODE_MAX).contains(&type_id)
    }
}

/// Accepts serialized type ID 14 (`STObject`) and the 10001–10004 root
/// object codes.
impl CastTarget for STObject {}

cast_target!(STArray, 15, "STArray");
cast_target!(Amount, 6, "Amount");
cast_target!(Issue, 24, "Issue");
cast_target!(u8, 16, "UInt8");
cast_target!(u16, 1, "UInt16");
cast_target!(u32, 2, "UInt32");
cast_target!(u64, 3, "UInt64");
cast_target!(Hash, 5, "Hash256");
cast_target!(AccountId, 8, "AccountID");
cast_target!(CurrencyCode, 26, "Currency");

// ---------------------------------------------------------------------------
// SlotObject
// ---------------------------------------------------------------------------

/// A handle to one loaded slot, typed by what it holds.
///
/// See the [module documentation](self) for the affinity rules, the
/// consuming-read cost model, and the warning about mixing this with the
/// numbered slot functions.
#[must_use = "a SlotObject owns a slot until the hook ends; read it, clear it, or bind it"]
pub struct SlotObject<T = Opaque> {
    no: u32,
    _t: PhantomData<fn() -> T>,
}

impl SlotObject<STObject> {
    /// Loads the ledger object a keylet points at.
    ///
    /// ```
    /// use rshooks::prelude::*;
    ///
    /// # fn f(accid: &AccountId) -> Result<()> {
    /// let account = SlotObject::from_keylet(&keylet_account(accid)?)?;
    /// let _seq: u32 = account.get(sfSequence)?.value()?;
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn from_keylet(keylet: &Keylet) -> Result<Self> {
        api::slot::slot_set(keylet.as_ref(), 0).map(Self::wrap)
    }

    /// Loads a transaction by its hash.
    #[inline(always)]
    pub fn from_txn_hash(hash: &Hash) -> Result<Self> {
        api::slot::slot_set(hash.as_ref(), 0).map(Self::wrap)
    }

    /// Loads the originating transaction.
    #[inline(always)]
    pub fn from_otxn() -> Result<Self> {
        api::otxn::otxn_slot(0).map(Self::wrap)
    }

    /// Loads the originating transaction's metadata.
    ///
    /// Only available in the `cbak` callback — outside it the host reports
    /// an error, which surfaces here unchanged.
    #[inline(always)]
    pub fn from_meta() -> Result<Self> {
        api::slot::meta_slot(0).map(Self::wrap)
    }
}

impl<T> SlotObject<T> {
    /// Wraps a slot number the host just assigned.
    #[inline(always)]
    fn wrap(no: u32) -> Self {
        Self {
            no,
            _t: PhantomData,
        }
    }

    /// The slot's size in bytes.
    ///
    /// Borrows, so it composes with a following consuming read — which is
    /// the point: sizing a buffer must not spend the handle.
    #[inline(always)]
    pub fn size(&self) -> Result<u32> {
        api::slot::slot_size(self.no)
    }

    /// The slot's serialized field code (`slot_type(no, 0)`), as an
    /// [`SField`] you can compare against the generated constants:
    ///
    /// ```
    /// use rshooks::prelude::*;
    ///
    /// # fn f(txn: &SlotObject<STObject>) -> Result<()> {
    /// let amount = txn.get(sfAmount)?;
    /// assert!(amount.field_code()? == sfAmount);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The type parameter is [`Opaque`] because the value type is *not*
    /// statically known here — this is an erased code, good for comparison
    /// (`SField` equality is on the code alone, across type parameters, so it
    /// matches a constant of any `T`) and for `.code()` extraction, not for
    /// deciding how to read the slot. Use [`try_cast`](Self::try_cast) or
    /// [`assume_type`](Self::assume_type) for that.
    ///
    /// A slot derived by navigation reports the field it came from. A **root**
    /// slot — `from_otxn`, `from_meta`, `from_keylet`, `from_txn_hash` —
    /// instead reports a high-level object code, because there is no
    /// enclosing object it is a field of: the value is still a packed field
    /// code, but its serialized type ID (`code >> 16`) lands in the
    /// 10001–10004 range (`sfTransaction`, `sfLedgerEntry`, ...) rather than
    /// among the ordinary 1–26 type IDs. [`try_cast::<STObject>`](Self::try_cast)
    /// accepts those codes for exactly that reason; note that a root slot's
    /// code therefore compares *unequal* to every constant in
    /// [`crate::sfield`], all of which are ordinary field codes.
    #[inline(always)]
    pub fn field_code(&self) -> Result<SField<Opaque>> {
        api::slot::slot_type(self.no, 0).map(SField::new)
    }

    /// Navigates to a child slot, auto-assigning its number.
    ///
    /// Borrows: one parent legitimately yields several children, and reading
    /// two fields of the same object should not require re-loading it. The
    /// aliasing hazard affinity guards against is clear-then-reuse, not
    /// several live children.
    ///
    /// The key decides the child's type: a [`SField<T>`] yields
    /// `SlotObject<T>` (from `slot_subfield`), a `u32` index yields
    /// `SlotObject<STObject>` (from `slot_subarray`).
    ///
    /// ```
    /// use rshooks::prelude::*;
    ///
    /// # fn f(accid: &AccountId) -> Result<()> {
    /// let signers = SlotObject::from_keylet(&keylet_signers(accid)?)?;
    /// let entries = signers.get(sfSignerEntries)?;      // SlotObject<STArray>
    /// let first = entries.get(0u32)?;                   // SlotObject<STObject>
    /// let who: AccountId = first.get(sfAccount)?.value()?;
    /// # let _ = who;
    /// # Ok(())
    /// # }
    /// ```
    #[inline(always)]
    pub fn get<K: SlotKey<T>>(&self, key: K) -> Result<SlotObject<K::Out>> {
        <K as private::Resolve>::resolve(key, self.no).map(SlotObject::wrap)
    }

    /// Releases the slot, consuming the handle.
    ///
    /// The only way to give a slot back before the hook ends. Fallible
    /// because the host call is; the error is almost always `DOESNT_EXIST`
    /// on a slot that was never populated.
    #[inline(always)]
    pub fn clear(self) -> Result<()> {
        api::slot::slot_clear(self.no).map(|_| ())
    }

    /// Reads the slot's raw bytes into `buf`, returning how many were
    /// written. Consumes the handle (see the module docs).
    ///
    /// The escape hatch for anything this layer does not type: the bytes are
    /// exactly what the host holds, in wire (big-endian) order. Nothing here
    /// interprets them — in particular [`crate::convert::FromBytes`] and
    /// [`crate::convert::FixedRead`] are *little*-endian guest conventions
    /// and must not be pointed at slot bytes.
    #[inline(always)]
    pub fn raw<B: AsMut<[u8]> + ?Sized>(self, buf: &mut B) -> Result<usize> {
        api::slot::slot(buf, self.no)
    }

    /// Reads exactly `N` bytes, erroring if the slot is not exactly that
    /// size. Consumes the handle.
    ///
    /// `N` is yours to choose, and so is the consequence: this allocates
    /// `[0u8; N]` on the stack, and a large `N` makes rustc emit a `memset`
    /// call instead of inlined stores — an unguarded loop the Hook API's
    /// guard checker rejects. The built-in typed reads all stay at or under
    /// 48 bytes for that reason; if you need more, check the built output
    /// (see this module's "Stack buffers" section).
    #[inline(always)]
    pub fn raw_exact<const N: usize>(self) -> Result<[u8; N]> {
        let no = self.no;
        read_exact_bytes::<N>(no)
    }

    /// [`raw_exact`](Self::raw_exact), then clears the slot — on the success
    /// path *and* the failure path.
    ///
    /// The loop-body form: recycling the slot is what keeps a long loop
    /// inside the 255-slot budget. The clear's own result is discarded,
    /// because the read's result is the one the caller asked for and a
    /// failed clear cannot be acted on usefully here.
    ///
    /// Same `N` caveat as [`raw_exact`](Self::raw_exact): the buffer is
    /// yours to size, and a large one reintroduces `memset`.
    #[inline(always)]
    pub fn take_raw_exact<const N: usize>(self) -> Result<[u8; N]> {
        let no = self.no;
        let out = read_exact_bytes::<N>(no);
        let _ = api::slot::slot_clear(no);
        out
    }

    /// Retypes the handle after checking the slot's serialized type ID.
    ///
    /// On **any** failure — a type mismatch or an error from the underlying
    /// `slot_type` call — the handle is consumed and the slot is
    /// best-effort cleared: a cast that did not hold means the caller was
    /// wrong about what is there and is done with it.
    #[inline(always)]
    pub fn try_cast<U: CastTarget>(self) -> Result<SlotObject<U>> {
        let no = self.no;
        match api::slot::slot_type(no, 0) {
            Ok(code) if <U as private::CastTargetSealed>::accepts(code >> 16) => {
                Ok(SlotObject::wrap(no))
            }
            Ok(_) => {
                let _ = api::slot::slot_clear(no);
                Err(HookError::NotAnObject)
            }
            Err(e) => {
                let _ = api::slot::slot_clear(no);
                Err(e)
            }
        }
    }

    /// Retypes the handle **without** checking, consuming it.
    ///
    /// `const` and free. The escape hatch for when the caller knows the slot's
    /// contents from context the type system cannot see — reading a field
    /// whose `SField` this layer types as [`Opaque`], say. Getting it wrong
    /// is not memory-unsafe (every read still bounds-checks against the
    /// slot's real size); it just produces a decode error or a nonsense
    /// value. Prefer [`try_cast`](Self::try_cast) unless the check is
    /// measurably in the way.
    #[inline(always)]
    pub const fn assume_type<U>(self) -> SlotObject<U> {
        SlotObject {
            no: self.no,
            _t: PhantomData,
        }
    }
}

/// Reads exactly `N` bytes out of `no`, erroring when the slot's size is not
/// exactly `N`.
///
/// Factored out of [`SlotObject::raw_exact`]/[`SlotObject::take_raw_exact`]
/// so the two share one definition of "exact".
#[inline(always)]
fn read_exact_bytes<const N: usize>(no: u32) -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    let written = api::slot::slot(&mut buf, no)?;
    if written == N {
        Ok(buf)
    } else {
        Err(HookError::TooSmall)
    }
}

// ---------------------------------------------------------------------------
// Container-only operations
// ---------------------------------------------------------------------------

impl SlotObject<STArray> {
    /// The number of elements in the array. Borrows.
    #[inline(always)]
    pub fn count(&self) -> Result<u32> {
        api::slot::slot_count(self.no)
    }
}

impl SlotObject<Opaque> {
    /// The number of elements, if the slot in fact holds an array. Borrows.
    #[inline(always)]
    pub fn count(&self) -> Result<u32> {
        api::slot::slot_count(self.no)
    }

    /// Whether the slot holds a native (XAH) amount, if it holds an amount
    /// at all. Borrows.
    ///
    /// Available here as well as on [`SlotObject<Amount>`] because an
    /// `Opaque` slot is exactly the case where the caller does *not* yet
    /// know what is in it — asking is the point. `slot_type(no, 1)` reports
    /// `1` for native and **`0` for non-native** (not "IOU" specifically);
    /// a slot that is not an amount at all reports `NOT_AN_AMOUNT`, which
    /// surfaces here as an error.
    #[inline(always)]
    pub fn is_native(&self) -> Result<bool> {
        api::slot::slot_type(self.no, 1).map(|v| v == 1)
    }
}

// ---------------------------------------------------------------------------
// Amount
// ---------------------------------------------------------------------------

/// A serialized amount, classified by its length.
///
/// MPT amounts (33 bytes) are **out of scope** for this layer — they need an
/// amendment Xahau does not have. An unexpected length is reported as
/// [`HookError::ParseError`] rather than guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountBytes {
    /// An 8-byte native (XAH) amount.
    Native(NativeAmount),
    /// A 48-byte IOU amount.
    Iou(IouAmount),
}

/// A serialized issue, classified by its length. `Iou` holds the decoded
/// asset identity as an [`IssuedAsset`] — the same type
/// [`IouAmount::asset`] produces from a 48-byte IOU `Amount`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueData {
    /// A 20-byte native issue.
    Native,
    /// A 40-byte IOU issue: currency and issuer.
    Iou(IssuedAsset),
}

impl SlotObject<Amount> {
    /// Whether the amount is native (XAH) rather than an IOU.
    ///
    /// Borrows, so it can precede a consuming [`as_xfl`](Self::as_xfl).
    /// `slot_type(no, 1)` reports `1` for native and **`0` for non-native** —
    /// note that `0` means "not native", not "IOU" specifically.
    #[inline(always)]
    pub fn is_native(&self) -> Result<bool> {
        api::slot::slot_type(self.no, 1).map(|v| v == 1)
    }

    /// Reads the amount as an [`XFL`], consuming the handle.
    ///
    /// A direct `slot_float` call — no pre-checks, no branches, exactly the
    /// cost of the raw host call.
    ///
    /// For a **native** amount the result is in **XAH units**, not drops:
    /// the host builds it with the drop count as the mantissa and an
    /// exponent of −6, then normalizes. Recover the drops with
    /// `float_int(xfl, 6, false)`.
    ///
    /// # A note on MPT amounts
    ///
    /// MPT is out of scope for this layer (see [`AmountBytes`]). This method
    /// does not screen for it: whatever the host does when handed a
    /// hypothetical future MPT amount — including trapping — is the host's
    /// behavior, inherited unchanged. Adding a guard would tax every
    /// ordinary read for a case that cannot arise on Xahau today.
    #[inline(always)]
    pub fn as_xfl(self) -> Result<XFL> {
        api::float::slot_float(self.no)
    }

    /// [`as_xfl`](Self::as_xfl), then clears the slot — on success and on
    /// failure. See [`take_raw_exact`](SlotObject::take_raw_exact).
    #[inline(always)]
    pub fn take_xfl(self) -> Result<XFL> {
        let no = self.no;
        let out = api::float::slot_float(no);
        let _ = api::slot::slot_clear(no);
        out
    }

    /// Reads the amount's raw bytes, classified by length. Consumes.
    #[inline(always)]
    pub fn value(self) -> Result<AmountBytes> {
        decode_amount(self.no)
    }

    /// [`value`](Self::value), then clears the slot — on success and on
    /// failure.
    #[inline(always)]
    pub fn take_value(self) -> Result<AmountBytes> {
        let no = self.no;
        let out = decode_amount(no);
        let _ = api::slot::slot_clear(no);
        out
    }
}

impl SlotObject<Issue> {
    /// Reads the issue, classified by length. Consumes.
    #[inline(always)]
    pub fn value(self) -> Result<IssueData> {
        decode_issue(self.no)
    }

    /// [`value`](Self::value), then clears the slot — on success and on
    /// failure.
    #[inline(always)]
    pub fn take_value(self) -> Result<IssueData> {
        let no = self.no;
        let out = decode_issue(no);
        let _ = api::slot::slot_clear(no);
        out
    }
}

/// Reads and classifies an amount slot.
#[inline(always)]
fn decode_amount(no: u32) -> Result<AmountBytes> {
    let mut buf = [0u8; crate::types::IOU_AMOUNT_LEN];
    let written = api::slot::slot(&mut buf, no)?;
    let bytes = buf.get(..written).ok_or(HookError::TooBig)?;
    classify_amount(bytes)
}

/// Classifies serialized amount bytes by their length: 8 → native, 48 → IOU,
/// anything else → [`HookError::ParseError`].
///
/// Split out of [`decode_amount`] as a pure function so the size contract is
/// testable without a live host — the stubs cannot populate a slot, so this
/// is the only way to check the classification directly.
///
/// MPT amounts (33 bytes) are **out of scope** for this layer — they need an
/// amendment Xahau does not have. An unexpected length is an error here, never
/// a guess: misreading a 33-byte MPT amount as something else would be worse
/// than refusing it.
#[inline(always)]
pub(crate) fn classify_amount(bytes: &[u8]) -> Result<AmountBytes> {
    match bytes.len() {
        crate::types::NATIVE_AMOUNT_LEN => {
            let mut out = [0u8; crate::types::NATIVE_AMOUNT_LEN];
            out.copy_from_slice(bytes);
            Ok(AmountBytes::Native(NativeAmount(out)))
        }
        crate::types::IOU_AMOUNT_LEN => {
            let mut out = [0u8; crate::types::IOU_AMOUNT_LEN];
            out.copy_from_slice(bytes);
            Ok(AmountBytes::Iou(IouAmount(out)))
        }
        _ => Err(HookError::ParseError),
    }
}

/// Reads and classifies an issue slot.
///
/// The buffer is **44** bytes, not the 40 an IOU issue needs, so that a
/// 44-byte MPT issue is *read* rather than rejected by the host call: with a
/// 40-byte buffer the host reports `TooSmall` and
/// [`HookError::ParseError`] — the documented answer for an out-of-scope
/// encoding — would be unreachable on the real path. 44 is the largest
/// encoding xahaud's shared code defines for this field, and still well
/// under the 64-byte zero-init threshold.
#[inline(always)]
fn decode_issue(no: u32) -> Result<IssueData> {
    let mut buf = [0u8; ISSUE_MAX_READ_LEN];
    let written = api::slot::slot(&mut buf, no)?;
    let bytes = buf.get(..written).ok_or(HookError::TooBig)?;
    classify_issue(bytes)
}

/// The widest issue encoding this layer will read before classifying it:
/// 44 bytes, the MPT issue length. Out of scope as a *value* (see
/// [`IssueData`]) but deliberately readable, so it is reported as a parse
/// error rather than as a buffer-too-small error from the host.
pub(crate) const ISSUE_MAX_READ_LEN: usize = 44;

/// Classifies serialized issue bytes by their length: 20 → native, 40 → IOU
/// (currency then issuer), anything else → [`HookError::ParseError`].
///
/// Pure, for the same reason [`classify_amount`] is. MPT issues (44 bytes)
/// are out of scope and reported as an error rather than guessed at — and
/// [`decode_issue`]'s buffer is sized so that a 44-byte issue actually
/// reaches this function instead of failing in the host call first.
#[inline(always)]
pub(crate) fn classify_issue(bytes: &[u8]) -> Result<IssueData> {
    const IOU_LEN: usize = crate::types::CURRENCY_CODE_LEN + crate::types::ACC_ID_LEN;
    match bytes.len() {
        crate::types::CURRENCY_CODE_LEN => Ok(IssueData::Native),
        IOU_LEN => {
            let mut currency = [0u8; crate::types::CURRENCY_CODE_LEN];
            let mut issuer = [0u8; crate::types::ACC_ID_LEN];
            let c_src = bytes
                .get(..crate::types::CURRENCY_CODE_LEN)
                .ok_or(HookError::TooSmall)?;
            let i_src = bytes
                .get(crate::types::CURRENCY_CODE_LEN..IOU_LEN)
                .ok_or(HookError::TooSmall)?;
            currency.copy_from_slice(c_src);
            issuer.copy_from_slice(i_src);
            Ok(IssueData::Iou(IssuedAsset {
                currency: CurrencyCode(currency),
                issuer: AccountId(issuer),
            }))
        }
        _ => Err(HookError::ParseError),
    }
}

// ---------------------------------------------------------------------------
// Scalar reads
// ---------------------------------------------------------------------------

/// Generates the consuming `value`/`take_value` pair for a scalar type read
/// through the host's as-int64 mode (`slot(0, 0, no)`), plus a width check.
///
/// as-int64 is the *only* integer path this layer uses: the host decodes the
/// slot's big-endian wire bytes itself. `FromBytes`/`FixedRead` are
/// little-endian guest conventions and would silently byte-swap.
macro_rules! int_value {
    ($ty:ty, $what:literal) => {
        impl SlotObject<$ty> {
            #[doc = concat!("Reads the slot as a ", $what, ", consuming the handle.")]
            ///
            /// Uses the host's as-int64 decode mode, then range-checks the
            /// result. The host rejects values with bit 63 set, which is
            /// unreachable at this width.
            #[inline(always)]
            pub fn value(self) -> Result<$ty> {
                read_int::<$ty>(self.no)
            }

            #[doc = concat!("Reads the slot as a ", $what, ", then clears it — on success and on failure.")]
            #[inline(always)]
            pub fn take_value(self) -> Result<$ty> {
                let no = self.no;
                let out = read_int::<$ty>(no);
                let _ = api::slot::slot_clear(no);
                out
            }
        }
    };
}

/// Narrows the host's `u64` as-int64 result to `T`, erroring if it does not
/// fit.
#[inline(always)]
fn read_int<T: TryFrom<u64>>(no: u32) -> Result<T> {
    let raw = api::slot::slot_u64(no)?;
    T::try_from(raw).map_err(|_| HookError::TooBig)
}

int_value!(u8, "`u8`");
int_value!(u16, "`u16`");
int_value!(u32, "`u32`");

impl SlotObject<u64> {
    /// Reads the slot as a `u64`, consuming the handle.
    ///
    /// Deliberately **not** the as-int64 path the narrower integers use: the
    /// host rejects a value with bit 63 set as `TOO_BIG`, and legitimate
    /// 64-bit fields set it (`sfExchangeRate` among them). Reading the eight
    /// wire bytes and decoding them big-endian here has no such hole.
    #[inline(always)]
    pub fn value(self) -> Result<u64> {
        let no = self.no;
        read_u64(no)
    }

    /// [`value`](Self::value), then clears the slot — on success and on
    /// failure.
    #[inline(always)]
    pub fn take_value(self) -> Result<u64> {
        let no = self.no;
        let out = read_u64(no);
        let _ = api::slot::slot_clear(no);
        out
    }
}

/// Reads a `u64` from its eight big-endian wire bytes.
#[inline(always)]
fn read_u64(no: u32) -> Result<u64> {
    read_exact_bytes::<8>(no).map(u64::from_be_bytes)
}

/// Generates the consuming `value`/`take_value` pair for a fixed-size byte
/// newtype read straight out of the slot's wire bytes.
macro_rules! bytes_value {
    ($ty:ty, $len:expr, $what:literal) => {
        impl SlotObject<$ty> {
            #[doc = concat!("Reads the slot as ", $what, ", consuming the handle.")]
            ///
            /// The bytes are taken verbatim, in wire order — no
            /// interpretation, no byte-swapping.
            #[inline(always)]
            pub fn value(self) -> Result<$ty> {
                read_exact_bytes::<$len>(self.no).map(<$ty>::from)
            }

            #[doc = concat!("Reads the slot as ", $what, ", then clears it — on success and on failure.")]
            #[inline(always)]
            pub fn take_value(self) -> Result<$ty> {
                let no = self.no;
                let out = read_exact_bytes::<$len>(no).map(<$ty>::from);
                let _ = api::slot::slot_clear(no);
                out
            }
        }
    };
}

bytes_value!(Hash, { crate::types::HASH_LEN }, "a [`Hash`]");
bytes_value!(AccountId, { crate::types::ACC_ID_LEN }, "an [`AccountId`]");
bytes_value!(
    CurrencyCode,
    { crate::types::CURRENCY_CODE_LEN },
    "a [`CurrencyCode`]"
);

/// Silences an unused-import warning in builds where no `value()` body
/// happens to name `FixedRead`; the trait is part of this module's
/// documented relationship to `convert` and stays referenced here.
const _: () = {
    #[allow(dead_code)]
    fn _assert_fixed_read_is_not_used_on_slots<T: FixedRead>() {}
};

/// Navigates a chain of slot hops, clearing every intermediate.
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::slot_path;
///
/// # fn f(accid: &AccountId) -> Result<()> {
/// let signers = SlotObject::from_keylet(&keylet_signers(accid)?)?;
/// let first: AccountId = slot_path!(signers[sfSignerEntries][0u32][sfAccount])?.value()?;
/// # let _ = first;
/// # Ok(())
/// # }
/// ```
///
/// # What it does that a chain of `?` cannot
///
/// `root.get(a)?.get(b)?.get(c)?` leaks the two intermediate slots: each
/// temporary handle is dropped without clearing, and (deliberately) nothing
/// clears on drop. This macro clears each intermediate as soon as its child
/// exists, so a 10-hop path costs 1 live slot, not 10.
///
/// The order per hop is `let next = cur.get(k); let _ = cur.clear(); match
/// next {..}` — the current handle is cleared **unconditionally**, before the
/// result is inspected, so a hop that fails cannot leak the parent that
/// produced it. Clearing a parent after deriving a child is sound because
/// the host copies the parent's storage into the child slot; that is pinned
/// by a live e2e test, not assumed.
///
/// The root is *borrowed* and never cleared — it is the caller's handle, and
/// the caller may well want more children from it. It is evaluated exactly
/// once.
///
/// # Spelling the root
///
/// The root is one token tree: a binding (`signers[..]`) or a parenthesized
/// expression (`(load_signers()?)[..]`). Rust's macro grammar forbids an
/// `expr` fragment before `[`, so a bare unparenthesized expression is not
/// expressible here by anyone — parenthesize it, or bind it to a `let`
/// first.
///
/// # Path length
///
/// The expansion nests one `match` per hop, but `rshooks-build`'s unnest pass
/// flattens them: measured block nesting after that pass is **1** at 1, 3
/// *and* 10 hops, with worst-case instructions growing linearly (46 / 94 /
/// 255). Depth is not the constraint it looked like on paper.
///
/// What *does* accumulate is the surrounding code: several multi-hop walks
/// inlined into one function nest their own `if let`/`match` ladders, and
/// that is what reaches the guard checker's 32-level limit. `examples/15_slot-objects`
/// hit 53 that way and came back to 4 by putting each walk in its own
/// `#[inline(never)]` function — the same escape hatch `examples/81_govern`
/// uses.
#[macro_export]
macro_rules! slot_path {
    // Entry: bind the root once (by reference — never cleared), then recurse.
    //
    // `tt`, not `expr`: Rust does not allow an `expr` fragment to be followed
    // by `[`, so `$root:expr [..]` cannot be written by anyone. A single
    // token tree covers the two shapes that matter — a plain binding
    // (`signers[..]`) and a parenthesized expression (`(make_root()?)[..]`) —
    // and binding it to `__root` first is what makes "evaluated exactly once"
    // true for the second.
    ($root:tt $([$key:expr])+) => {{
        let __root = &$root;
        $crate::slot_path!(@hop __root $([$key])+)
    }};

    // Last hop: hand back the child directly.
    (@hop $cur:ident [$key:expr]) => {
        $cur.get($key)
    };

    // Intermediate hop: derive the child, clear the current handle
    // unconditionally, then continue only if the child arrived.
    (@hop $cur:ident [$key:expr] $([$rest:expr])+) => {
        match $cur.get($key) {
            ::core::result::Result::Ok(__next) => {
                let __r = $crate::slot_path!(@owned __next $([$rest])+);
                __r
            }
            ::core::result::Result::Err(__e) => ::core::result::Result::Err(__e),
        }
    };

    // An owned intermediate: derive, clear this one, then match.
    (@owned $cur:ident [$key:expr]) => {{
        let __next = $cur.get($key);
        let _ = $cur.clear();
        __next
    }};

    (@owned $cur:ident [$key:expr] $([$rest:expr])+) => {{
        let __next = $cur.get($key);
        let _ = $cur.clear();
        match __next {
            ::core::result::Result::Ok(__child) => $crate::slot_path!(@owned __child $([$rest])+),
            ::core::result::Result::Err(__e) => ::core::result::Result::Err(__e),
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    // The classification contract, checked directly. Host stubs cannot
    // populate a slot, so the pure functions are the only place the
    // size rules are observable — which is why they are pure functions.

    #[test]
    fn amount_sizes_classify_or_error() {
        let buf = [7u8; crate::types::IOU_AMOUNT_LEN];
        assert_eq!(
            classify_amount(&buf[..crate::types::NATIVE_AMOUNT_LEN]),
            Ok(AmountBytes::Native(NativeAmount([7u8; 8])))
        );
        assert_eq!(classify_amount(&buf), Ok(AmountBytes::Iou(IouAmount(buf))));

        // Every other length is an error, never a misclassification. 33 is
        // the MPT amount length, deliberately out of scope.
        assert_eq!(classify_amount(&buf[..33]), Err(HookError::ParseError));
        assert_eq!(classify_amount(&buf[..0]), Err(HookError::ParseError));
        assert_eq!(classify_amount(&buf[..20]), Err(HookError::ParseError));
    }

    #[test]
    fn issue_sizes_classify_or_error() {
        let mut buf = [0u8; ISSUE_MAX_READ_LEN];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = i as u8;
        }
        const CUR: usize = crate::types::CURRENCY_CODE_LEN;
        const IOU: usize = CUR + crate::types::ACC_ID_LEN;
        // `.get(..)` rather than `&buf[..n]`: this workspace denies
        // `clippy::indexing_slicing`, tests included.
        let at = |n: usize| buf.get(..n).unwrap_or(&[]);

        assert_eq!(classify_issue(at(CUR)), Ok(IssueData::Native));

        let mut currency = [0u8; CUR];
        let mut issuer = [0u8; crate::types::ACC_ID_LEN];
        currency.copy_from_slice(at(CUR));
        issuer.copy_from_slice(buf.get(CUR..IOU).unwrap_or(&[]));
        assert_eq!(
            classify_issue(at(IOU)),
            Ok(IssueData::Iou(IssuedAsset {
                currency: CurrencyCode(currency),
                issuer: AccountId(issuer),
            }))
        );

        // 44 is the MPT issue length. `decode_issue`'s buffer is sized to
        // read it, so this error is reachable through
        // `SlotObject<Issue>::value()` rather than being pre-empted by a
        // `TooSmall` from the host call — which is the whole point of that
        // buffer being 44 and not 40.
        assert_eq!(classify_issue(&buf), Err(HookError::ParseError));
        assert_eq!(classify_issue(at(0)), Err(HookError::ParseError));
    }

    #[test]
    fn issue_read_buffer_covers_the_out_of_scope_encoding() {
        // A compile-time check, not a runtime one: `assert!` on a constant
        // comparison is optimized out (and clippy says so). The buffer must
        // hold a 44-byte MPT issue, or the `ParseError` contract above is
        // unreachable on the real read path; and it must stay under the
        // 64-byte zero-init threshold.
        const _: () = assert!(ISSUE_MAX_READ_LEN >= 44);
        const _: () = assert!(ISSUE_MAX_READ_LEN <= 48);
    }
}
