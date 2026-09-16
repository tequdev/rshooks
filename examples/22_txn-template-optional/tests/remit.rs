//! Off-chain unit tests for `TxnTemplateOptional`'s `remit` entry
//! (`#[hook(0, ..)]`, `Remit`) — driven through `TestEnv::invoke`
//! against the real chain, no wasm build, no node. `env.emitted()`
//! blobs are canonical (re-serialized the way a real ledger stores
//! them): every `0x99` NOP — an absent optional field's whole slot, or
//! the unused tail of a partially-filled one — is stripped. Absence is
//! asserted by byte-count delta against the present-state blob, not by
//! searching for the header's *absence*: a short header can
//! coincidentally match part of a longer, unrelated one elsewhere in
//! the blob (`EmitDetails`, say). See `src/lib.rs`'s in-crate
//! `#[cfg(test)]` module for the raw, NOP-padded pre-emit layout
//! (`Remit` is private, so that's the only place it's reachable).

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
fn accepts_and_emits_one_remit() {
    let env = env();
    let exit = env.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
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
/// canonical emitted blob by default — the blob is exactly `header + 4`
/// bytes shorter than one with it present — and present with the exact
/// big-endian value once `DTAG` is supplied.
#[test]
fn destination_tag_absent_by_default_present_when_supplied() {
    let (hdr, hdr_len) = codec::field_header(sfDestinationTag);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let absent_len = emitted_absent[0].blob().len();

    let present = env().hook_param(b"DTAG", &7u32.to_be_bytes());
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

/// `amounts` (`sfAmounts [ sfAmountEntry { .. }, optional sfAmountEntry
/// { .. } ]`, unnamed elements numbered `0`/`1` by position) always
/// carries `amounts.0`: exactly one element with `AMT2` absent, exactly
/// two once it enables `amounts.1`. Counted by the array's own `0xE1`
/// element terminators between `sfAmounts`'s header and its closing
/// `0xF1` (`AmountEntry` has no nested `object`/`array` of its own, so
/// every `0xE1` in that span is one element's terminator, not a false
/// match from an unrelated field elsewhere in the blob).
#[test]
fn amounts_has_one_element_absent_two_present() {
    let (amounts_hdr, amounts_hdr_len) = codec::field_header(sfAmounts);
    let (amount_hdr, amount_hdr_len) = codec::field_header(sfAmount);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let blob = emitted_absent[0].blob();
    assert_eq!(
        count_amounts_elements(blob, &amounts_hdr[..amounts_hdr_len]),
        1,
        "{blob:02x?}"
    );
    // `amounts.0` defaults to the native 1-drop amount (`AMT1` absent).
    let mut expected_first = Vec::new();
    expected_first.extend_from_slice(&amount_hdr[..amount_hdr_len]);
    let mut native_value = 1u64.to_be_bytes();
    native_value[0] |= 0x40;
    expected_first.extend_from_slice(&native_value);
    assert!(
        blob.windows(expected_first.len())
            .any(|w| w == expected_first.as_slice()),
        "amounts.0's native 1-drop amount not found: {blob:02x?}"
    );

    let present = env().hook_param(b"AMT2", &5u64.to_be_bytes());
    let exit = present.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_present = present.emitted();
    let blob = emitted_present[0].blob();
    assert_eq!(
        count_amounts_elements(blob, &amounts_hdr[..amounts_hdr_len]),
        2,
        "{blob:02x?}"
    );
}

/// With `ISSUER` present, both element `0` and (once `AMT2` enables it)
/// element `1` switch to the 48-byte issued form (`USD`, the supplied
/// issuer) instead of the native form: the whole 48-byte value region
/// fills exactly (no leftover `NOP`s, unlike the native form's 40
/// trailing ones), and the currency/issuer bytes match exactly.
#[test]
fn issuer_present_writes_both_amounts_issued() {
    let (amount_hdr, amount_hdr_len) = codec::field_header(sfAmount);
    let issuer = [9u8; 20];
    let mut currency = [0u8; 20];
    currency[12..15].copy_from_slice(b"USD");

    let env = env()
        .hook_param(b"AMT2", &5u64.to_be_bytes())
        .hook_param(b"ISSUER", &issuer);
    let exit = env.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    let blob = emitted[0].blob();

    let mut expected_tail = Vec::new();
    expected_tail.extend_from_slice(&currency);
    expected_tail.extend_from_slice(&issuer);
    // Both `amounts.0` and `amounts.1` are the issued form: currency/
    // issuer (the value's own encoding is exercised byte-exactly by
    // `crates/rshooks/src/txn.rs`'s `encode_iou_amount_value_const`
    // tests) appear twice, immediately after an `sfAmount` header, with
    // no `0x99` anywhere in either 48-byte region.
    let occurrences = blob
        .windows(amount_hdr_len + 48)
        .filter(|w| w.starts_with(&amount_hdr[..amount_hdr_len]) && w.ends_with(&expected_tail))
        .count();
    assert_eq!(
        occurrences, 2,
        "expected both amounts.0 and amounts.1 issued (USD, the supplied issuer): {blob:02x?}"
    );
}

/// `AMT1 = 0` and `AMT2 = 0` both roll back (Remit rejects a zero
/// amount) rather than emit.
#[test]
fn zero_amount_rolls_back_without_emitting() {
    let amt1_zero = env().hook_param(b"AMT1", &0u64.to_be_bytes());
    let exit = amt1_zero.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(amt1_zero.emitted().len(), 0);

    let amt2_zero = env().hook_param(b"AMT2", &0u64.to_be_bytes());
    let exit = amt2_zero.invoke::<TxnTemplateOptional>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(amt2_zero.emitted().len(), 0);
}

/// Counts `0xE1` (`STObject` terminator) bytes between `sfAmounts`'s own
/// header and the array's closing `0xF1` — one per element.
fn count_amounts_elements(blob: &[u8], amounts_hdr: &[u8]) -> usize {
    let start = blob
        .windows(amounts_hdr.len())
        .position(|w| w == amounts_hdr)
        .expect("sfAmounts header present")
        + amounts_hdr.len();
    blob[start..]
        .iter()
        .take_while(|&&b| b != 0xF1)
        .filter(|&&b| b == 0xE1)
        .count()
}
