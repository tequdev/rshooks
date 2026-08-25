//! [`RemitBuilder`]: a typed builder for an emitted `Remit` transaction's
//! variable-length `sfAmounts` array, built directly on
//! [`crate::sto_writer::StoWriter`].
//!
//! `Remit` (`ttREMIT`) is the one protocol transaction whose emission needs
//! [`StoWriter`]'s runtime-sized `STArray` support: `sfAmounts` holds zero or
//! more `sfAmountEntry` objects (each wrapping a single `sfAmount`), one per
//! destination-bound asset, with no compile-time-known count. Rather than
//! have every hook author re-derive Remit's field layout and container
//! structure over [`StoWriter`] directly, [`RemitBuilder`] bakes that layout
//! in once.
//!
//! # Field layout
//!
//! [`RemitBuilder::new`] writes `TransactionType`, `Flags`, `Sequence`,
//! `FirstLedgerSequence`, `LastLedgerSequence`, `Fee`, `SigningPubKey`,
//! `Account`, and `Destination` (in that order — [`StoWriter`] does not
//! require any particular order, so this is simply the layout `new` uses),
//! then opens the `Amounts` array — so by the time `new` returns,
//! [`RemitBuilder::push_native_amount`]/[`RemitBuilder::push_issued_amount`]
//! only ever need to open one `sfAmountEntry` object, write its `sfAmount`,
//! and close it. [`RemitBuilder::prepare_for_emit`] closes the `Amounts`
//! array and delegates to [`StoWriter::prepare_for_emit`], which appends
//! `sfEmitDetails` itself — `buf` must have
//! [`EMIT_DETAILS_MAX_LEN`](crate::types::EMIT_DETAILS_MAX_LEN) bytes of
//! headroom beyond the fixed prefix and every amount pushed, for that append
//! to succeed (see [`StoWriter::new`]'s doc comment).
//!
//! # Error taxonomy
//!
//! [`RemitError`] separates failures by which part of the builder produced
//! them (matching the categories a caller needs to react to differently):
//! [`RemitError::Capacity`] for anything [`StoWriter`] rejected while
//! writing the fixed prefix or an `sfAmountEntry` wrapper,
//! [`RemitError::InvalidAmount`] for a native `drops` value out of range,
//! [`RemitError::Encoding`] for an issued amount's `float_sto` write, and
//! [`RemitError::Preparation`] for [`RemitBuilder::prepare_for_emit`]. Once
//! prepared, emission failures surface as [`Prepared::emit`]'s own
//! [`HookError`] directly, not wrapped in a `RemitError`.

use crate::error::{HookError, Result};
use crate::sfield::{
    sfAccount, sfAmount, sfAmountEntry, sfAmounts, sfDestination, sfFee, sfFirstLedgerSequence,
    sfFlags, sfLastLedgerSequence, sfSequence, sfSigningPubKey, sfTransactionType,
};
use crate::sto_writer::StoWriter;
use crate::tx_type::TxType;
use crate::txn::{Prepared, codec};
use crate::types::{AccountId, CurrencyCode};
use crate::xfl::XFL;

/// Errors specific to [`RemitBuilder`] — see the module doc comment's "Error
/// taxonomy" section for how each variant maps to a builder operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemitError {
    /// The destination buffer was too small, or a container-nesting
    /// invariant was violated — a [`StoWriter`] rejection of the fixed
    /// prefix [`RemitBuilder::new`] writes, or of the `sfAmountEntry`
    /// wrapper every push opens/closes.
    Capacity,
    /// [`RemitBuilder::push_native_amount`]'s `drops` did not fit a native
    /// amount's 62-bit range.
    InvalidAmount,
    /// [`RemitBuilder::push_issued_amount`]'s `sfAmount` write failed —
    /// either it did not fit (as [`RemitError::Capacity`]) or the
    /// underlying `float_sto` host call rejected the currency/issuer/
    /// amount; both surface here since [`StoWriter::iou_amount`] does not
    /// distinguish them.
    Encoding(HookError),
    /// [`RemitBuilder::prepare_for_emit`] failed: closing the `Amounts`
    /// array or the underlying [`StoWriter::prepare_for_emit`] rejected the
    /// writer's state (including insufficient headroom for the
    /// `sfEmitDetails` it appends), or a host call it makes
    /// (`ledger_seq`/`hook_account`/`etxn_details`/`etxn_fee_base`) failed.
    Preparation(HookError),
}

