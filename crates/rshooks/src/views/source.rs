//! The hand-written runtime the generated views in [`super`] are built on:
//! where a view's fields come from, how an absent field becomes `Ok(None)`,
//! and the slot-lifetime policy that keeps a view from leaking slots.
//!
//! Nothing here is generated. The generated files hold one struct and one
//! accessor per upstream declaration and no logic at all; every decision a
//! view makes lives in this module, so it is reviewable once rather than
//! 2000 times.
//!
//! # Two sources, one accessor
//!
//! A transaction view is generic over [`FieldSource`], which has exactly two
//! implementations:
//!
//! - [`OtxnSource`] — a ZST. Reads go straight to the originating
//!   transaction (`otxn_field`), one host call per access, with the field
//!   lookup paid by the host. This is the cheapest way to read the
//!   originating transaction.
//! - [`SlotSource`] — wraps an already-loaded `SlotObject<STObject>`. Reads
//!   navigate to a child slot and read it. This is the only source that can
//!   reach *into* a container, and the only one available for a ledger
//!   object.
//!
//! Both are monomorphized and every accessor is `#[inline(always)]`, so the
//! abstraction compiles away: a view accessor is the host call it wraps.
//!
//! # Absence is decided on the raw return code
//!
//! An optional field reads back as `Ok(None)` when it is missing. That
//! decision is made by comparing the **undecoded** `i64` the host returned
//! against `rshooks_core::DOESNT_EXIST`, never by matching
//! `HookError::DoesntExist`.
//!
//! This is not a micro-optimization, it is a hard requirement:
//! `docs/DESIGN.md` §5.6 explains that `HookError::from` compiles to a
//! ~40-block `br_table` which the optimizer only keeps at call sites that
//! inspect *which* variant a failure was, and that `rshooks-build` inlines
//! every function into `hook()`/`cbak()` and must then keep total block
//! nesting under the guard checker's 32-level limit. A view emits an
//! optional read per optional field, so a single `Err(HookError::DoesntExist)`
//! match in this module would be inlined into `hook()` once per accessor
//! call and blow the budget on its own. The raw-code helpers
//! ([`crate::api::otxn`]'s `otxn_field_raw_code`, [`crate::api::slot`]'s
//! `slot_subfield_raw_code`) exist for exactly this.
//!
//! The required-field accessors are the optional ones plus
//! `.ok_or(HookError::DoesntExist)`: *constructing* a variant is free, only
//! *inspecting* one is not.
//!
//! # Slot lifetime: get, read, clear
//!
//! Every [`SlotSource`] read is get → read → **clear**. It navigates to a
//! child slot, performs a terminal read, and releases the child before
//! returning — through the `take_*` read-and-clear family
//! ([`SlotObject::take_raw`] is the variable-length member, added for this).
//!
//! The consequence is the point: a view's accessors can be called any number
//! of times and consume **zero** slots beyond the view's own root. Without
//! it, the 255-slot budget would fall to one slot per accessor call, and a
//! view over a ledger object with thirty fields would be unusable.
//!
//! The generated `*_slot` subobject accessors are the one deliberate
//! exception: they hand the child slot's ownership to the caller, who then
//! owns its lifetime the way they would after any `SlotObject::get`. Their
//! doc comments say so.
//!
//! ## What that costs, stated plainly
//!
//! This is the **only** place a view spends an instruction a hand-written
//! hook need not. The C idiom — and [`SlotObject`]'s own consuming reads,
//! whose module docs explain why — does `slot_subfield` then a read and
//! leaves the slot allocated, because the budget is per-execution and the
//! host frees everything when the hook returns. A view adds one
//! `slot_clear` host call per slot-backed read on top of that.
//!
//! It is not a tax a view can opt out of. A generated accessor cannot know
//! how many times a hook will call it, and an accessor that leaks one slot
//! per call is one a hook cannot use in a loop at all. Where that trade is
//! not the one you want, the raw layers are unchanged and public:
//! [`crate::slot_obj`]'s non-clearing reads and
//! [`crate::api::slot`]'s numbered functions cost exactly what they always
//! did.
//!
//! Nothing else here costs anything. An [`OtxnSource`] accessor is one
//! `otxn_field` call and no bookkeeping — the same call a hand-written hook
//! would make, with the field code supplied by a generated constant instead
//! of typed out. Both sources' reads write straight into the caller's
//! storage; nothing is copied twice.

