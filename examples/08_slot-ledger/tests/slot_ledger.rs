//! Off-chain unit tests for the `slot-ledger` example, driven through
//! `TestEnv::invoke` against the real `SlotLedger` chain — no wasm build,
//! no node. Exercises `SlotObject::from_otxn`/`slot_subfield`/`slot_size`/
//! `raw_exact` through `rshooks-testenv`'s P2-D slot family
//! (`.claude/design/TESTENV_PHASE2_DESIGN.md` §4/§7 "slot family").

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks_testenv::prelude::*;
use slot_ledger::{SlotLedger, SlotLedgerError};

/// The wire-encoded first byte of a positive native amount, 0..=999,999
/// drops: `0x40 | ((drops >> 56) & 0x3F)` — `0x40` for anything under
/// `2^56` drops, which every amount seeded below is.
const NATIVE_AMOUNT_FIRST_BYTE: u16 = 0x40;

fn env(destination: [u8; 20], drops: u64) -> TestEnv {
    TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .destination(destination)
            .amount_drops(drops),
    )
}

#[test]
fn accepts_and_computes_the_marker_from_destination_and_amount() {
    let destination = [9u8; 20];
    let exit = env(destination, 1_000_000).invoke::<SlotLedger>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    let expected = u16::from(destination[0]).wrapping_add(NATIVE_AMOUNT_FIRST_BYTE);
    assert_eq!(exit.code, i64::from(expected));
    assert_eq!(exit.msg, b"slot-ledger: read Destination and native Amount");
}

#[test]
fn marker_wraps_and_reflects_a_different_destination_byte() {
    let destination = [0xFFu8; 20];
    let exit = env(destination, 0).invoke::<SlotLedger>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    let expected = u16::from(0xFFu8).wrapping_add(NATIVE_AMOUNT_FIRST_BYTE);
    assert_eq!(exit.code, i64::from(expected));
}

#[test]
fn rolls_back_when_the_otxn_has_no_destination_field() {
    let e = TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .amount_drops(1),
    );
    let exit = e.invoke::<SlotLedger>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, SlotLedgerError::NoDestinationField.code());
}

#[test]
fn rolls_back_when_destination_is_the_wrong_size() {
    let e = TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .field_raw(rshooks::sfield::sfDestination.code(), &[1, 2, 3])
            .amount_drops(1),
    );
    let exit = e.invoke::<SlotLedger>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, SlotLedgerError::UnexpectedDestinationSize.code());
}

#[test]
fn rolls_back_when_the_otxn_has_no_amount_field() {
    let e = TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .destination([9u8; 20]),
    );
    let exit = e.invoke::<SlotLedger>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, SlotLedgerError::NoAmountField.code());
}

#[test]
fn rolls_back_on_a_non_native_amount() {
    // A 48-byte IOU-shaped Amount: byte 0's top bit set marks it non-native
    // on the wire, which is what makes `otxn_slot`'s canonical
    // serialization self-consistent (the amount's own wire framing decides
    // how many bytes it occupies within the field sequence) — an all-zero
    // 48-byte value would misparse as an 8-byte native amount instead,
    // corrupting every field after it.
    let mut iou_amount = [0u8; 48];
    iou_amount[0] = 0x80;
    let e = TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .destination([9u8; 20])
            .field_raw(rshooks::sfield::sfAmount.code(), &iou_amount),
    );
    let exit = e.invoke::<SlotLedger>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, SlotLedgerError::UnsupportedAmount.code());
}

#[test]
fn accept_leaves_no_trace_side_effects() {
    let e = env([9u8; 20], 1);
    let _ = e.invoke::<SlotLedger>(0);
    assert!(e.traces().is_empty());
}