/// A bounded, allocation-free builder for an emitted `Remit` transaction's
/// `sfAmounts` array, backed entirely by caller-owned storage (via
/// [`StoWriter`]). See the module doc comment for the field layout and
/// error handling.
#[must_use = "a RemitBuilder that is never prepared/emitted means the writes were wasted"]
pub struct RemitBuilder<'a> {
    writer: StoWriter<'a>,
}

impl<'a> RemitBuilder<'a> {
    /// Wraps `buf` as a fresh builder: writes `Remit`'s fixed prefix
    /// (`TransactionType` through `Destination`) and opens the `Amounts`
    /// array — leaving the builder ready for
    /// [`Self::push_native_amount`]/[`Self::push_issued_amount`]. See the
    /// module doc comment for the exact field layout.
    ///
    /// `FirstLedgerSequence`/`LastLedgerSequence`/`Fee`/`Account` are
    /// written as placeholders here and patched by
    /// [`Self::prepare_for_emit`]; `destination` is the only caller-supplied
    /// field written up front. `buf` must have room for the fixed prefix,
    /// every amount [`Self::push_native_amount`]/[`Self::push_issued_amount`]
    /// will append, and [`EMIT_DETAILS_MAX_LEN`](crate::types::EMIT_DETAILS_MAX_LEN)
    /// bytes beyond that for [`Self::prepare_for_emit`] to append
    /// `sfEmitDetails`.
    ///
    /// # Errors
    ///
    /// [`RemitError::Capacity`] if `buf` is too small to hold the fixed
    /// prefix.
    #[inline(always)]
    pub fn new(
        buf: &'a mut [u8],
        destination: &AccountId,
    ) -> core::result::Result<Self, RemitError> {
        let mut writer = StoWriter::new(buf);
        write_prefix(&mut writer, destination).map_err(|_| RemitError::Capacity)?;
        Ok(Self { writer })
    }

    /// Appends one `sfAmountEntry` holding a native (XAH/XRP) `sfAmount` of
    /// `drops`.
    ///
    /// # Errors
    ///
    /// [`RemitError::InvalidAmount`] if `drops` does not fit a native
    /// amount's 62-bit range. [`RemitError::Capacity`] if the entry does not
    /// fit the remaining buffer, or the builder was already prepared (see
    /// [`Self::prepare_for_emit`]).
    #[inline(always)]
    pub fn push_native_amount(&mut self, drops: u64) -> core::result::Result<(), RemitError> {
        if drops >= codec::MAX_NATIVE_DROPS {
            return Err(RemitError::InvalidAmount);
        }
        self.writer
            .begin_object(sfAmountEntry)
            .map_err(|_| RemitError::Capacity)?;
        self.writer
            .native_amount(sfAmount, drops)
            .map_err(|_| RemitError::Capacity)?;
        self.writer.end_object().map_err(|_| RemitError::Capacity)
    }

    /// Appends one `sfAmountEntry` holding an issued (IOU) `sfAmount` of
    /// `amount` in `currency` issued by `issuer` — the same shape
    /// [`StoWriter::iou_amount`] takes.
    ///
    /// # Errors
    ///
    /// [`RemitError::Capacity`] if opening/closing the entry's wrapper
    /// object does not fit, or the builder was already prepared. See
    /// [`RemitError::Encoding`]'s doc comment for what a `sfAmount` write
    /// failure maps to.
    #[inline(always)]
    pub fn push_issued_amount(
        &mut self,
        amount: XFL,
        currency: &CurrencyCode,
        issuer: &AccountId,
    ) -> core::result::Result<(), RemitError> {
        self.writer
            .begin_object(sfAmountEntry)
            .map_err(|_| RemitError::Capacity)?;
        self.writer
            .iou_amount(sfAmount, amount, currency, issuer)
            .map_err(RemitError::Encoding)?;
        self.writer.end_object().map_err(|_| RemitError::Capacity)
    }

