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
//! read, a retype), and it cannot be duplicated. A `Copy` handle plus a
//! consuming `clear` would let a stale copy read or clear a slot the host
//! has since reassigned to something else, silently operating on an
//! unrelated object.
//!
//! There is deliberately no `Drop`: cleanup is fallible (a host call that
//! can report `DOESNT_EXIST`) and would cost instructions on every exit
//! path, including ones that never touched a slot. The budget is
//! per-execution — 255 slots, all released by the host when the hook
//! returns — so a leaked slot costs nothing but itself, and affinity keeps
//! a leaked handle from aliasing anything.
//!
//! # Terminal reads consume, and do not clear
//!
//! [`value`](SlotObject::value), [`as_xfl`](SlotObject::as_xfl),
//! [`raw`](SlotObject::raw) and [`raw_exact`](SlotObject::raw_exact) take
//! `self`: the handle is spent, but the slot keeps its contents until the
//! hook ends — the same cost model as C, where a `slot_subfield` followed by
//! a `slot()` read leaks the slot identically and an implicit clear would
//! tax every read with a host call the idiom does not pay.
//!
//! When a loop would otherwise burn through the 255-slot budget, the
//! opt-in [`take_value`](SlotObject::take_value) /
//! [`take_xfl`](SlotObject::take_xfl) /
//! [`take_raw_exact`](SlotObject::take_raw_exact) /
//! [`take_raw`](SlotObject::take_raw) family reads *and* clears, on both the
//! success and failure path. Budget math: one slot per `get`, 255 per
//! execution; a 300-iteration loop deriving one child per iteration must use
//! `take_*` (or clear explicitly) or it will run out.
//!
//! # Do not mix this with the numbered slot functions
//!
//! [`crate::api::slot`]'s numbered functions (`slot_set`, `slot_clear`,
//! `slot_subfield`, ...) and this module address the same 255 registers.
//! Calling `slot_clear(3)` while a `SlotObject` holds slot 3 corrupts that
//! handle's meaning — it stays valid-looking and starts describing whatever
//! lands there next. This is a logic hazard, not a memory-safety one (no
//! `unsafe` on either side): pick one layer per hook. The numbered functions
//! are deliberately out of the prelude, reachable only via explicit paths
//! (`rshooks::api::slot::slot_clear`, `rshooks::api::otxn::otxn_slot`), so
//! mixing them stays visible at the call site.
//!
//! # Stack buffers, never zero-initialized
//!
//! Every fixed-size read here — the `Amount`/`Issue` decoders (at most 48
//! bytes, the widest thing this layer decodes: an IOU amount; the issue path
//! reads 44), [`raw_exact::<N>`](SlotObject::raw_exact),
//! [`take_raw_exact::<N>`](SlotObject::take_raw_exact), and the built-in
//! `u64`/`Hash`/`AccountId`/`CurrencyCode` reads — reads into an
//! uninitialized scratch buffer: the host always overwrites what it's
//! handed, and the result is accepted only when it reports writing the
//! buffer's *entire* length (the `Amount`/`Issue` decoders instead accept
//! only the reported prefix, since a native/IOU/MPT-shaped value's length
//! varies), so nothing here is ever read uninitialized. No zero-init, no
//! `memset` risk at any `N` or optimization level. [`raw`](SlotObject::raw)
//! and [`take_raw`](SlotObject::take_raw) write into a caller-supplied
//! buffer, so any zero-init cost there is the caller's own to manage.

use core::marker::PhantomData;
use core::mem::MaybeUninit;

use crate::api;
use crate::convert::FixedRead;
use crate::error::{HookError, Result, res};
use crate::types::{
    AccountId, Amount, CurrencyCode, Hash, IouAmount, Issue, IssuedAsset, Keylet, NativeAmount,
    Opaque, SField, STArray, STObject,
};
use crate::xfl::XFL;