use crate::api::otxn;
use crate::error::{HookError, Result, res};
use crate::slot_obj::{AmountBytes, IssueData, SlotObject};
use crate::types::{
    ACC_ID_LEN, AccountId, Amount, CURRENCY_CODE_LEN, CurrencyCode, HASH_LEN, Hash, IOU_AMOUNT_LEN,
    Issue, SField, STObject,
};

/// Sealing module: [`FieldSource`] and [`ViewValue`] are closed sets.
///
/// [`FieldSource`] is sealed because a downstream implementation would
/// decide what a *generated* accessor does — including whether it clears the
/// slots it opens — which is this module's invariant to keep, not a
/// caller's. [`ViewValue`] is sealed for the reason
/// [`crate::api::otxn::OtxnFieldValue`] is: it pairs a wire type with a Rust
/// type, and only the generated [`crate::sfield`] table is supposed to make
/// that pairing.
mod private {
    /// Supertrait of [`super::FieldSource`].
    pub trait SealedSource {}
    /// Supertrait of [`super::ViewValue`].
    pub trait SealedValue {}
}

/// A value type a generated view accessor can return, and how to read it
/// from each source.
///
/// The set is exactly the types [`crate::sfield`] gives a field constant a
/// value type for, minus the containers: `u8`/`u16`/`u32`/`u64`, [`Hash`],
/// [`AccountId`], [`CurrencyCode`], [`Amount`] (read as [`AmountBytes`]) and
/// [`Issue`] (read as [`IssueData`]). A field whose serialized type this
/// crate models no typed read for gets a raw `*_into` accessor instead, not
/// an entry here.
///
/// **Sealed** — see [`private`].
pub trait ViewValue: private::SealedValue + Sized {
    /// What a read of this field returns. `Self` for every scalar and
    /// fixed-byte type; [`AmountBytes`]/[`IssueData`] for the two whose wire
    /// encoding is one of several shapes rather than one.
    type Out;

    /// Reads this field off the originating transaction, `Ok(None)` if it is
    /// absent. Absence is decided on the raw return code (module docs).
    #[doc(hidden)]
    fn read_otxn_opt(field: SField<Self>) -> Result<Option<Self::Out>>;

    /// Reads this field out of `parent`, `Ok(None)` if it is absent. The
    /// child slot is released before returning, on every path.
    #[doc(hidden)]
    fn read_slot_opt(
        parent: &SlotObject<STObject>,
        field: SField<Self>,
    ) -> Result<Option<Self::Out>>;
}

/// Implements [`ViewValue`] for a type read through the host's as-int64
/// mode on the otxn side and `take_value` on the slot side.
macro_rules! int_view_value {
    ($ty:ty) => {
        impl private::SealedValue for $ty {}
        impl ViewValue for $ty {
            type Out = $ty;

            #[inline(always)]
            fn read_otxn_opt(field: SField<Self>) -> Result<Option<$ty>> {
                let code = otxn::otxn_field_u64_raw_code(field.code());
                if code == rshooks_core::DOESNT_EXIST {
                    return Ok(None);
                }
                let raw = res(code)? as u64;
                <$ty>::try_from(raw)
                    .map(Some)
                    .map_err(|_| HookError::TooBig)
            }

            #[inline(always)]
            fn read_slot_opt(
                parent: &SlotObject<STObject>,
                field: SField<Self>,
            ) -> Result<Option<$ty>> {
                match parent.get_opt(field)? {
                    Some(child) => child.take_value().map(Some),
                    None => Ok(None),
                }
            }
        }
    };
}

int_view_value!(u8);
int_view_value!(u16);
int_view_value!(u32);