    /// Closes the `Amounts` array and delegates to
    /// [`StoWriter::prepare_for_emit`] — patching
    /// `FirstLedgerSequence`/`LastLedgerSequence`/`Account`/`Fee`, appending
    /// `sfEmitDetails`, and returning the [`Prepared`] handle ready for
    /// [`Prepared::emit`].
    ///
    /// Borrows `self` mutably for as long as the returned [`Prepared`] is
    /// alive, so the safe API has no way to call
    /// [`Self::push_native_amount`]/[`Self::push_issued_amount`] again until
    /// it is dropped — see the module doc comment's example.
    ///
    /// # Errors
    ///
    /// [`RemitError::Preparation`] if the array fails to close (should not
    /// happen through this type's own API) or the underlying
    /// `prepare_for_emit` fails (insufficient headroom for `sfEmitDetails`,
    /// a host call failure, or the builder was already prepared).
    #[inline(always)]
    pub fn prepare_for_emit(
        &mut self,
    ) -> core::result::Result<Prepared<'_, StoWriter<'a>>, RemitError> {
        self.writer.end_array().map_err(RemitError::Preparation)?;
        self.writer
            .prepare_for_emit()
            .map_err(RemitError::Preparation)
    }
}

/// Writes `Remit`'s fixed prefix and opens the `Amounts` array — see the
/// module doc comment's "Field layout" section. Separated from
/// [`RemitBuilder::new`] purely so the constructor's `?` sites all resolve
/// through one `map_err`.
#[inline(always)]
fn write_prefix(writer: &mut StoWriter<'_>, destination: &AccountId) -> Result<()> {
    writer.u16_field(sfTransactionType, TxType::Remit.code())?;
    writer.u32_field(sfFlags, rshooks_core::tfCANONICAL)?;
    writer.u32_field(sfSequence, 0)?;
    writer.u32_field(sfFirstLedgerSequence, 0)?;
    writer.u32_field(sfLastLedgerSequence, 0)?;
    writer.native_amount(sfFee, 0)?;
    writer.empty_vl(sfSigningPubKey)?;
    writer.account_id(sfAccount, &AccountId::default())?;
    writer.account_id(sfDestination, destination)?;
    writer.begin_array(sfAmounts)?;
    Ok(())
}

/// Calling a push method after [`RemitBuilder::prepare_for_emit`] fails to
/// compile: the returned [`Prepared`] borrows `remit` mutably for as long as
/// it is alive, so the borrow checker rejects a second mutable borrow for
/// `push_native_amount` — the safe API has no runtime-checked way to reach
/// this state (it never gets the chance to).
/// ```compile_fail
/// use rshooks::prelude::*;
///
/// let mut buf = [0u8; 512];
/// let mut remit = RemitBuilder::new(&mut buf, &AccountId::default()).expect("fits");
/// remit.push_native_amount(1).expect("fits");
/// let prepared = remit.prepare_for_emit().expect("all required fields present");
/// remit.push_native_amount(2); // ERROR: `remit` is still borrowed by `prepared`
/// let _ = prepared;
/// ```
#[cfg(doctest)]
struct PushAfterPrepareIsRejected;

#[cfg(all(test, feature = "testenv", not(target_arch = "wasm32")))]
mod tests {
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
    use crate::types::ACC_ID_LEN;
    use rshooks_core::backend::HostBackend;

