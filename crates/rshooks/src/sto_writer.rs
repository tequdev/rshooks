//! [`StoWriter`]: a bounded, allocation-free STObject/STArray writer for
//! runtime-sized emitted transactions, plus its dynamic counterpart to
//! [`crate::txn_template!`]'s `prepare_for_emit()`/[`crate::txn::Prepared`]
//! lifecycle.
//!
//! [`crate::txn_template!`] bakes field offsets and the total length into a
//! `const fn` at compile time, which only works when a transaction's shape
//! is known ahead of time. A transaction with a runtime-sized nested
//! `STArray`/`STObject` (Remit's `sfAmounts`, one `sfAmountEntry` per
//! destination) cannot be described that way — [`StoWriter`] is the
//! runtime counterpart: a cursor over caller-owned storage that writes
//! field headers, tracks open containers, and checks every write against
//! the buffer's real bounds.
//!
//! # Field order is caller-supplied, not canonical
//!
//! [`StoWriter`] writes fields in exactly the order its methods are
//! called and never reorders or validates that order. xahaud accepts a
//! serialized object's fields in any order (`STObject::set`) and always
//! re-serializes sorted by field code (`STObject::add`), so the on-ledger
//! transaction is canonically ordered regardless of write order. A
//! consequence: when fields are written outside ascending `(type, field)`
//! order, [`StoWriter`]'s buffer bytes are not byte-identical to the
//! on-ledger serialization — this has no effect on validity or
//! `etxn_fee_base` (the serialized size is the same either way; only
//! field position differs).
//!
//! What *is* enforced: every write is checked against the buffer's real
//! bounds and overflow-checked cursor arithmetic;
//! [`StoWriter::begin_object`]/[`StoWriter::begin_array`] and
//! [`StoWriter::end_object`]/[`StoWriter::end_array`] must match (an
//! `STArray`'s direct children may only be opened with
//! [`StoWriter::begin_object`] — a bare scalar or nested array directly
//! inside an open array is rejected); nesting is bounded by
//! [`STO_WRITER_MAX_DEPTH`]; and no write succeeds once
//! [`StoWriter::prepare_for_emit`] has finalized the writer.
//!
//! # Required fields and duplicate rejection
//!
//! [`StoWriter`] detects the same required emit-plumbing fields
//! `txn_template!` does — `sfSequence`, `sfFirstLedgerSequence`,
//! `sfLastLedgerSequence`, `sfFee`, `sfSigningPubKey`, `sfAccount` — by
//! value, as they are written, recording an offset or presence flag for
//! [`StoWriter::prepare_for_emit`] to patch or verify later. Because
//! `FirstLedgerSequence`/`LastLedgerSequence`/`Account`/`Fee` are patched
//! at each field's *recorded* offset, a second write of any of these six
//! fields would leave the first occurrence unpatched or duplicated in the
//! emitted blob — a serialized object cannot repeat a field — so it is
//! rejected with [`HookError::AlreadySet`]. Any other field may be
//! written more than once as far as [`StoWriter`] is concerned; whether a
//! repeated non-plumbing field is valid is between the caller and the
//! host.
//!
//! # `prepare_for_emit` writes `EmitDetails` itself
//!
//! There is no public `emit_details` method: [`StoWriter::prepare_for_emit`]
//! appends the runtime-sized `sfEmitDetails` field itself, at the current
//! cursor, after every container the caller opened has been closed and
//! before computing `etxn_fee_base` (the fee is sized over the blob
//! *including* `EmitDetails`). `buf` must therefore have at least
//! [`EMIT_DETAILS_MAX_LEN`](crate::types::EMIT_DETAILS_MAX_LEN) bytes of
//! headroom beyond everything the caller already wrote, or
//! [`StoWriter::prepare_for_emit`] fails with
//! [`HookError::InvalidArgument`].

use crate::api::{etxn, float, hook_ctx, ledger};
use crate::error::{HookError, Result};
use crate::sfield::{
    sfAccount, sfFee, sfFirstLedgerSequence, sfLastLedgerSequence, sfSequence, sfSigningPubKey,
};
use crate::txn::{Prepared, TemplateBytes, codec};
use crate::types::{
    ACC_ID_LEN, AccountId, Amount, CurrencyCode, EMIT_DETAILS_MAX_LEN, IOU_AMOUNT_LEN, Opaque,
    SField, STArray, STObject,
};
use crate::xfl::XFL;

/// Maximum container nesting depth (top-level counts as depth 0, so this is
/// the number of frames the writer can hold at once — the top-level object
/// plus up to `STO_WRITER_MAX_DEPTH - 1` nested containers). Fixed and small
/// to stay allocation-free; comfortably covers every real transaction shape
/// (Remit's `sfAmounts` array of `sfAmountEntry` objects is 2 deep) while
/// matching the order of magnitude XRPL's own STObject parser bounds
/// nesting to.
pub const STO_WRITER_MAX_DEPTH: usize = 10;

/// The `ObjectEndMarker` field: type `STObject` (14), field `1` — reserved
/// by the protocol solely for this terminator (never assigned to a named
/// field; see `crates/rshooks/src/sfield.rs`'s generated table). Header
/// derives to the single byte `0xE1` via [`codec::field_header`], the same
/// derivation every other field header in this crate uses.
const OBJECT_END_MARKER: SField<Opaque> = SField::new((14u32 << 16) | 1);

/// The `ArrayEndMarker` field: type `STArray` (15), field `1`. See
/// [`OBJECT_END_MARKER`].
const ARRAY_END_MARKER: SField<Opaque> = SField::new((15u32 << 16) | 1);

/// What kind of container a given nesting depth holds. Only the *kind*
/// matters — there is no per-container ordering state (see the module doc
/// comment): an `STArray`'s direct children may only be opened with
/// [`StoWriter::begin_object`], while an `STObject` accepts any field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Object,
    Array,
}