/// Implements [`ViewValue`] for a fixed-size wire type read verbatim.
///
/// `u64` uses this too rather than the as-int64 path: the host rejects a
/// value with bit 63 set as `TOO_BIG`, and legitimate 64-bit fields set it
/// (`sfExchangeRate` among them) — the identical rationale
/// `SlotObject::<u64>::value` and `OtxnFieldValue for u64` both record.
///
/// The read goes into **uninitialized** scratch, the same way
/// [`crate::slot_obj`]'s fixed-size reads do and for the same reason: the
/// host call overwrites whatever it is handed, and the result is only
/// accepted when it reports writing the buffer's entire length, so nothing
/// is ever read uninitialized. Zero-initializing first would be dead work
/// the guard checker still charges for — a zeroed buffer whose address
/// escapes into an `extern` call is a store LLVM cannot prove dead across
/// the FFI boundary. [`Amount`] and [`Issue`] below deliberately keep their
/// zero-init: those reads are variable-length and inspect `buf[..written]`,
/// so the full-length proof this relies on is not available to them —
/// exactly the split `slot_obj`'s `decode_amount`/`decode_issue` make.
macro_rules! bytes_view_value {
    ($ty:ty, $len:expr, $decode:expr) => {
        impl private::SealedValue for $ty {}
        impl ViewValue for $ty {
            type Out = $ty;

            #[inline(always)]
            fn read_otxn_opt(field: SField<Self>) -> Result<Option<$ty>> {
                let mut buf = core::mem::MaybeUninit::<[u8; $len]>::uninit();
                // SAFETY: the slice is handed straight to the host call and
                // never read here; `buf` itself is only read through
                // `assume_init` below, and only once `written == $len`
                // confirms the host wrote every byte of it — honoring
                // `uninit_slice_mut`'s contract.
                let out = unsafe { crate::slot_obj::uninit_slice_mut(&mut buf) };
                let code = otxn::otxn_field_raw_code(out, field.code());
                if code == rshooks_core::DOESNT_EXIST {
                    return Ok(None);
                }
                let written = res(code)? as usize;
                if written == $len {
                    // SAFETY: `written == $len` means the host reported
                    // writing all `$len` bytes of `buf`'s storage, so it is
                    // now fully initialized.
                    let bytes = unsafe { buf.assume_init() };
                    #[allow(clippy::redundant_closure_call)]
                    Ok(Some($decode(bytes)))
                } else {
                    Err(HookError::TooSmall)
                }
            }

            #[inline(always)]
            fn read_slot_opt(
                parent: &SlotObject<STObject>,
                field: SField<Self>,
            ) -> Result<Option<$ty>> {
                match parent.get_opt(field)? {
                    Some(child) => child.take_value().map(Some),
                    None => Ok(None),
                }
            }
        }
    };
}

bytes_view_value!(u64, 8, u64::from_be_bytes);
bytes_view_value!(Hash, HASH_LEN, Hash::from);
bytes_view_value!(AccountId, ACC_ID_LEN, AccountId::from);
bytes_view_value!(CurrencyCode, CURRENCY_CODE_LEN, CurrencyCode::from);

impl private::SealedValue for Amount {}
impl ViewValue for Amount {
    type Out = AmountBytes;

    /// Reads into a 48-byte buffer (the widest form, an IOU amount) and
    /// classifies by written length, the same way
    /// `OtxnFieldValue for Amount` does.
    #[inline(always)]
    fn read_otxn_opt(field: SField<Self>) -> Result<Option<AmountBytes>> {
        let mut buf = [0u8; IOU_AMOUNT_LEN];
        let code = otxn::otxn_field_raw_code(&mut buf, field.code());
        if code == rshooks_core::DOESNT_EXIST {
            return Ok(None);
        }
        let written = res(code)? as usize;
        let bytes = buf.get(..written).ok_or(HookError::TooBig)?;
        crate::slot_obj::classify_amount(bytes).map(Some)
    }

    #[inline(always)]
    fn read_slot_opt(
        parent: &SlotObject<STObject>,
        field: SField<Self>,
    ) -> Result<Option<AmountBytes>> {
        match parent.get_opt(field)? {
            Some(child) => child.take_value().map(Some),
            None => Ok(None),
        }
    }
}

impl private::SealedValue for Issue {}
impl ViewValue for Issue {
    type Out = IssueData;

    /// 44 bytes, not the 40 an IOU issue needs — `slot_obj::decode_issue`'s
    /// doc comment records why the wider buffer is what makes a 44-byte MPT
    /// issue surface as a `ParseError` instead of failing the host call as
    /// `TooSmall` first.
    #[inline(always)]
    fn read_otxn_opt(field: SField<Self>) -> Result<Option<IssueData>> {
        let mut buf = [0u8; crate::slot_obj::ISSUE_MAX_READ_LEN];
        let code = otxn::otxn_field_raw_code(&mut buf, field.code());
        if code == rshooks_core::DOESNT_EXIST {
            return Ok(None);
        }
        let written = res(code)? as usize;
        let bytes = buf.get(..written).ok_or(HookError::TooBig)?;
        crate::slot_obj::classify_issue(bytes).map(Some)
    }

