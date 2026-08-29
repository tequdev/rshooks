//! Behavior coverage for the generated views (`rshooks::views`) against a
//! live mock host.
//!
//! `crates/rshooks/tests/slot_object.rs` can only prove typing and
//! reachability — every host call there returns `NOT_IMPLEMENTED`. These
//! tests run the same accessors against `TestEnv`'s modeled world, so they
//! prove the parts that actually needed a host: that a required field comes
//! back, that an absent optional field is `Ok(None)` and not an error, that
//! a constructor rejects the wrong type, and that a slot-backed view really
//! does release every child slot it opens.
//!
//! **The generator is the unit under test, not 134 instantiations of its
//! template.** Three representative views are covered — `tx::Payment` (both
//! sources), `ledger::RippleState`, `inner::EmitDetails` — because every
//! other view is the same rendering of the same `crate::views::source`
//! primitives with different field names.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::prelude::*;
use rshooks::txn::codec::encode_native_amount_const;
use rshooks::views::{inner, ledger, tx};
use rshooks::*;
use rshooks_testenv::prelude::*;

// ---------------------------------------------------------------------
// Serialized-object fixtures
// ---------------------------------------------------------------------

/// The serialized type IDs whose wire form carries a VL length prefix —
/// `crate::otxn`'s own `STI_VL`/`STI_ACCOUNT` pair, mirrored here because
/// these fixtures build the *full* wire form (`TestEnv::ledger_object`
/// takes a serialized object), not the value-only bytes `Otxn::field_raw`
/// takes.
const STI_VL: u32 = 7;
const STI_ACCOUNT: u32 = 8;

/// One field's wire header, in rippled's four-case encoding: the low
/// nibble pair when both halves fit in four bits, otherwise the
/// out-of-band byte(s).
fn header(code: u32) -> Vec<u8> {
    let ty = code >> 16;
    let field = code & 0xFFFF;
    match (ty < 16, field < 16) {
        (true, true) => vec![((ty << 4) | field) as u8],
        (true, false) => vec![(ty << 4) as u8, field as u8],
        (false, true) => vec![field as u8, ty as u8],
        (false, false) => vec![0u8, ty as u8, field as u8],
    }
}

/// Serializes `fields` (each `(sfXxx code, value bytes)`) as a root
/// object's canonical field sequence: sorted by field code, VL-prefixed
/// where the type calls for it, no wrapping header or terminator.
fn sto(fields: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut sorted = fields.to_vec();
    sorted.sort_by_key(|(code, _)| *code);
    let mut out = Vec::new();
    for (code, value) in sorted {
        out.extend_from_slice(&header(code));
        if matches!(code >> 16, STI_VL | STI_ACCOUNT) {
            assert!(
                value.len() < 193,
                "fixture VL fields stay in the 1-byte form"
            );
            out.push(value.len() as u8);
        }
        out.extend_from_slice(&value);
    }
    out
}

/// A `RippleState`-shaped ledger object: every `soeREQUIRED` field of the
/// format, `sfLowNode` as a present optional, and nothing else — so the
/// absent-optional assertions below have something real to be absent from.
fn ripple_state_bytes() -> Vec<u8> {
    sto(&[
        (
            sfLedgerEntryType.code(),
            rshooks::raw::ltRIPPLE_STATE.to_be_bytes().to_vec(),
        ),
        (sfFlags.code(), 0x0001_0000u32.to_be_bytes().to_vec()),
        (sfBalance.code(), encode_native_amount_const(7_000).to_vec()),
        (sfLowLimit.code(), encode_native_amount_const(1).to_vec()),
        (sfHighLimit.code(), encode_native_amount_const(2).to_vec()),
        (sfPreviousTxnID.code(), vec![0xAB; 32]),
        (sfPreviousTxnLgrSeq.code(), 42u32.to_be_bytes().to_vec()),
        (sfLowNode.code(), 9u64.to_be_bytes().to_vec()),
    ])
}

/// The same shape but tagged `ltACCOUNT_ROOT`, for the wrong-type
/// constructor assertions.
fn mistyped_bytes() -> Vec<u8> {
    sto(&[
        (
            sfLedgerEntryType.code(),
            rshooks::raw::ltACCOUNT_ROOT.to_be_bytes().to_vec(),
        ),
        (sfFlags.code(), 0u32.to_be_bytes().to_vec()),
    ])
}

/// An `sfEmitDetails` **value**: the inner object's own field sequence plus
/// its `0xE1` object terminator, which is part of that value's
/// serialization (`crate::host::slots`' module doc comment). `sfEmitCallback`
/// is deliberately left out — it is the format's one optional field.
fn emit_details_value() -> Vec<u8> {
    let mut v = sto(&[
        (sfEmitGeneration.code(), 3u32.to_be_bytes().to_vec()),
        (sfEmitBurden.code(), 11u64.to_be_bytes().to_vec()),
        (sfEmitParentTxnID.code(), vec![0x11; 32]),
        (sfEmitNonce.code(), vec![0x22; 32]),
        (sfEmitHookHash.code(), vec![0x33; 32]),
    ]);
    v.push(0xE1);
    v
}