/// A bounded, allocation-free writer for a runtime-sized `STObject`
/// (typically an emitted transaction), backed entirely by caller-owned
/// storage. See the module doc comment for the field-order and
/// `EmitDetails` lifecycle rules.
///
/// Every write is bounds-checked against `buf`'s real length and against
/// checked cursor arithmetic — a write that would not fit, invalid
/// container nesting, a duplicate required-field write, or a write after
/// [`Self::prepare_for_emit`] has succeeded all fail
/// (see each method's `# Errors`) rather than panicking or corrupting
/// already-written bytes.
#[must_use = "a StoWriter that is never read via as_bytes()/prepare_for_emit() means the writes were wasted"]
pub struct StoWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
    frames: [FrameKind; STO_WRITER_MAX_DEPTH],
    depth: usize,
    sequence_seen: bool,
    first_ledger_sequence_off: Option<usize>,
    last_ledger_sequence_off: Option<usize>,
    fee_off: Option<usize>,
    account_off: Option<usize>,
    signing_pub_key_seen: bool,
    finalized: bool,
}

impl<'a> StoWriter<'a> {
    /// Wraps `buf` as a fresh writer, empty (position `0`), at the top-level
    /// container. `buf` must have at least
    /// [`EMIT_DETAILS_MAX_LEN`](crate::types::EMIT_DETAILS_MAX_LEN) bytes of
    /// headroom beyond every field the caller writes, for
    /// [`Self::prepare_for_emit`] to append `EmitDetails` into — see the
    /// module doc comment.
    #[inline(always)]
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            pos: 0,
            frames: [FrameKind::Object; STO_WRITER_MAX_DEPTH],
            depth: 0,
            sequence_seen: false,
            first_ledger_sequence_off: None,
            last_ledger_sequence_off: None,
            fee_off: None,
            account_off: None,
            signing_pub_key_seen: false,
            finalized: false,
        }
    }

    /// The bytes written so far (`0..`[`Self::len`] of the backing buffer).
    /// Available at any point, including mid-construction with open
    /// containers — unlike [`crate::txn::Prepared::as_bytes`], this does
    /// not require the container stack to be closed or any emit-plumbing
    /// field to be present. Never exposes reserved-but-unwritten capacity.
    #[inline(always)]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.pos).unwrap_or_default()
    }

    /// The number of bytes written so far.
    #[inline(always)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.pos
    }

    /// Whether nothing has been written yet.
    #[inline(always)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pos == 0
    }

    // -- Cursor arithmetic --------------------------------------------------

    /// Reserves `n` bytes starting at the cursor without writing anything,
    /// returning their start offset. Does **not** advance the cursor — call
    /// [`Self::commit`] once the region is actually filled (or use
    /// [`Self::reserve`] for the common exact-size case).
    #[inline(always)]
    fn reserve_capacity(&self, n: usize) -> Result<usize> {
        if self.finalized {
            return Err(HookError::InvalidArgument);
        }
        let end = self.pos.checked_add(n).ok_or(HookError::InvalidArgument)?;
        if end > self.buf.len() {
            return Err(HookError::InvalidArgument);
        }
        Ok(self.pos)
    }

    /// Advances the cursor to `start + actual_len`, checked against the
    /// buffer bounds. Pairs with [`Self::reserve_capacity`] when the real
    /// size written is only known after a host call (`etxn_details`,
    /// `float_sto`) returns it.
    #[inline(always)]
    fn commit(&mut self, start: usize, actual_len: usize) -> Result<()> {
        let end = start
            .checked_add(actual_len)
            .ok_or(HookError::InvalidArgument)?;
        if end > self.buf.len() {
            return Err(HookError::InvalidArgument);
        }
        self.pos = end;
        Ok(())
    }

    /// Reserves and immediately commits exactly `n` bytes, returning their
    /// start offset.
    #[inline(always)]
    fn reserve(&mut self, n: usize) -> Result<usize> {
        let start = self.reserve_capacity(n)?;
        self.commit(start, n)?;
        Ok(start)
    }

    /// Writes `data` at the cursor, advancing it by `data.len()`. Always
    /// inlined so `data.len()` resolves to a compile-time constant at each
    /// call site (every call here passes a fixed-size array literal or
    /// slice of statically known length) — otherwise this single shared
    /// body would compile to a length-generic `memcpy`, which lowers to an
    /// unguarded loop on `wasm32v1-none` (see `hook-rust-build`'s "full
    /// inlining is required" note).
    #[inline(always)]
    fn write_bytes(&mut self, data: &[u8]) -> Result<()> {
        let start = self.reserve(data.len())?;
        let end = start.wrapping_add(data.len());
        let dst = self
            .buf
            .get_mut(start..end)
            .ok_or(HookError::InvalidArgument)?;
        dst.copy_from_slice(data);
        Ok(())
    }

    // -- Container state ------------------------------------------------------

    #[inline(always)]
    fn top_kind(&self) -> Result<FrameKind> {
        self.frames
            .get(self.depth)
            .copied()
            .ok_or(HookError::InvalidArgument)
    }

    /// Checks that a write is legal at the writer's current state: not
    /// finalized, and — unless `allow_in_array` — not nested directly
    /// inside an open `STArray` (only [`Self::begin_object`] is legal
    /// there; see the module doc comment's structural rules). Field order
    /// is never checked — see the module doc comment.
    #[inline(always)]
    fn check_write(&self, allow_in_array: bool) -> Result<()> {
        if self.finalized {
            return Err(HookError::InvalidArgument);
        }
        if self.top_kind()? == FrameKind::Array && !allow_in_array {
            return Err(HookError::InvalidArgument);
        }
        Ok(())
    }

    /// Writes field `f`'s header bytes at the cursor. Callers run
    /// [`Self::check_write`] first.
    #[inline(always)]
    fn write_field_header_bytes<T>(&mut self, f: SField<T>) -> Result<()> {
        let (hdr, hdr_len) = codec::field_header(f);
        let hdr = hdr.get(..hdr_len).ok_or(HookError::InvalidArgument)?;
        self.write_bytes(hdr)
    }

    /// Whether `code` is one of the six required emit-plumbing fields
    /// (`sfSequence`/`sfFirstLedgerSequence`/`sfLastLedgerSequence`/
    /// `sfFee`/`sfSigningPubKey`/`sfAccount`) that has already been
    /// written once. `false` for every other field code.
    #[inline(always)]
    fn plumbing_already_set(&self, code: u32) -> bool {
        (code == sfSequence.code() && self.sequence_seen)
            || (code == sfFirstLedgerSequence.code() && self.first_ledger_sequence_off.is_some())
            || (code == sfLastLedgerSequence.code() && self.last_ledger_sequence_off.is_some())
            || (code == sfFee.code() && self.fee_off.is_some())
            || (code == sfSigningPubKey.code() && self.signing_pub_key_seen)
            || (code == sfAccount.code() && self.account_off.is_some())
    }

    // -- Scalar field writers -------------------------------------------------

    /// Writes an `STI_UINT16` field (e.g. `sfTransactionType`).
    ///
    /// # Errors
    ///
    /// [`HookError::InvalidArgument`] if the write does not fit, is nested
    /// directly inside an `STArray`, or the writer is already finalized
    /// (see [`Self::prepare_for_emit`]).
    #[inline(always)]
    pub fn u16_field(&mut self, f: SField<u16>, value: u16) -> Result<()> {
        self.check_write(false)?;
        self.write_field_header_bytes(f)?;
        self.write_bytes(&value.to_be_bytes())
    }

    /// Writes an `STI_UINT32` field (e.g. `sfFlags`, `sfSequence`).
    ///
    /// Recognizes `sfSequence`/`sfFirstLedgerSequence`/
    /// `sfLastLedgerSequence` by value and records what
    /// [`Self::prepare_for_emit`] needs from them — see the module doc
    /// comment.
    ///
    /// # Errors
    ///
    /// [`HookError::AlreadySet`] if `f` is `sfSequence`,
    /// `sfFirstLedgerSequence`, or `sfLastLedgerSequence` and has already
    /// been written once. Otherwise as [`Self::u16_field`].
    #[inline(always)]
    pub fn u32_field(&mut self, f: SField<u32>, value: u32) -> Result<()> {
        let code = f.code();
        if self.plumbing_already_set(code) {
            return Err(HookError::AlreadySet);
        }
        self.check_write(false)?;
        self.write_field_header_bytes(f)?;
        let value_off = self.pos;
        self.write_bytes(&value.to_be_bytes())?;
        if code == sfSequence.code() {
            self.sequence_seen = true;
        } else if code == sfFirstLedgerSequence.code() {
            self.first_ledger_sequence_off = Some(value_off);
        } else if code == sfLastLedgerSequence.code() {
            self.last_ledger_sequence_off = Some(value_off);
        }
        Ok(())
    }

    /// Writes an `STI_ACCOUNT` field (a 1-byte VL length of `20` followed by
    /// the 20 raw bytes — matches [`codec::write_account_id`]'s output).
    ///
    /// Recognizes `sfAccount` by value; see the module doc comment.
    ///
    /// # Errors
    ///
    /// [`HookError::AlreadySet`] if `f` is `sfAccount` and has already been
    /// written once. Otherwise as [`Self::u16_field`].
    #[inline(always)]
    pub fn account_id(&mut self, f: SField<AccountId>, value: &AccountId) -> Result<()> {
        let code = f.code();
        if self.plumbing_already_set(code) {
            return Err(HookError::AlreadySet);
        }
        self.check_write(false)?;
        self.write_field_header_bytes(f)?;
        self.write_bytes(&[ACC_ID_LEN as u8])?;
        let value_off = self.pos;
        self.write_bytes(value.as_ref())?;
        if code == sfAccount.code() {
            self.account_off = Some(value_off);
        }
        Ok(())
    }

    /// Writes an `STI_VL` field as an empty blob (a 1-byte zero-length VL
    /// marker, no payload) — what `SigningPubKey` looks like on an emitted
    /// transaction.
    ///
    /// Recognizes `sfSigningPubKey` by value; see the module doc comment.
    ///
    /// # Errors
    ///
    /// [`HookError::AlreadySet`] if `f` is `sfSigningPubKey` and has
    /// already been written once. Otherwise as [`Self::u16_field`].
    #[inline(always)]
    pub fn empty_vl(&mut self, f: SField<Opaque>) -> Result<()> {
        let code = f.code();
        if self.plumbing_already_set(code) {
            return Err(HookError::AlreadySet);
        }
        self.check_write(false)?;
        self.write_field_header_bytes(f)?;
        self.write_bytes(&[0u8])?;
        if code == sfSigningPubKey.code() {
            self.signing_pub_key_seen = true;
        }
        Ok(())
    }

    /// Writes an `STI_AMOUNT` field encoded as a native (XRP/XAH) amount —
    /// byte-identical to [`codec::encode_native_amount`]'s output, which
    /// this reuses directly.
    ///
    /// Recognizes `sfFee` by value; see the module doc comment.
    ///
    /// # Errors
    ///
    /// [`HookError::AlreadySet`] if `f` is `sfFee` and has already been
    /// written once. Otherwise [`HookError::InvalidArgument`] as
    /// [`Self::u16_field`], or if `drops` does not fit in 62 bits (see
    /// [`codec::encode_native_amount`]).
    #[inline(always)]
    pub fn native_amount(&mut self, f: SField<Amount>, drops: u64) -> Result<()> {
        let code = f.code();
        if self.plumbing_already_set(code) {
            return Err(HookError::AlreadySet);
        }
        self.check_write(false)?;
        self.write_field_header_bytes(f)?;
        let start = self.reserve(8)?;
        let dst = self
            .buf
            .get_mut(start..start.wrapping_add(8))
            .ok_or(HookError::InvalidArgument)?;
        codec::encode_native_amount(dst, drops)?;
        if code == sfFee.code() {
            self.fee_off = Some(start);
        }
        Ok(())
    }

    /// Writes an `STI_AMOUNT` field encoded as an issued (IOU) amount:
    /// `currency`/`issuer` plus `amount`'s mantissa/exponent, via the
    /// `float_sto` host call ([`crate::api::float::float_sto`]) — the same
    /// primitive [`XFL::sto`] wraps, reused here rather than re-deriving
    /// the 48-byte STAmount encoding locally.
    ///
    /// # Errors
    ///
    /// [`HookError::InvalidArgument`] if the write does not fit or is
    /// nested directly inside an `STArray` (as [`Self::u16_field`]);
    /// otherwise propagates `float_sto`'s error.
    #[inline(always)]
    pub fn iou_amount(
        &mut self,
        f: SField<Amount>,
        amount: XFL,
        currency: &CurrencyCode,
        issuer: &AccountId,
    ) -> Result<()> {
        self.check_write(false)?;
        let code = f.code();
        let (_, hdr_len) = codec::field_header(f);
        let cap = hdr_len
            .checked_add(IOU_AMOUNT_LEN)
            .ok_or(HookError::InvalidArgument)?;
        let start = self.reserve_capacity(cap)?;
        let end = start.wrapping_add(cap);
        let region = self
            .buf
            .get_mut(start..end)
            .ok_or(HookError::InvalidArgument)?;
        let written = float::float_sto(region, Some(currency), Some(issuer), amount, code)?;
        self.commit(start, written)?;
        Ok(())
    }

    // -- Containers -----------------------------------------------------------

    /// Opens a nested `STObject` field `f` (e.g. `sfAmountEntry`): writes
    /// `f`'s header and pushes a fresh container frame. Legal directly
    /// inside an `STArray` (as the element wrapper) or an `STObject`.
    ///
    /// # Errors
    ///
    /// [`HookError::InvalidArgument`] if the write does not fit, or would
    /// exceed [`STO_WRITER_MAX_DEPTH`].
    #[inline(always)]
    pub fn begin_object(&mut self, f: SField<STObject>) -> Result<()> {
        self.check_write(true)?;
        let next_depth = self
            .depth
            .checked_add(1)
            .ok_or(HookError::InvalidArgument)?;
        if next_depth >= STO_WRITER_MAX_DEPTH {
            return Err(HookError::InvalidArgument);
        }
        self.write_field_header_bytes(f)?;
        self.depth = next_depth;
        let frame = self
            .frames
            .get_mut(self.depth)
            .ok_or(HookError::InvalidArgument)?;
        *frame = FrameKind::Object;
        Ok(())
    }

    /// Opens a nested `STArray` field `f` (e.g. `sfAmounts`): writes `f`'s
    /// header and pushes a fresh container frame. Legal only directly
    /// inside an `STObject` — arrays do not nest directly inside another
    /// array (every real protocol usage wraps array elements in an
    /// `STObject` via [`Self::begin_object`] first).
    ///
    /// # Errors
    ///
    /// Same as [`Self::begin_object`].
    #[inline(always)]
    pub fn begin_array(&mut self, f: SField<STArray>) -> Result<()> {
        self.check_write(false)?;
        let next_depth = self
            .depth
            .checked_add(1)
            .ok_or(HookError::InvalidArgument)?;
        if next_depth >= STO_WRITER_MAX_DEPTH {
            return Err(HookError::InvalidArgument);
        }
        self.write_field_header_bytes(f)?;
        self.depth = next_depth;
        let frame = self
            .frames
            .get_mut(self.depth)
            .ok_or(HookError::InvalidArgument)?;
        *frame = FrameKind::Array;
        Ok(())
    }

    /// Closes the innermost open `STObject` (opened by the matching
    /// [`Self::begin_object`]), writing the `ObjectEndMarker` terminator.
    ///
    /// # Errors
    ///
    /// [`HookError::InvalidArgument`] if nothing is open, the innermost open
    /// container is an `STArray` (mismatched terminator), the write does not
    /// fit, or the writer is already finalized.
    #[inline(always)]
    pub fn end_object(&mut self) -> Result<()> {
        self.end_container(FrameKind::Object, OBJECT_END_MARKER)
    }

    /// Closes the innermost open `STArray` (opened by the matching
    /// [`Self::begin_array`]), writing the `ArrayEndMarker` terminator.
    ///
    /// # Errors
    ///
    /// Same as [`Self::end_object`], substituting `STObject` for `STArray`.
    #[inline(always)]
    pub fn end_array(&mut self) -> Result<()> {
        self.end_container(FrameKind::Array, ARRAY_END_MARKER)
    }

    #[inline(always)]
    fn end_container(&mut self, expected: FrameKind, marker: SField<Opaque>) -> Result<()> {
        if self.finalized {
            return Err(HookError::InvalidArgument);
        }
        if self.depth == 0 {
            return Err(HookError::InvalidArgument);
        }
        if self.top_kind()? != expected {
            return Err(HookError::InvalidArgument);
        }
        self.write_field_header_bytes(marker)?;
        self.depth = self.depth.wrapping_sub(1);
        Ok(())
    }

    // -- Dynamic emission preparation -----------------------------------------

    /// Validates the writer, patches the emit-plumbing fields, appends
    /// `sfEmitDetails`, and returns a [`crate::txn::Prepared`] handle — the
    /// dynamic counterpart to `txn_template!`'s generated
    /// `prepare_for_emit()`. Requires every container the caller opened to
    /// be closed and every required field ([`sfSequence`],
    /// [`sfFirstLedgerSequence`], [`sfLastLedgerSequence`], [`sfFee`],
    /// [`sfSigningPubKey`], [`sfAccount`]) to have already been written, in
    /// whatever order the caller chose, then:
    ///
    /// 1. Patches `FirstLedgerSequence`/`LastLedgerSequence` from
    ///    `ledger_seq() + 1` / `+ 5`.
    /// 2. Patches `Account` from [`crate::api::hook_ctx::hook_account`].
    /// 3. Appends the runtime-sized `sfEmitDetails` field at the current
    ///    cursor via [`crate::api::etxn::etxn_details`] — see the module
    ///    doc comment's "`prepare_for_emit` writes `EmitDetails` itself"
    ///    section for the buffer-headroom requirement this implies.
    /// 4. Computes `etxn_fee_base` over the full serialized prefix,
    ///    including the just-appended `EmitDetails`, and patches `Fee`.
    /// 5. Finalizes the writer (further writes fail with
    ///    [`HookError::InvalidArgument`]) and returns a
    ///    [`crate::txn::Prepared`] handle sized to exactly what was written
    ///    — the same typestate [`crate::txn::Prepared::emit`] already knows
    ///    how to emit.
    ///
    /// `Sequence` and `SigningPubKey` are left untouched (checked for
    /// presence only), exactly as in the macro.
    ///
    /// # Errors
    ///
    /// [`HookError::InvalidArgument`] if any container is still open, any
    /// required field was never written, `buf` lacks enough headroom for
    /// `EmitDetails`, or the writer is already finalized. Otherwise
    /// propagates `hook_account`/`etxn_details`/`etxn_fee_base`'s errors.
    #[inline(always)]
    pub fn prepare_for_emit(&mut self) -> Result<Prepared<'_, Self>> {
        if self.finalized {
            return Err(HookError::InvalidArgument);
        }
        if self.depth != 0 {
            return Err(HookError::InvalidArgument);
        }
        let (Some(fls_off), Some(lls_off), Some(fee_off), Some(account_off)) = (
            self.first_ledger_sequence_off,
            self.last_ledger_sequence_off,
            self.fee_off,
            self.account_off,
        ) else {
            return Err(HookError::InvalidArgument);
        };
        if !self.sequence_seen || !self.signing_pub_key_seen {
            return Err(HookError::InvalidArgument);
        }

        let fls = ledger::ledger_seq().wrapping_add(1);
        {
            let dst = self
                .buf
                .get_mut(fls_off..fls_off.wrapping_add(4))
                .ok_or(HookError::InvalidArgument)?;
            dst.copy_from_slice(&fls.to_be_bytes());
        }
        {
            let dst = self
                .buf
                .get_mut(lls_off..lls_off.wrapping_add(4))
                .ok_or(HookError::InvalidArgument)?;
            dst.copy_from_slice(&fls.wrapping_add(4).to_be_bytes());
        }
        {
            let dst = self
                .buf
                .get_mut(account_off..account_off.wrapping_add(ACC_ID_LEN))
                .ok_or(HookError::InvalidArgument)?;
            hook_ctx::hook_account(dst)?;
        }

        let ed_start = self.reserve_capacity(EMIT_DETAILS_MAX_LEN)?;
        let ed_end = ed_start.wrapping_add(EMIT_DETAILS_MAX_LEN);
        let ed_region = self
            .buf
            .get_mut(ed_start..ed_end)
            .ok_or(HookError::InvalidArgument)?;
        let ed_len = etxn::etxn_details(ed_region)?;
        self.commit(ed_start, ed_len)?;

        let total_len = self.pos;
        let fee = {
            let blob = self
                .buf
                .get(..total_len)
                .ok_or(HookError::InvalidArgument)?;
            etxn::etxn_fee_base(blob)?
        };
        {
            let dst = self
                .buf
                .get_mut(fee_off..fee_off.wrapping_add(8))
                .ok_or(HookError::InvalidArgument)?;
            codec::encode_native_amount(dst, fee)?;
        }

        self.finalized = true;
        Ok(Prepared::new(self, total_len))
    }
}