    #[inline(always)]
    fn read_slot_opt(
        parent: &SlotObject<STObject>,
        field: SField<Self>,
    ) -> Result<Option<IssueData>> {
        match parent.get_opt(field)? {
            Some(child) => child.take_value().map(Some),
            None => Ok(None),
        }
    }
}

/// Where a generated view's fields come from.
///
/// **Sealed** — see [`private`]. Two implementations, [`OtxnSource`] and
/// [`SlotSource`]; the module docs cover what each costs and why the
/// slot-backed one clears every child slot it opens.
///
/// A third implementation over already-parsed bytes (a buffer walker) would
/// slot in here without touching a line of generated code. That extension
/// point is the reason this trait exists rather than two duplicated view
/// hierarchies.
pub trait FieldSource: private::SealedSource {
    /// Reads a field whose serialized type has a modeled value type,
    /// `Ok(None)` when it is absent.
    fn read_opt<T: ViewValue>(&self, field: SField<T>) -> Result<Option<T::Out>>;

    /// Reads a field's raw wire bytes into `out`, returning the number
    /// written, or `Ok(None)` when the field is absent.
    fn read_raw_opt<B: AsMut<[u8]> + ?Sized>(
        &self,
        field_code: u32,
        out: &mut B,
    ) -> Result<Option<usize>>;

    /// [`read_opt`](Self::read_opt) for a field the format declares
    /// required: an absent field is [`HookError::DoesntExist`].
    ///
    /// Constructing that variant is free; only inspecting one is not (module
    /// docs), so this stays on the cheap side of the nesting budget.
    #[inline(always)]
    fn read<T: ViewValue>(&self, field: SField<T>) -> Result<T::Out> {
        self.read_opt(field)?.ok_or(HookError::DoesntExist)
    }

    /// [`read_raw_opt`](Self::read_raw_opt) for a required field.
    #[inline(always)]
    fn read_raw<B: AsMut<[u8]> + ?Sized>(&self, field_code: u32, out: &mut B) -> Result<usize> {
        self.read_raw_opt(field_code, out)?
            .ok_or(HookError::DoesntExist)
    }
}

/// Reads a view's fields directly off the originating transaction.
///
/// A zero-sized type: a view built on it is a zero-sized struct, and its
/// accessors are one host call each with no bookkeeping. This is the
/// cheapest way to read the originating transaction — the host does the
/// field lookup, and no slot is consumed at all.
///
/// It cannot reach into a container: an `STObject`/`STArray` field is
/// readable only as raw bytes here, because `otxn_field` has no way to
/// navigate. Load the transaction into a slot
/// (`SlotObject::from_otxn`) and use a [`SlotSource`] view when you need to
/// go inside one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OtxnSource;

impl private::SealedSource for OtxnSource {}

impl FieldSource for OtxnSource {
    #[inline(always)]
    fn read_opt<T: ViewValue>(&self, field: SField<T>) -> Result<Option<T::Out>> {
        T::read_otxn_opt(field)
    }

    #[inline(always)]
    fn read_raw_opt<B: AsMut<[u8]> + ?Sized>(
        &self,
        field_code: u32,
        out: &mut B,
    ) -> Result<Option<usize>> {
        let code = otxn::otxn_field_raw_code(out, field_code);
        if code == rshooks_core::DOESNT_EXIST {
            return Ok(None);
        }
        res(code).map(|v| Some(v as usize))
    }
}

/// Reads a view's fields out of an already-loaded slot.
///
/// Owns the root `SlotObject<STObject>` for the view's lifetime — one slot
/// total, however many accessors are called, because every read releases the
/// child slot it opened (module docs). [`into_slot`](Self::into_slot) hands
/// the root back.
///
/// No `Debug`: [`SlotObject`] deliberately has none, since a printable slot
/// handle invites exactly the aliasing confusion its affinity rules exist to
/// prevent.
pub struct SlotSource {
    obj: SlotObject<STObject>,
}

impl SlotSource {
    /// Takes ownership of an already-loaded object slot.
    #[inline(always)]
    #[must_use]
    pub const fn new(obj: SlotObject<STObject>) -> Self {
        Self { obj }
    }

    /// Navigates to an `STObject`/`STArray` field and hands the child slot
    /// to the caller.
    ///
    /// The one deliberate exception to the get→read→clear policy (module
    /// docs): a container has no terminal read, so there is nothing to
    /// clear *after*. The caller owns the child's lifetime from here, the
    /// same as after any [`SlotObject::get`].
    #[inline(always)]
    pub fn subobject<U>(&self, field: SField<U>) -> Result<SlotObject<U>> {
        self.obj.get(field)
    }