const KEYLET: [u8; 34] = [0x77; 34];

fn keylet() -> Keylet {
    Keylet::from(KEYLET)
}

const SENDER: [u8; 20] = [0x0A; 20];
const DEST: [u8; 20] = [0x0B; 20];

// ---------------------------------------------------------------------
// The chain under test
// ---------------------------------------------------------------------

#[hooks]
pub struct Views;

#[hooks]
impl Views {
    /// `tx::Payment` over [`OtxnSource`]: presence rules and the type check.
    #[hook(0, on = [Invoke])]
    fn payment_from_otxn(&self) -> HookResult {
        let p = tx::Payment::otxn().expect("the otxn is a Payment");

        // soeREQUIRED, present.
        assert_eq!(p.destination().unwrap(), AccountId(DEST));
        assert_eq!(p.account().unwrap(), AccountId(SENDER));
        assert_eq!(
            p.amount().unwrap(),
            AmountBytes::Native(NativeAmount(encode_native_amount_const(1_000_000)))
        );

        // soeOPTIONAL, present — and read back through the host's as-int64
        // path, so the value has survived a big-endian round trip.
        assert_eq!(p.destination_tag().unwrap(), Some(1_234u32));

        // soeOPTIONAL, absent: `Ok(None)`, never an error.
        assert_eq!(p.invoice_id().unwrap(), None);
        assert_eq!(p.source_tag().unwrap(), None);

        // soeDEFAULT, absent: also `Ok(None)` — and a raw accessor, since
        // `PathSet` has no modeled value type.
        let mut buf = [0u8; 64];
        assert_eq!(p.paths_into(&mut buf).unwrap(), None);

        // A raw accessor over a present field returns the byte count.
        assert_eq!(p.signing_pub_key_into(&mut buf).unwrap(), 33);

        // The type check is a code compare, not an enum decode.
        assert_eq!(
            tx::EscrowCreate::otxn().err(),
            Some(HookError::DoesNotMatch)
        );

        accept!(b"ok", 0)
    }

    /// The same view over an originating transaction that is missing a
    /// `soeREQUIRED` field.
    #[hook(1, on = [Invoke])]
    fn required_field_absent(&self) -> HookResult {
        let p = tx::Payment::otxn().expect("the otxn is a Payment");
        assert_eq!(p.destination().err(), Some(HookError::DoesntExist));
        accept!(b"ok", 0)
    }

    /// `tx::Payment` over [`SlotSource`], plus the inner-object hop and the
    /// slot-lifetime guarantee.
    #[hook(2, on = [Invoke])]
    fn payment_from_slot(&self) -> HookResult {
        let root = SlotObject::from_otxn().expect("otxn_slot");
        let p = tx::Payment::from_slot(root).expect("sfTransactionType is ttPAYMENT");

        assert_eq!(p.destination().unwrap(), AccountId(DEST));
        assert_eq!(p.destination_tag().unwrap(), Some(1_234u32));
        assert_eq!(p.invoice_id().unwrap(), None);
        assert_eq!(p.transaction_type().unwrap(), rshooks::raw::ttPAYMENT);

        // 400 reads through a source that owns one slot. Without the
        // get→read→clear policy this exhausts the 255-slot budget around
        // iteration 255 and every read after that fails.
        for _ in 0..400 {
            assert_eq!(p.destination_tag().unwrap(), Some(1_234u32));
        }
        // Still room to load another root object, which is the observable
        // consequence: nothing leaked.
        SlotObject::from_otxn()
            .expect("slots are still available")
            .clear()
            .unwrap();

        // An STObject field: raw bytes on any source, a child slot here.
        let mut buf = [0u8; 256];
        let raw_len = p.emit_details_into(&mut buf).unwrap().expect("present");
        assert!(raw_len > 0);

        let child = p
            .emit_details_slot()
            .unwrap()
            .expect("sfEmitDetails is present");
        let details = inner::EmitDetails::from_slot(child);
        assert_eq!(details.emit_generation().unwrap(), 3);
        assert_eq!(details.emit_burden().unwrap(), 11);
        assert_eq!(details.emit_parent_txn_id().unwrap(), Hash([0x11; 32]));
        // The format's one optional field, absent from the fixture.
        assert_eq!(details.emit_callback().unwrap(), None);
        details.into_slot().clear().unwrap();

        // The escape hatch hands the root back intact.
        let back = p.into_slot();
        assert!(back.size().unwrap() > 0);
        back.clear().unwrap();

        accept!(b"ok", 0)
    }