impl<'a> TemplateBytes for StoWriter<'a> {
    #[inline(always)]
    fn template_bytes(&self) -> &[u8] {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;
    use crate::sfield::{
        sfAmount, sfAmountEntry, sfAmounts, sfDestination, sfFirstLedgerSequence, sfFlags,
        sfLastLedgerSequence, sfSequence, sfSigningPubKey, sfTransactionType,
    };

    #[test]
    fn u16_field_writes_header_and_be_value() {
        let mut buf = [0u8; 16];
        let mut w = StoWriter::new(&mut buf);
        w.u16_field(sfTransactionType, 2).expect("fits");
        assert_eq!(w.as_bytes(), &[0x12, 0x00, 0x02]);
    }

    #[test]
    fn u32_field_writes_header_and_be_value() {
        let mut buf = [0u8; 16];
        let mut w = StoWriter::new(&mut buf);
        w.u32_field(sfFlags, 0x8000_0000).expect("fits");
        assert_eq!(w.as_bytes(), &[0x22, 0x80, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn account_id_matches_existing_codec_output() {
        let mut buf = [0u8; 64];
        let mut w = StoWriter::new(&mut buf);
        let id = AccountId([0xAB; ACC_ID_LEN]);
        w.account_id(sfAccount, &id).expect("fits");

        // sfAccount (8,1) -> 1-byte header 0x81, then the 1-byte VL length
        // (always 20), then the 20 raw bytes — matches
        // `codec::account_id_field_size`'s layout exactly.
        let mut expected = [0u8; 22];
        expected[0] = 0x81;
        expected[1] = ACC_ID_LEN as u8;
        expected[2..].copy_from_slice(&id.0);
        assert_eq!(w.as_bytes(), &expected[..]);
    }

    #[test]
    fn empty_vl_writes_zero_length_marker() {
        let mut buf = [0u8; 8];
        let mut w = StoWriter::new(&mut buf);
        w.empty_vl(sfSigningPubKey).expect("fits");
        assert_eq!(w.as_bytes(), &[0x73, 0x00]);
    }

    #[test]
    fn native_amount_matches_codec_const_encoding() {
        let mut buf = [0u8; 16];
        let mut w = StoWriter::new(&mut buf);
        w.native_amount(sfFee, 6).expect("6 drops is in range");
        let mut expected = [0u8; 9];
        expected[0] = 0x68; // sfFee (6,8) -> 1-byte header
        expected[1..].copy_from_slice(&codec::encode_native_amount_const(6));
        assert_eq!(w.as_bytes(), &expected[..]);
    }

    #[test]
    fn native_amount_rejects_out_of_range_drops() {
        let mut buf = [0u8; 16];
        let mut w = StoWriter::new(&mut buf);
        assert_eq!(
            w.native_amount(sfFee, 1u64 << 62),
            Err(HookError::InvalidArgument)
        );
    }

    #[test]
    fn exact_capacity_buffer_succeeds() {
        let mut buf = [0u8; 5];
        let mut w = StoWriter::new(&mut buf);
        assert!(w.u32_field(sfFlags, 1).is_ok());
        assert_eq!(w.len(), 5);
    }

    #[test]
    fn one_byte_short_buffer_fails() {
        // `sfFlags`'s 1-byte header fits in 4 bytes, but its 4-byte value
        // does not — the call still fails as a whole (the header write
        // alone is not a usable partial result; callers must treat any
        // `Err` as "stop using this writer").
        let mut buf = [0u8; 4];
        let mut w = StoWriter::new(&mut buf);
        assert_eq!(w.u32_field(sfFlags, 1), Err(HookError::InvalidArgument));
        // The failed value write never advanced the cursor past the header
        // it already committed.
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn zero_capacity_buffer_fails_even_a_single_header_byte() {
        let mut buf: [u8; 0] = [];
        let mut w = StoWriter::new(&mut buf);
        assert_eq!(
            w.u16_field(sfTransactionType, 0),
            Err(HookError::InvalidArgument)
        );
    }

    #[test]
    fn writes_fields_in_caller_supplied_order_without_reordering() {
        // sfDestination (8,3) is written before sfAccount (8,1) — outside
        // ascending (type, field) order. The writer preserves exactly the
        // order it was called in; see the module doc comment for why that
        // is fine (the host canonicalizes on re-serialization).
        let mut buf = [0u8; 64];
        let mut w = StoWriter::new(&mut buf);
        w.account_id(sfDestination, &AccountId([0xAA; ACC_ID_LEN]))
            .expect("fits");
        w.account_id(sfAccount, &AccountId([0xBB; ACC_ID_LEN]))
            .expect("fits");

        let mut expected = [0u8; 44];
        expected[0] = 0x83; // sfDestination header, written first
        expected[1] = ACC_ID_LEN as u8;
        expected[2..22].copy_from_slice(&[0xAA; ACC_ID_LEN]);
        expected[22] = 0x81; // sfAccount header, written second
        expected[23] = ACC_ID_LEN as u8;
        expected[24..].copy_from_slice(&[0xBB; ACC_ID_LEN]);
        assert_eq!(w.as_bytes(), &expected[..]);
    }

    #[test]
    fn non_plumbing_duplicate_field_is_allowed() {
        // sfFlags is not one of the six tracked emit-plumbing fields, so
        // StoWriter itself does not reject a repeat — see the module doc
        // comment's "duplicate rejection" section.
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.u32_field(sfFlags, 1).expect("fits");
        w.u32_field(sfFlags, 2).expect("fits, not tracked");
        assert_eq!(w.len(), 10);
    }

    #[test]
    fn duplicate_sequence_is_rejected() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.u32_field(sfSequence, 0).expect("fits");
        assert_eq!(w.u32_field(sfSequence, 1), Err(HookError::AlreadySet));
    }

    #[test]
    fn duplicate_first_ledger_sequence_is_rejected() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.u32_field(sfFirstLedgerSequence, 0).expect("fits");
        assert_eq!(
            w.u32_field(sfFirstLedgerSequence, 1),
            Err(HookError::AlreadySet)
        );
    }

    #[test]
    fn duplicate_last_ledger_sequence_is_rejected() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.u32_field(sfLastLedgerSequence, 0).expect("fits");
        assert_eq!(
            w.u32_field(sfLastLedgerSequence, 1),
            Err(HookError::AlreadySet)
        );
    }

    #[test]
    fn duplicate_fee_is_rejected() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.native_amount(sfFee, 0).expect("fits");
        assert_eq!(w.native_amount(sfFee, 1), Err(HookError::AlreadySet));
    }

    #[test]
    fn duplicate_signing_pub_key_is_rejected() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.empty_vl(sfSigningPubKey).expect("fits");
        assert_eq!(w.empty_vl(sfSigningPubKey), Err(HookError::AlreadySet));
    }