    /// [`subobject`](Self::subobject) for a field the format declares
    /// optional: `Ok(None)` when it is absent, decided on the raw return
    /// code (module docs).
    #[inline(always)]
    pub fn subobject_opt<U>(&self, field: SField<U>) -> Result<Option<SlotObject<U>>> {
        self.obj.get_opt(field)
    }

    /// Hands the root slot back, consuming the source. The caller owns the
    /// slot's lifetime again.
    ///
    /// No `#[must_use]` here: [`SlotObject`] carries one already, and a
    /// second would be `clippy::double_must_use`.
    #[inline(always)]
    pub fn into_slot(self) -> SlotObject<STObject> {
        self.obj
    }
}

impl private::SealedSource for SlotSource {}

impl FieldSource for SlotSource {
    #[inline(always)]
    fn read_opt<T: ViewValue>(&self, field: SField<T>) -> Result<Option<T::Out>> {
        T::read_slot_opt(&self.obj, field)
    }

    /// Navigates to the field, reads its bytes and clears the child slot —
    /// the get→read→clear policy, via
    /// [`SlotObject::take_raw`](crate::slot_obj::SlotObject::take_raw).
    #[inline(always)]
    fn read_raw_opt<B: AsMut<[u8]> + ?Sized>(
        &self,
        field_code: u32,
        out: &mut B,
    ) -> Result<Option<usize>> {
        // `SField<Opaque>` navigates without claiming a value type: the raw
        // path never calls `value()`, so nothing depends on the marker.
        match self
            .obj
            .get_opt(SField::<crate::types::Opaque>::new(field_code))?
        {
            Some(child) => child.take_raw(out).map(Some),
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Checked construction
// ---------------------------------------------------------------------------
//
// Both constructors below compare **raw `u16` codes** — a `tt*` constant
// from `rshooks_core::tts`, an `lt*` one from `rshooks_core::lets` — and
// never a [`crate::tx_type::TxType`] or
// [`crate::ledger_entry_type::LedgerEntryType`]. Those enums' `From<u16>`
// impls are ~74- and ~34-arm matches with the same nesting cost as
// `HookError::from` (`docs/DESIGN.md` §5.6), and a view checks its type once
// per construction, so the check has to stay a single integer compare.

/// Verifies the originating transaction's type before a view claims it,
/// for the generated `Xxx::otxn()` constructors.
///
/// One `otxn_type` host call and one integer compare; a mismatch is
/// [`HookError::DoesNotMatch`]. `otxn_type` cannot fail on a live host, and
/// a negative code from anywhere else (the non-wasm stubs, say) simply
/// matches no `tt*` value and surfaces as `DoesNotMatch` too.
#[inline(always)]
pub(crate) fn otxn_of_type(expected: u16) -> Result<OtxnSource> {
    if otxn::otxn_type_code() == expected {
        Ok(OtxnSource)
    } else {
        Err(HookError::DoesNotMatch)
    }
}

/// Verifies a slotted object's own type field before a view claims it, for
/// the generated `Xxx::from_slot()` constructors — `sfTransactionType` for a
/// transaction, `sfLedgerEntryType` for a ledger object, both `UINT16`.
///
/// The read is one get→read→clear on a child slot, like every other
/// slot-backed read here. On **any** failure the root slot is consumed and
/// best-effort cleared, exactly as
/// [`SlotObject::try_cast`](crate::slot_obj::SlotObject::try_cast) does and
/// for the same reason: a caller who was wrong about what is in the slot is
/// done with it, and leaving it allocated would charge them a slot for the
/// mistake.
#[inline(always)]
pub(crate) fn slot_of_type(
    obj: SlotObject<STObject>,
    field: SField<u16>,
    expected: u16,
) -> Result<SlotSource> {
    match slot_type_field(&obj, field) {
        Ok(actual) if actual == expected => Ok(SlotSource::new(obj)),
        Ok(_) => {
            let _ = obj.clear();
            Err(HookError::DoesNotMatch)
        }
        Err(e) => {
            let _ = obj.clear();
            Err(e)
        }
    }
}

/// Reads a slotted object's own type field as a raw `u16`.
#[inline(always)]
fn slot_type_field(obj: &SlotObject<STObject>, field: SField<u16>) -> Result<u16> {
    match obj.get_opt(field)? {
        Some(child) => child.take_value(),
        None => Err(HookError::DoesntExist),
    }
}