    /// `tx::Payment::from_slot` refuses a slot holding another type, and
    /// does not keep the slot it refused.
    #[hook(3, on = [Invoke])]
    fn slot_constructor_rejects_the_wrong_type(&self) -> HookResult {
        let root = SlotObject::from_otxn().expect("otxn_slot");
        assert_eq!(
            tx::Payment::from_slot(root).err(),
            Some(HookError::DoesNotMatch)
        );
        // A rejected view costs no slot: the object still loads 255 more
        // times than a leak would allow.
        for _ in 0..300 {
            let obj = SlotObject::from_otxn().expect("otxn_slot");
            assert_eq!(
                tx::Payment::from_slot(obj).err(),
                Some(HookError::DoesNotMatch)
            );
        }
        accept!(b"ok", 0)
    }

    /// `ledger::RippleState`: both constructors, the presence rules, and
    /// the type check.
    #[hook(4, on = [Invoke])]
    fn ripple_state_view(&self) -> HookResult {
        let rs = ledger::RippleState::from_keylet(&keylet()).expect("seeded RippleState");

        // soeREQUIRED, present.
        assert_eq!(
            rs.balance().unwrap(),
            AmountBytes::Native(NativeAmount(encode_native_amount_const(7_000)))
        );
        assert_eq!(rs.previous_txn_lgr_seq().unwrap(), 42);
        assert_eq!(rs.previous_txn_id().unwrap(), Hash([0xAB; 32]));
        assert_eq!(
            rs.ledger_entry_type().unwrap(),
            rshooks::raw::ltRIPPLE_STATE
        );
        assert_eq!(rs.flags().unwrap(), 0x0001_0000);

        // soeOPTIONAL, present and absent.
        assert_eq!(rs.low_node().unwrap(), Some(9u64));
        assert_eq!(rs.high_node().unwrap(), None);
        assert_eq!(rs.low_quality_in().unwrap(), None);
        // An optional STObject field: the `*_slot` accessor reports absence
        // the same way a value accessor does.
        assert!(rs.high_reward_slot().unwrap().is_none());

        rs.into_slot().clear().unwrap();

        // from_slot is the same check, one step lower.
        let obj = SlotObject::from_keylet(&keylet()).expect("slot_set");
        let rs = ledger::RippleState::from_slot(obj).expect("ltRIPPLE_STATE");
        assert_eq!(rs.low_node().unwrap(), Some(9u64));
        rs.into_slot().clear().unwrap();

        accept!(b"ok", 0)
    }

    /// A ledger view refuses an object of another type.
    #[hook(5, on = [Invoke])]
    fn ledger_constructor_rejects_the_wrong_type(&self) -> HookResult {
        assert_eq!(
            ledger::RippleState::from_keylet(&keylet()).err(),
            Some(HookError::DoesNotMatch)
        );
        // The matching view accepts the very same object.
        let ar = ledger::AccountRoot::from_keylet(&keylet()).expect("ltACCOUNT_ROOT");
        ar.into_slot().clear().unwrap();
        accept!(b"ok", 0)
    }
}

// ---------------------------------------------------------------------
// Environments
// ---------------------------------------------------------------------

fn payment_otxn() -> Otxn {
    Otxn::new(TxType::Payment)
        .account(SENDER)
        .destination(DEST)
        .amount_drops(1_000_000)
        .field_raw(sfDestinationTag.code(), &1_234u32.to_be_bytes())
        .field_raw(sfSigningPubKey.code(), &[0u8; 33])
        .field_raw(sfEmitDetails.code(), &emit_details_value())
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20]).otxn(payment_otxn())
}

#[test]
fn otxn_backed_payment_view_reads_every_presence_kind() {
    let exit = env().invoke::<Views>(0);
    assert!(exit.is_success(), "{exit:?}");
}

#[test]
fn a_missing_required_field_is_an_error_not_a_default() {
    let env = TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Payment).account(SENDER));
    let exit = env.invoke::<Views>(1);
    assert!(exit.is_success(), "{exit:?}");
}

#[test]
fn slot_backed_payment_view_reads_and_releases_every_child_slot() {
    let exit = env().invoke::<Views>(2);
    assert!(exit.is_success(), "{exit:?}");
}

#[test]
fn a_slot_constructor_rejects_a_transaction_of_another_type() {
    let env = TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::EscrowCreate).account(SENDER));
    let exit = env.invoke::<Views>(3);
    assert!(exit.is_success(), "{exit:?}");
}

#[test]
fn ledger_view_reads_a_seeded_ripple_state() {
    let env = env().ledger_object(KEYLET, &ripple_state_bytes());
    let exit = env.invoke::<Views>(4);
    assert!(exit.is_success(), "{exit:?}");
}

#[test]
fn a_ledger_constructor_rejects_an_object_of_another_type() {
    let env = env().ledger_object(KEYLET, &mistyped_bytes());
    let exit = env.invoke::<Views>(5);
    assert!(exit.is_success(), "{exit:?}");
}