    #[test]
    fn duplicate_account_is_rejected() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.account_id(sfAccount, &AccountId::default())
            .expect("fits");
        assert_eq!(
            w.account_id(sfAccount, &AccountId::default()),
            Err(HookError::AlreadySet)
        );
    }

    #[test]
    fn begin_end_object_writes_header_and_terminator() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.begin_object(sfAmountEntry).expect("fits");
        w.native_amount(sfAmount, 5).expect("fits");
        w.end_object().expect("open object");
        let bytes = w.as_bytes();
        // sfAmountEntry (14,91): type<16, field>=16 -> 2-byte header
        // [type<<4, field] = [0xE0, 0x5B].
        assert_eq!(&bytes[..2], &[0xE0, 0x5B]);
        assert_eq!(*bytes.last().expect("non-empty"), 0xE1); // ObjectEndMarker
    }

    #[test]
    fn nested_array_of_objects_round_trips() {
        // Mirrors Remit's `sfAmounts: [ { AmountEntry: { Amount } } ]`
        // shape, using `native_amount` so this test needs no host call.
        let mut buf = [0u8; 64];
        let mut w = StoWriter::new(&mut buf);
        w.begin_array(sfAmounts).expect("fits");
        w.begin_object(sfAmountEntry).expect("fits");
        w.native_amount(sfAmount, 42).expect("fits");
        w.end_object().expect("open object");
        w.end_array().expect("open array");

        let bytes = w.as_bytes();
        // Ends with the ArrayEndMarker (0xF1), and contains the
        // ObjectEndMarker (0xE1) for the single element.
        assert_eq!(*bytes.last().expect("non-empty"), 0xF1);
        assert!(bytes.contains(&0xE1));
    }

    #[test]
    fn end_object_without_open_container_errors() {
        let mut buf = [0u8; 8];
        let mut w = StoWriter::new(&mut buf);
        assert_eq!(w.end_object(), Err(HookError::InvalidArgument));
    }

    #[test]
    fn mismatched_terminator_errors() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.begin_array(sfAmounts).expect("fits");
        // An array is open; closing it as an object is a mismatched
        // terminator.
        assert_eq!(w.end_object(), Err(HookError::InvalidArgument));
    }

    #[test]
    fn scalar_field_directly_inside_array_errors() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.begin_array(sfAmounts).expect("fits");
        // Only `begin_object` is valid directly inside an open array.
        assert_eq!(
            w.native_amount(sfAmount, 1),
            Err(HookError::InvalidArgument)
        );
    }

    #[test]
    fn array_cannot_nest_directly_inside_array() {
        let mut buf = [0u8; 32];
        let mut w = StoWriter::new(&mut buf);
        w.begin_array(sfAmounts).expect("fits");
        assert_eq!(w.begin_array(sfAmounts), Err(HookError::InvalidArgument));
    }

    #[test]
    fn exceeding_max_depth_errors() {
        let mut buf = [0u8; 4096];
        let mut w = StoWriter::new(&mut buf);
        for _ in 0..(STO_WRITER_MAX_DEPTH - 1) {
            w.begin_object(sfAmountEntry)
                .expect("fits and under the depth cap");
        }
        assert_eq!(
            w.begin_object(sfAmountEntry),
            Err(HookError::InvalidArgument)
        );
    }

    #[test]
    fn prepare_for_emit_rejects_unclosed_containers() {
        let mut buf = [0u8; 256];
        let mut w = StoWriter::new(&mut buf);
        w.begin_array(sfAmounts).expect("fits");
        assert_eq!(
            w.prepare_for_emit().expect_err("array still open"),
            HookError::InvalidArgument
        );
    }

    #[test]
    fn prepare_for_emit_rejects_missing_required_fields() {
        let mut buf = [0u8; 256];
        let mut w = StoWriter::new(&mut buf);
        w.u16_field(sfTransactionType, 0).expect("fits");
        // None of the required emit-plumbing fields were written.
        assert_eq!(
            w.prepare_for_emit().expect_err("missing required fields"),
            HookError::InvalidArgument
        );
    }

    #[test]
    fn prepare_for_emit_propagates_host_stub_error_once_fields_are_present() {
        let mut buf = [0u8; 256];
        let mut w = fully_populated_writer(&mut buf);
        // On the host target every Hook API call is a deterministic
        // NOT_IMPLEMENTED stub (see rshooks-core), so this must fail on the
        // first host call (`ledger_seq` inside `prepare_for_emit`) rather
        // than on the (already-passed) presence checks.
        assert_eq!(
            w.prepare_for_emit().expect_err("host stub"),
            HookError::NotImplemented
        );
    }

    /// Writes every required emit-plumbing field — used by tests that only
    /// care about `prepare_for_emit`'s presence checks / host-call
    /// plumbing, not the exact bytes.
    fn fully_populated_writer<'a>(buf: &'a mut [u8; 256]) -> StoWriter<'a> {
        let mut w = StoWriter::new(buf);
        w.u16_field(sfTransactionType, 0).expect("fits");
        w.u32_field(sfFlags, 0).expect("fits");
        w.u32_field(sfSequence, 0).expect("fits");
        w.u32_field(sfFirstLedgerSequence, 0).expect("fits");
        w.u32_field(sfLastLedgerSequence, 0).expect("fits");
        w.native_amount(sfFee, 0).expect("fits");
        w.empty_vl(sfSigningPubKey).expect("fits");
        w.account_id(sfAccount, &AccountId::default())
            .expect("fits");
        w
    }
}