    /// A minimal mock backend covering exactly what `RemitBuilder` needs:
    /// `ledger_seq`/`hook_account`/`etxn_details`/`etxn_fee_base` (by
    /// `prepare_for_emit`) and `emit` (by `Prepared::emit`). `float_sto` is
    /// deliberately left at its default `NOT_IMPLEMENTED` body for the
    /// `Encoding` failure test.
    struct MockBackend {
        account: [u8; 20],
        ledger_seq: i64,
        fee_base: i64,
        emit_details: Vec<u8>,
        emit_hash: [u8; 32],
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
        fn emit(&self, _tx_blob: &[u8]) -> core::result::Result<[u8; 32], i64> {
            Ok(self.emit_hash)
        }
        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("MockBackend::accept: not exercised by these tests")
        }
        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("MockBackend::rollback: not exercised by these tests")
        }
    }

    const EMIT_DETAILS: [u8; 116] = [0xEE_u8; 116];

    fn install_backend() -> rshooks_core::backend::BackendGuard {
        rshooks_core::backend::install(Rc::new(MockBackend {
            account: [0xAB; 20],
            ledger_seq: 1000,
            fee_base: 12,
            emit_details: EMIT_DETAILS.to_vec(),
            emit_hash: [0x99; 32],
        }))
    }

    /// Independently reconstructs the expected bytes of `Remit`'s fixed
    /// prefix (`TransactionType` through `Destination`), using the same
    /// codec primitives `StoWriter`/`txn_template!` are built on rather than
    /// re-deriving header bytes by hand.
    fn expected_prefix(
        destination: &AccountId,
        fls: u32,
        lls: u32,
        account: &AccountId,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        push_header(&mut out, sfTransactionType);
        out.extend_from_slice(&TxType::Remit.code().to_be_bytes());
        push_header(&mut out, sfFlags);
        out.extend_from_slice(&rshooks_core::tfCANONICAL.to_be_bytes());
        push_header(&mut out, sfSequence);
        out.extend_from_slice(&0u32.to_be_bytes());
        push_header(&mut out, sfFirstLedgerSequence);
        out.extend_from_slice(&fls.to_be_bytes());
        push_header(&mut out, sfLastLedgerSequence);
        out.extend_from_slice(&lls.to_be_bytes());
        push_header(&mut out, sfFee);
        out.extend_from_slice(&codec::encode_native_amount_const(12));
        push_header(&mut out, sfSigningPubKey);
        out.push(0);
        push_header(&mut out, sfAccount);
        out.push(ACC_ID_LEN as u8);
        out.extend_from_slice(account.as_ref());
        push_header(&mut out, sfDestination);
        out.push(ACC_ID_LEN as u8);
        out.extend_from_slice(destination.as_ref());
        out
    }

    fn push_header<T>(out: &mut Vec<u8>, f: crate::types::SField<T>) {
        let (hdr, len) = codec::field_header(f);
        out.extend_from_slice(&hdr[..len]);
    }

    /// One `sfAmountEntry` wrapping a native `sfAmount` of `drops`.
    fn amount_entry_bytes(drops: u64) -> Vec<u8> {
        let mut out = Vec::new();
        push_header(&mut out, sfAmountEntry);
        push_header(&mut out, sfAmount);
        out.extend_from_slice(&codec::encode_native_amount_const(drops));
        out.push(0xE1); // ObjectEndMarker
        out
    }

    #[test]
    fn one_amount_produces_byte_exact_output() {
        let _guard = install_backend();
        let destination = AccountId([0x22; ACC_ID_LEN]);

        let mut buf = [0u8; 512];
        let mut remit = RemitBuilder::new(&mut buf, &destination).expect("fits");
        remit.push_native_amount(42).expect("fits");
        let prepared = remit
            .prepare_for_emit()
            .expect("all required fields present");

        let mut expected =
            expected_prefix(&destination, 1001, 1005, &AccountId([0xAB; ACC_ID_LEN]));
        push_header(&mut expected, sfAmounts);
        expected.extend_from_slice(&amount_entry_bytes(42));
        expected.push(0xF1); // ArrayEndMarker
        expected.extend_from_slice(&EMIT_DETAILS); // appended by prepare_for_emit

        assert_eq!(prepared.as_bytes(), &expected[..]);
    }

    #[test]
    fn two_amounts_produce_byte_exact_output() {
        let _guard = install_backend();
        let destination = AccountId([0x33; ACC_ID_LEN]);

        let mut buf = [0u8; 512];
        let mut remit = RemitBuilder::new(&mut buf, &destination).expect("fits");
        remit.push_native_amount(1).expect("fits");
        remit.push_native_amount(2).expect("fits");
        let prepared = remit
            .prepare_for_emit()
            .expect("all required fields present");

        let mut expected =
            expected_prefix(&destination, 1001, 1005, &AccountId([0xAB; ACC_ID_LEN]));
        push_header(&mut expected, sfAmounts);
        expected.extend_from_slice(&amount_entry_bytes(1));
        expected.extend_from_slice(&amount_entry_bytes(2));
        expected.push(0xF1); // ArrayEndMarker
        expected.extend_from_slice(&EMIT_DETAILS); // appended by prepare_for_emit

        assert_eq!(prepared.as_bytes(), &expected[..]);
    }

    /// Whether a full `new` + one `push_native_amount` + `prepare_for_emit`
    /// flow succeeds against a `len`-byte buffer.
    fn one_amount_flow_fits(destination: &AccountId, len: usize) -> bool {
        let mut buf = vec![0u8; len];
        let Ok(mut remit) = RemitBuilder::new(&mut buf, destination) else {
            return false;
        };
        if remit.push_native_amount(7).is_err() {
            return false;
        }
        remit.prepare_for_emit().is_ok()
    }

    /// Finds the exact buffer-length boundary between the flow above
    /// succeeding and failing, then proves the boundary is exact: one byte
    /// less always fails (with `RemitError::Capacity`/`Preparation`, not a
    /// panic — `StoWriter`'s own bounds checks reject every construction/
    /// push/close/`sfEmitDetails`-append along the way, so scanning the
    /// whole `0..=512` range below also doubles as a no-out-of-bounds-write
    /// proof for every undersized length, not just the boundary itself).
    #[test]
    fn capacity_boundary_is_exact_and_never_panics() {
        let _guard = install_backend();
        let destination = AccountId([0x44; ACC_ID_LEN]);

        let min_len = (0..=512)
            .find(|&len| one_amount_flow_fits(&destination, len))
            .expect("512 bytes is enough for one native amount");
        assert!(!one_amount_flow_fits(&destination, min_len - 1));
    }

    /// Whether two successive `push_native_amount` calls (after `new`, with
    /// no `prepare_for_emit`) both succeed against a `len`-byte buffer.
    fn two_pushes_fit(destination: &AccountId, len: usize) -> bool {
        let mut buf = vec![0u8; len];
        let Ok(mut remit) = RemitBuilder::new(&mut buf, destination) else {
            return false;
        };
        if remit.push_native_amount(7).is_err() {
            return false;
        }
        remit.push_native_amount(8).is_ok()
    }

    #[test]
    fn second_push_can_fail_as_capacity_after_the_first_succeeds() {
        let _guard = install_backend();
        let destination = AccountId([0x66; ACC_ID_LEN]);

        // One byte short of what two entries need (independent of
        // `sfEmitDetails`'s headroom, which `prepare_for_emit` — not
        // `push_native_amount` — is responsible for): the first entry still
        // fits, but the second does not.
        let min_two = (0..=512)
            .find(|&len| two_pushes_fit(&destination, len))
            .expect("512 bytes is enough for two native amounts");

        let mut buf = vec![0u8; min_two - 1];
        let mut remit = RemitBuilder::new(&mut buf, &destination).expect("fits one entry");
        remit.push_native_amount(7).expect("first entry fits");
        assert_eq!(
            remit.push_native_amount(8),
            Err(RemitError::Capacity),
            "no room left for a second entry"
        );
    }

    #[test]
    fn native_amount_out_of_range_is_rejected_as_invalid_amount() {
        let _guard = install_backend();
        let mut buf = [0u8; 512];
        let mut remit = RemitBuilder::new(&mut buf, &AccountId::default()).expect("fits");
        assert_eq!(
            remit.push_native_amount(1u64 << 62),
            Err(RemitError::InvalidAmount)
        );
    }

    #[test]
    fn issued_amount_without_a_float_sto_backend_is_rejected_as_encoding() {
        let _guard = install_backend();
        let mut buf = [0u8; 512];
        let mut remit = RemitBuilder::new(&mut buf, &AccountId::default()).expect("fits");
        let currency = CurrencyCode::default();
        let issuer = AccountId::default();
        assert_eq!(
            remit.push_issued_amount(XFL::one(), &currency, &issuer),
            Err(RemitError::Encoding(HookError::NotImplemented))
        );
    }

    #[test]
    fn prepare_for_emit_patches_plumbing_fields_and_emits() {
        let _guard = install_backend();
        let mut buf = [0u8; 512];
        let mut remit = RemitBuilder::new(&mut buf, &AccountId([0x66; ACC_ID_LEN])).expect("fits");
        remit.push_native_amount(9).expect("fits");
        let prepared = remit
            .prepare_for_emit()
            .expect("all required fields present");
        let hash = prepared.emit().expect("mock backend accepts");
        assert_eq!(hash.as_ref(), &[0x99u8; 32][..]);
    }

    #[test]
    fn zero_amounts_prepares_with_an_empty_array_and_appended_emit_details() {
        let _guard = install_backend();
        let destination = AccountId::default();
        let mut buf = [0u8; 512];
        let mut remit = RemitBuilder::new(&mut buf, &destination).expect("fits");
        let prepared = remit.prepare_for_emit().expect("no pushes required");

        let mut expected =
            expected_prefix(&destination, 1001, 1005, &AccountId([0xAB; ACC_ID_LEN]));
        push_header(&mut expected, sfAmounts);
        expected.push(0xF1); // ArrayEndMarker, immediately (no entries)
        expected.extend_from_slice(&EMIT_DETAILS); // appended by prepare_for_emit

        assert_eq!(prepared.as_bytes(), &expected[..]);
    }
}
