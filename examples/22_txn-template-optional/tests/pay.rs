//! Off-chain unit tests for `TxnTemplateOptional`'s `tplpay` entry
//! (`#[hook(0, ..)]`, `OptionalPayment`) — driven through
//! `TestEnv::invoke` against the real chain, no wasm build, no node.
//! `env.emitted()` blobs are canonical (re-serialized the way a real
//! ledger stores them): every `0x99` NOP — an absent optional field's
//! whole slot, or the unused tail of a partially-filled one — is
//! stripped, so these tests assert the *decoded* functional behavior:
//! present with exactly the written bytes, absent by byte-count delta
//! against the present-state blob (not by searching for the header's
//! *absence* — a short header can coincidentally match part of a
//! longer, unrelated one elsewhere in the blob, e.g. `EmitDetails`). The
//! NOP-padding mechanism itself — the raw, pre-emit byte layout — is
//! pinned in `src/lib.rs`'s in-crate `#[cfg(test)]` module instead, the
//! only place reachable from (`OptionalPayment` is private).

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks::prelude::*;
use rshooks::txn::codec;
use rshooks_testenv::prelude::*;
use txn_template_optional::TxnTemplateOptional;

const DEST: [u8; 20] = [3u8; 20];

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
        .hook_param(b"DEST", &DEST)
}

#[test]
fn accepts_and_emits_one_payment() {
    let env = env();
    let exit = env.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Payment));
}

#[test]
fn missing_destination_rolls_back_without_emitting() {
    let env = TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]));
    let exit = env.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(env.emitted().len(), 0);
}

/// `destination_tag` (`optional sfDestinationTag`) is absent from the
/// canonical emitted blob by default — the blob is exactly `header +
/// 4` bytes shorter than one with it present, a byte-count check rather
/// than a "the header never appears" search, since a short header can
/// coincidentally match part of a longer, unrelated one elsewhere in
/// the blob (`EmitDetails`, say) — and present with the exact
/// big-endian value once `DEST_TAG` is supplied.
#[test]
fn destination_tag_absent_by_default_present_when_supplied() {
    let (hdr, hdr_len) = codec::field_header(sfDestinationTag);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let absent_len = emitted_absent[0].blob().len();

    let present = env().hook_param(b"DEST_TAG", &7u32.to_be_bytes());
    let exit = present.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_present = present.emitted();
    let blob = emitted_present[0].blob();
    let mut expected = Vec::new();
    expected.extend_from_slice(&hdr[..hdr_len]);
    expected.extend_from_slice(&7u32.to_be_bytes());
    assert!(
        blob.windows(expected.len())
            .any(|w| w == expected.as_slice()),
        "present DestinationTag field not found: {blob:02x?}"
    );
    assert_eq!(
        blob.len(),
        absent_len + hdr_len + 4,
        "the canonical blob must be exactly the field's own bytes longer with it present"
    );
}

/// `send_max` (`optional any_amount(sfSendMax)`) is absent from the
/// canonical blob by default, present in the native form (500 drops)
/// when `SEND_MAX` alone is supplied, and present in the issued form
/// (`USD`, `1.0`, the supplied issuer) when `ISSUER` is supplied
/// (`ISSUER` takes precedence over `SEND_MAX`) — no leftover NOP bytes
/// in the canonical blob in either present case.
#[test]
fn send_max_absent_by_default_native_or_issued_when_supplied() {
    let (hdr, hdr_len) = codec::field_header(sfSendMax);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let absent_len = emitted_absent[0].blob().len();

    let native = env().hook_param(b"SEND_MAX", &[1u8]);
    let exit = native.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_native = native.emitted();
    let blob = emitted_native[0].blob();
    let mut expected_native = Vec::new();
    expected_native.extend_from_slice(&hdr[..hdr_len]);
    let mut native_value = 500u64.to_be_bytes();
    native_value[0] |= 0x40;
    expected_native.extend_from_slice(&native_value);
    assert!(
        blob.windows(expected_native.len())
            .any(|w| w == expected_native.as_slice()),
        "native sfSendMax region not found: {blob:02x?}"
    );
    assert_eq!(blob.len(), absent_len + expected_native.len());

    let issuer = [9u8; 20];
    let issued = env().hook_param(b"ISSUER", &issuer);
    let exit = issued.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_issued = issued.emitted();
    let blob = emitted_issued[0].blob();
    let mut currency = [0u8; 20];
    currency[12..15].copy_from_slice(b"USD");
    let mut expected_issued = Vec::new();
    expected_issued.extend_from_slice(&hdr[..hdr_len]);
    expected_issued.extend_from_slice(&[0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]); // XFL::new(0, 1) == 1.0
    expected_issued.extend_from_slice(&currency);
    expected_issued.extend_from_slice(&issuer);
    assert!(
        blob.windows(expected_issued.len())
            .any(|w| w == expected_issued.as_slice()),
        "issued sfSendMax region not found: {blob:02x?}"
    );
    assert_eq!(blob.len(), absent_len + expected_issued.len());
}