#[cfg(all(test, feature = "testenv", not(target_arch = "wasm32")))]
mod testenv_tests {
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        clippy::indexing_slicing
    )] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8
    extern crate std;

    use std::rc::Rc;
    use std::vec;
    use std::vec::Vec;

    use super::*;
    use crate::sfield::{
        sfAccount, sfFee, sfFirstLedgerSequence, sfFlags, sfLastLedgerSequence, sfSequence,
        sfSigningPubKey, sfTransactionType,
    };
    use rshooks_core::backend::HostBackend;

    /// A minimal mock backend covering only what `StoWriter::prepare_for_emit`
    /// and `StoWriter::iou_amount` need: `hook_account`, `ledger_seq`,
    /// `etxn_fee_base`, `etxn_details` (configurable length, to prove
    /// variable-`EmitDetails`-length handling), and `float_sto` (returns a
    /// fixed, deterministic payload — this crate does not re-verify
    /// `float_sto`'s own STAmount byte format, only that `StoWriter` plumbs
    /// its actual returned length correctly).
    struct MockBackend {
        account: [u8; 20],
        ledger_seq: i64,
        fee_base: i64,
        emit_details: Vec<u8>,
        float_sto_out: Vec<u8>,
    }

    impl HostBackend for MockBackend {
        fn hook_account(&self) -> core::result::Result<[u8; 20], i64> {
            Ok(self.account)
        }
        fn ledger_seq(&self) -> i64 {
            self.ledger_seq
        }
        fn etxn_fee_base(&self, _tx_blob: &[u8]) -> i64 {
            self.fee_base
        }
        fn etxn_details(&self) -> core::result::Result<Vec<u8>, i64> {
            Ok(self.emit_details.clone())
        }
        fn float_sto(
            &self,
            _currency: Option<&[u8]>,
            _issuer: Option<&[u8]>,
            _amount: i64,
            _field_code: u32,
        ) -> core::result::Result<Vec<u8>, i64> {
            Ok(self.float_sto_out.clone())
        }
        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("MockBackend::accept: not exercised by these tests")
        }
        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("MockBackend::rollback: not exercised by these tests")
        }
    }

    fn write_required_prefix(w: &mut StoWriter<'_>) {
        w.u16_field(sfTransactionType, 95).expect("fits"); // ttREMIT
        w.u32_field(sfFlags, 0).expect("fits");
        w.u32_field(sfSequence, 0).expect("fits");
        w.u32_field(sfFirstLedgerSequence, 0).expect("fits");
        w.u32_field(sfLastLedgerSequence, 0).expect("fits");
        w.native_amount(sfFee, 0).expect("fits");
        w.empty_vl(sfSigningPubKey).expect("fits");
        w.account_id(sfAccount, &AccountId::default())
            .expect("fits");
    }

    #[test]
    fn prepare_for_emit_patches_ledger_sequences_account_fee_and_appends_emit_details() {
        let backend = Rc::new(MockBackend {
            account: [0xAB; 20],
            ledger_seq: 1000,
            fee_base: 12,
            emit_details: vec![0u8; 116],
            float_sto_out: Vec::new(),
        });
        let _guard = rshooks_core::backend::install(backend);

        let mut buf = [0u8; 512];
        let mut w = StoWriter::new(&mut buf);
        write_required_prefix(&mut w);
        let (fls_off, lls_off, account_off, fee_off) = (
            w.first_ledger_sequence_off.expect("recorded"),
            w.last_ledger_sequence_off.expect("recorded"),
            w.account_off.expect("recorded"),
            w.fee_off.expect("recorded"),
        );
        let before_prepare = w.len();
        let prepared_len;
        {
            let prepared = w.prepare_for_emit().expect("all required fields present");
            prepared_len = prepared.as_bytes().len();
        }

        let bytes = w.as_bytes();
        // FirstLedgerSequence patched to ledger_seq()+1, LastLedgerSequence to +5.
        assert_eq!(&bytes[fls_off..fls_off + 4], &1001u32.to_be_bytes()[..]);
        assert_eq!(&bytes[lls_off..lls_off + 4], &1005u32.to_be_bytes()[..]);
        assert_eq!(&bytes[account_off..account_off + 20], &[0xABu8; 20][..]);
        assert_eq!(
            &bytes[fee_off..fee_off + 8],
            &codec::encode_native_amount_const(12)[..]
        );
        // `EmitDetails` was appended by `prepare_for_emit` itself, past
        // everything the caller wrote.
        assert_eq!(prepared_len, before_prepare + 116);
        assert_eq!(prepared_len, w.len());
    }

    #[test]
    fn prepare_for_emit_sizes_the_prefix_to_the_variable_emit_details_length() {
        // 138 bytes (cbak-exporting hook) instead of 116 — proves the
        // writer trusts `etxn_details`'s *returned* length, not a fixed
        // assumption.
        let backend = Rc::new(MockBackend {
            account: [0x11; 20],
            ledger_seq: 5,
            fee_base: 10,
            emit_details: vec![0xEEu8; 138],
            float_sto_out: Vec::new(),
        });
        let _guard = rshooks_core::backend::install(backend);

        let mut buf = [0u8; 512];
        let mut w = StoWriter::new(&mut buf);
        write_required_prefix(&mut w);
        let before_prepare = w.len();

        let prepared = w.prepare_for_emit().expect("all required fields present");
        assert_eq!(prepared.as_bytes().len(), before_prepare + 138);
    }

    #[test]
    fn prepare_for_emit_rejects_insufficient_emit_details_headroom() {
        let backend = Rc::new(MockBackend {
            account: [0; 20],
            ledger_seq: 1,
            fee_base: 10,
            emit_details: vec![0u8; 116],
            float_sto_out: Vec::new(),
        });
        let _guard = rshooks_core::backend::install(backend);

        // Sized to exactly one byte short of the required prefix plus
        // EMIT_DETAILS_MAX_LEN headroom.
        let prefix_len = {
            let mut probe_buf = [0u8; 256];
            let mut probe = StoWriter::new(&mut probe_buf);
            write_required_prefix(&mut probe);
            probe.len()
        };
        let mut buf = alloc_zeroed(prefix_len + EMIT_DETAILS_MAX_LEN - 1);
        let mut w = StoWriter::new(&mut buf);
        write_required_prefix(&mut w);
        assert_eq!(w.prepare_for_emit().err(), Some(HookError::InvalidArgument));
    }

    /// A zeroed `Vec<u8>` of length `n`, usable as a runtime-sized backing
    /// buffer — only test code needs a non-const-generic size here.
    fn alloc_zeroed(n: usize) -> Vec<u8> {
        vec![0u8; n]
    }

    #[test]
    fn writes_after_finalization_are_rejected() {
        let backend = Rc::new(MockBackend {
            account: [0; 20],
            ledger_seq: 1,
            fee_base: 10,
            emit_details: vec![0u8; 116],
            float_sto_out: Vec::new(),
        });
        let _guard = rshooks_core::backend::install(backend);

        let mut buf = [0u8; 512];
        let mut w = StoWriter::new(&mut buf);
        write_required_prefix(&mut w);
        let _ = w.prepare_for_emit().expect("all required fields present");

        assert_eq!(w.u32_field(sfFlags, 1), Err(HookError::InvalidArgument));
        assert_eq!(w.prepare_for_emit().err(), Some(HookError::InvalidArgument));
    }

    #[test]
    fn iou_amount_commits_the_actual_returned_length() {
        let mut float_out = vec![0x61u8]; // 1-byte header for a type<16,field<16 code
        float_out.extend_from_slice(&[0u8; IOU_AMOUNT_LEN]);
        let backend = Rc::new(MockBackend {
            account: [0; 20],
            ledger_seq: 1,
            fee_base: 10,
            emit_details: Vec::new(),
            float_sto_out: float_out.clone(),
        });
        let _guard = rshooks_core::backend::install(backend);

        let mut buf = [0u8; 128];
        let mut w = StoWriter::new(&mut buf);
        let currency = CurrencyCode::default();
        let issuer = AccountId::default();
        w.iou_amount(sfFee, XFL::one(), &currency, &issuer)
            .expect("mock backend accepts");
        assert_eq!(w.len(), float_out.len());
        assert_eq!(w.as_bytes(), &float_out[..]);
    }
}
