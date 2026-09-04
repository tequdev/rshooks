//! Off-chain unit tests for the `txn-template-nested` example, driven
//! through `TestEnv::invoke` against the real `TxnTemplateNested` chain —
//! no wasm build, no node. `src/lib.rs` carries an equivalent in-crate
//! `#[cfg(test)]` variant, which additionally cross-checks the private
//! `Remit` template directly against `rshooks::sto_writer::StoWriter`,
//! since that's only reachable from an in-crate test.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks::prelude::*;
use rshooks_testenv::prelude::*;
use txn_template_nested::TxnTemplateNested;

const DEST: [u8; 20] = [3u8; 20];

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
        .hook_param(b"DEST", &DEST)
}

#[test]
fn accepts_and_emits_one_remit() {
    let env = env();
    let exit = env.invoke::<TxnTemplateNested>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
    assert!(!emitted[0].blob().is_empty());
}

#[test]
fn each_invocation_emits_its_own_remit() {
    let env = env();
    env.invoke::<TxnTemplateNested>(0);
    env.invoke::<TxnTemplateNested>(0);
    assert_eq!(env.emitted().len(), 2);
}

#[test]
fn missing_destination_rolls_back_without_emitting() {
    let env = TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]));
    let exit = env.invoke::<TxnTemplateNested>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(env.emitted().len(), 0);
}

/// Byte-exact check of the declared `sfAmounts` region: an `sfAmounts`
/// header around two `sfAmountEntry` images (`header(sfAmountEntry) 0x61
/// <8 value bytes> <USD> <USD_ISSUER> 0xE1` each), closed by the array end
/// marker — element 0 holds `XFL::new(0, 1)` (`1.0`), element 1 holds
/// `XFL::new(0, 2)` (`2.0`), both against the baked `USD`/`USD_ISSUER`
/// default. Headers are derived via `txn::codec::field_header` rather
/// than hardcoded; value bytes are hand-derived from the XFL bit layout
/// the way `rshooks/src/txn.rs`'s `encode_iou_amount_value_const_one`
/// test documents for `1.0` (element 1 follows the identical
/// normalization rule: mantissa `2_000_000_000_000_000`, exponent `-15`,
/// positive).
#[test]
fn amounts_region_matches_the_two_declared_issued_entries() {
    let env = env();
    let exit = env.invoke::<TxnTemplateNested>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    let blob = emitted[0].blob();

    let (amounts_hdr, amounts_hdr_len) = rshooks::txn::codec::field_header(sfAmounts);
    let (entry_hdr, entry_hdr_len) = rshooks::txn::codec::field_header(sfAmountEntry);
    let (amount_hdr, amount_hdr_len) = rshooks::txn::codec::field_header(sfAmount);

    let mut currency = [0u8; 20];
    currency[12..15].copy_from_slice(b"USD");

    let mut expected = Vec::new();
    expected.extend_from_slice(&amounts_hdr[..amounts_hdr_len]);
    for value in [
        [0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00], // XFL::new(0, 1) == 1.0
        [0xD4, 0x87, 0x1A, 0xFD, 0x49, 0x8D, 0x00, 0x00], // XFL::new(0, 2) == 2.0
    ] {
        expected.extend_from_slice(&entry_hdr[..entry_hdr_len]);
        expected.extend_from_slice(&amount_hdr[..amount_hdr_len]);
        expected.extend_from_slice(&value);
        expected.extend_from_slice(&currency);
        expected.extend_from_slice(&rshooks::account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh").0);
        expected.push(0xE1); // object end marker
    }
    expected.push(0xF1); // array end marker

    assert!(
        blob.windows(expected.len())
            .any(|w| w == expected.as_slice()),
        "sfAmounts region not found in the emitted blob: {blob:02x?}"
    );
}

/// `ISSUER` (a 20-byte `AccountId` hook parameter) overrides the baked
/// issuer at runtime, on **every** `sfAmounts` entry, through the full
/// 48-byte `set_amount` setter — the currency stays the baked `USD` on
/// both.
#[test]
fn issuer_hook_param_overrides_the_baked_issuer() {
    let env = env().hook_param(b"ISSUER", &[4u8; 20]);
    let exit = env.invoke::<TxnTemplateNested>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    let blob = emitted[0].blob();

    let (entry_hdr, entry_hdr_len) = rshooks::txn::codec::field_header(sfAmountEntry);
    let (amount_hdr, amount_hdr_len) = rshooks::txn::codec::field_header(sfAmount);
    let mut currency = [0u8; 20];
    currency[12..15].copy_from_slice(b"USD");

    for value in [
        [0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00], // XFL::new(0, 1) == 1.0
        [0xD4, 0x87, 0x1A, 0xFD, 0x49, 0x8D, 0x00, 0x00], // XFL::new(0, 2) == 2.0
    ] {
        let mut expected = Vec::new();
        expected.extend_from_slice(&entry_hdr[..entry_hdr_len]);
        expected.extend_from_slice(&amount_hdr[..amount_hdr_len]);
        expected.extend_from_slice(&value);
        expected.extend_from_slice(&currency);
        expected.extend_from_slice(&[4u8; 20]); // the overridden issuer
        expected.push(0xE1); // object end marker
        assert!(
            blob.windows(expected.len())
                .any(|w| w == expected.as_slice()),
            "issued sfAmountEntry with the overridden issuer not found: {blob:02x?}"
        );
    }
}

// `TxnTemplateNested`'s `#[cbak(0)]` body unconditionally accepts, so the
// only real behavior to assert is that `invoke_cbak` reaches `Accept`
// regardless of `CbakOutcome`.

#[test]
fn invoke_cbak_success_reaches_the_real_accept_path() {
    let env = env();
    let exit = env.invoke::<TxnTemplateNested>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<TxnTemplateNested>(0, CbakOutcome::Success(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}

#[test]
fn invoke_cbak_failure_still_accepts_because_the_cbak_body_ignores_the_outcome() {
    let env = env();
    let _ = env.invoke::<TxnTemplateNested>(0);
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<TxnTemplateNested>(0, CbakOutcome::Failure(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}
