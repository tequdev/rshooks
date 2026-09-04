//! Off-chain unit tests for the `txn-template-nested` example, driven
//! through `TestEnv::invoke` against the real `TxnTemplateNested` chain —
//! no wasm build, no node. `src/lib.rs` carries an equivalent in-crate
//! `#[cfg(test)]` variant, which additionally covers the private `Remit`
//! template directly — byte-exact `sfAmounts` region and a `StoWriter`
//! cross-check — since those are only reachable from an in-crate test.

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

/// `ISSUER` (a 20-byte `AccountId` hook parameter) overrides the issued
/// entry's baked issuer at runtime, through the full 48-byte
/// `set_amounts_usd_amount` setter — the currency stays the baked `USD`.
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
    let mut expected = Vec::new();
    expected.extend_from_slice(&entry_hdr[..entry_hdr_len]);
    expected.extend_from_slice(&amount_hdr[..amount_hdr_len]);
    // XFL::one()'s issued STAmount value bytes (rshooks/src/txn.rs's
    // `encode_iou_amount_value_const_one` test derives the same vector by
    // hand from the XFL bit layout).
    expected.extend_from_slice(&[0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]);
    let mut currency = [0u8; 20];
    currency[12..15].copy_from_slice(b"USD");
    expected.extend_from_slice(&currency);
    expected.extend_from_slice(&[4u8; 20]); // the overridden issuer
    expected.push(0xE1); // object end marker
    assert!(
        blob.windows(expected.len())
            .any(|w| w == expected.as_slice()),
        "issued sfAmountEntry with the overridden issuer not found: {blob:02x?}"
    );
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