/// Sealing module: the traits below carry a supertrait that only this crate
/// can name, so downstream crates cannot add implementations.
///
/// [`Resolve::resolve`] turns a parent slot number into a child slot
/// number — a downstream implementation could hand back an arbitrary number
/// and forge a [`SlotObject`] aliasing a slot something else already owns,
/// defeating the affinity that makes the handles safe. Likewise
/// [`CastTargetSealed`] decides which serialized type IDs a retype accepts;
/// a downstream impl could accept everything.
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
    /// The type parameter is [`Opaque`] because the field code alone does
    /// not determine the value type — it is erased, good for comparison
    /// (`SField` equality is on the code alone, so it matches a constant of
    /// any `T`) and for `.code()` extraction, not for deciding how to read
    /// the slot. Use [`try_cast`](Self::try_cast) or
    /// [`assume_type`](Self::assume_type) for that.
    ///
    /// A slot derived by navigation reports the field it came from. A
    /// **root** slot (`from_otxn`, `from_meta`, `from_keylet`,
    /// `from_txn_hash`) instead reports a high-level object code, because
    /// there is no enclosing object it is a field of: its serialized type ID
    /// (`code >> 16`) lands in the 10001–10004 range (`sfTransaction`,
    /// `sfLedgerEntry`, ...) rather than the ordinary 1–26 type IDs.
    /// [`try_cast::<STObject>`](Self::try_cast) accepts those codes for
    /// exactly that reason, and a root slot's code compares *unequal* to
    /// every constant in [`crate::sfield`].
    #[inline(always)]
    pub fn field_code(&self) -> Result<SField<Opaque>> {
        api::slot::slot_type(self.no, 0).map(SField::new)
    }

    /// Navigates to a child slot, auto-assigning its number.
    ///
    /// Borrows — one parent can yield several children, and reading two
    /// fields of the same object should not require re-loading it. The
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

    /// [`get`](Self::get) for an optional field. Absence is checked on the
    /// raw host code to avoid the nesting cost of decoding [`HookError`].
    #[inline(always)]
    pub(crate) fn get_opt<U>(&self, field: SField<U>) -> Result<Option<SlotObject<U>>>
    where
        SField<U>: SlotKey<T>,
    {
        let code = api::slot::slot_subfield_raw_code(self.no, field.code(), 0);
        if code == rshooks_core::DOESNT_EXIST {
            return Ok(None);
        }
        res(code).map(|no| Some(SlotObject::wrap(no as u32)))
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

    /// [`raw`](Self::raw), then best-effort clears the slot on every path.
    /// The read result takes precedence over a clear failure.
    #[inline(always)]
    pub fn take_raw<B: AsMut<[u8]> + ?Sized>(self, buf: &mut B) -> Result<usize> {
        let no = self.no;
        let out = api::slot::slot(buf, no);
        let _ = api::slot::slot_clear(no);
        out
    }

    /// Reads exactly `N` bytes, erroring if the slot is not exactly that
    /// size. Consumes the handle.
    ///
    /// `N` is yours to choose: this reads into uninitialized scratch, so
    /// there is no zero-init cost and no `memset` risk at any size (see this
    /// module's "Stack buffers" section).
    #[inline(always)]
    pub fn raw_exact<const N: usize>(self) -> Result<[u8; N]> {
        let no = self.no;
        read_exact_bytes::<N>(no)
    }

    /// [`raw_exact`](Self::raw_exact), then clears the slot — on the success
    /// path *and* the failure path. The clear's own result is discarded: the
    /// read's result is the one the caller asked for, and a failed clear
    /// cannot be acted on usefully here.
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

/// Reads exactly `N` bytes without zero-initializing the host-owned output
/// buffer. The result is exposed only after the host reports writing all
/// `N` bytes.
#[inline(always)]
fn read_exact_bytes<const N: usize>(no: u32) -> Result<[u8; N]> {
    let mut buf = [const { MaybeUninit::<u8>::uninit() }; N];
    let written = api::slot::slot_uninit(&mut buf, no)?;
    if written == N {
        // SAFETY: `written == N` proves every byte is initialized;
        // `MaybeUninit<u8>` has the same layout as `u8`.
        Ok(unsafe { core::mem::transmute_copy::<[MaybeUninit<u8>; N], [u8; N]>(&buf) })
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
    /// hypothetical future MPT amount — including trapping — is inherited
    /// unchanged, since MPT cannot arise on Xahau today.
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
    let mut storage = MaybeUninit::<[u8; crate::types::IOU_AMOUNT_LEN]>::uninit();
    // SAFETY: only the `..written` prefix `api::slot::slot` reports writing
    // is ever read below.
    let buf = unsafe { crate::convert::uninit_slice_mut(&mut storage) };
    let written = api::slot::slot(buf, no)?;
    let bytes = buf.get(..written).ok_or(HookError::TooBig)?;
    classify_amount(bytes)
}

/// Classifies serialized amount bytes by their length: 8 → native, 48 → IOU,
/// anything else → [`HookError::ParseError`].
///
/// Split out of [`decode_amount`] as a pure function so the size contract is
/// testable without a live host (stubs cannot populate a slot).
///
/// MPT amounts (33 bytes, out of scope — they need an amendment Xahau does
/// not have) are never guessed at; an unexpected length is always an error.
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
/// The buffer is **44** bytes, not the 40 an IOU issue needs, so a 44-byte
/// MPT issue is *read* rather than rejected by the host call — with a
/// 40-byte buffer the host would report `TooSmall` before
/// [`HookError::ParseError`] (the documented answer for an out-of-scope
/// encoding) is reachable. 44 is the largest encoding xahaud's shared code
/// defines for this field.
#[inline(always)]
fn decode_issue(no: u32) -> Result<IssueData> {
    let mut storage = MaybeUninit::<[u8; ISSUE_MAX_READ_LEN]>::uninit();
    // SAFETY: only the `..written` prefix `api::slot::slot` reports writing
    // is ever read below.
    let buf = unsafe { crate::convert::uninit_slice_mut(&mut storage) };
    let written = api::slot::slot(buf, no)?;
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
/// happens to name `FixedRead`.
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
/// `root.get(a)?.get(b)?.get(c)?` leaks the two intermediate slots — nothing
/// clears on drop. This macro clears each intermediate as soon as its child
/// exists, so a 10-hop path costs 1 live slot, not 10.
///
/// Per hop: `let next = cur.get(k); let _ = cur.clear(); match next {..}` —
/// the current handle is cleared **unconditionally**, before the result is
/// inspected, so a failed hop cannot leak its parent. Clearing a parent
/// after deriving a child is sound because the host copies the parent's
/// storage into the child slot (pinned by a live e2e test, not assumed).
///
/// The root is *borrowed*, never cleared, and evaluated exactly once — it
/// is the caller's handle, which may still be wanted for more children.
///
/// # Spelling the root
///
/// The root is one token tree: a binding (`signers[..]`) or a parenthesized
/// expression (`(load_signers()?)[..]`). Rust's macro grammar forbids an
/// `expr` fragment before `[`, so a bare unparenthesized expression cannot
/// be written here — parenthesize it, or bind it to a `let` first.
///
/// # Path length
///
/// The expansion nests one `match` per hop, but `rshooks-build`'s unnest
/// pass flattens them: measured block nesting after that pass is **1** at
/// 1, 3, and 10 hops, with worst-case instructions growing linearly
/// (46 / 94 / 255).
///
/// What *does* accumulate is the surrounding code: several multi-hop walks
/// inlined into one function nest their own `if let`/`match` ladders, which
/// is what reaches the guard checker's 32-level limit.
/// `examples/15_slot-objects` hit 53 that way and came back to 4 by putting
/// each walk in its own `#[inline(never)]` function — the same escape hatch
/// `examples/80_governance`'s `govern` entry uses.
#[macro_export]
macro_rules! slot_path {
    // Entry: bind the root once (by reference — never cleared), then
    // recurse. `tt`, not `expr` — see "Spelling the root" above.
    ($root:tt $([$key:expr])+) => {{
        let __root = &$root;
        $crate::slot_path!(@hop __root $([$key])+)
    }};

    // Last hop: hand back the child directly.
    (@hop $cur:ident [$key:expr]) => {
        $cur.get($key)
    };

    // Intermediate hop: derive, clear the current handle unconditionally,
    // then continue only if the child arrived.
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

    // The classification contract, checked directly (host stubs cannot
    // populate a slot).

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

        // 44 is the MPT issue length; `decode_issue`'s buffer is sized to
        // reach this classification instead of failing as `TooSmall` in the
        // host call.
        assert_eq!(classify_issue(&buf), Err(HookError::ParseError));
        assert_eq!(classify_issue(at(0)), Err(HookError::ParseError));
    }

    #[test]
    fn issue_read_buffer_covers_the_out_of_scope_encoding() {
        // A compile-time check, not a runtime one: `assert!` on a constant
        // comparison is optimized out. The buffer must hold a 44-byte MPT
        // issue, or the `ParseError` contract above is unreachable on the
        // real read path, and it must stay under the 64-byte zero-init
        // threshold.
        const _: () = assert!(ISSUE_MAX_READ_LEN >= 44);
        const _: () = assert!(ISSUE_MAX_READ_LEN <= 48);
    }
}
