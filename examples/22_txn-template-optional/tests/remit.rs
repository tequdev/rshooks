//! Off-chain unit tests for `TxnTemplateOptional`'s `tplremit` entry
//! (`#[hook(1, ..)]`, `OptionalRemit`) — driven through
//! `TestEnv::invoke` against the real chain, no wasm build, no node.
//! `env.emitted()` blobs are canonical (re-serialized the way a real
//! ledger stores them): every `0x99` NOP — an absent optional field's
//! whole slot, or the unused tail of a partially-filled one — is
//! stripped. Absence is asserted by byte-count delta against the
//! present-state blob, not by searching for the header's *absence*: a
//! short header can coincidentally match part of a longer, unrelated
//! one elsewhere in the blob (`EmitDetails`, say). See `src/lib.rs`'s
//! in-crate `#[cfg(test)]` module for the raw, NOP-padded pre-emit
//! layout (`OptionalRemit` is private, so that's the only place it's
//! reachable).

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
    let exit = env.invoke::<TxnTemplateOptional>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
}

/// `blob` (`optional vl(sfBlob, 2, 8)`) is absent from the canonical
/// blob by default and carries the 5-byte `"blob!"` payload once
/// `BLOB` is present.
#[test]
fn blob_absent_by_default_present_when_supplied() {
    let (hdr, hdr_len) = codec::field_header(sfBlob);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let absent_len = emitted_absent[0].blob().len();

    let present = env().hook_param(b"BLOB", &[1u8]);
    let exit = present.invoke::<TxnTemplateOptional>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_present = present.emitted();
    let blob = emitted_present[0].blob();
    let mut expected = Vec::new();
    expected.extend_from_slice(&hdr[..hdr_len]);
    expected.push(5); // one-byte VL prefix, 5 <= 192
    expected.extend_from_slice(b"blob!");
    assert!(
        blob.windows(expected.len())
            .any(|w| w == expected.as_slice()),
        "present blob field not found: {blob:02x?}"
    );
    assert_eq!(blob.len(), absent_len + expected.len());
}

/// `memos` (`optional Memos: array(sfMemos) [ .. ]`) is absent from the
/// canonical blob by default — a shorter blob than with it present, by
/// exactly the declared region's own byte count — and, once `MEMO` is
/// supplied, carries the one declared `sfMemo` element with its baked
/// `*b"note"` `memo_type`.
#[test]
fn memos_absent_by_default_present_when_supplied() {
    let (memos_hdr, memos_hdr_len) = codec::field_header(sfMemos);
    let (memo_hdr, memo_hdr_len) = codec::field_header(sfMemo);
    let (type_hdr, type_hdr_len) = codec::field_header(sfMemoType);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let absent_len = emitted_absent[0].blob().len();

    let present = env().hook_param(b"MEMO", &[1u8]);
    let exit = present.invoke::<TxnTemplateOptional>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_present = present.emitted();
    let blob = emitted_present[0].blob();
    let mut expected = Vec::new();
    expected.extend_from_slice(&memos_hdr[..memos_hdr_len]);
    expected.extend_from_slice(&memo_hdr[..memo_hdr_len]);
    expected.extend_from_slice(&type_hdr[..type_hdr_len]);
    expected.push(4); // fixed_vl(sfMemoType, 4)'s length prefix
    expected.extend_from_slice(b"note");
    expected.push(0xE1); // object end marker
    expected.push(0xF1); // array end marker
    assert!(
        blob.windows(expected.len())
            .any(|w| w == expected.as_slice()),
        "present memos region not found: {blob:02x?}"
    );
    assert_eq!(
        blob.len(),
        absent_len + expected.len(),
        "the canonical blob must be exactly the region's own bytes longer with it present"
    );
}

/// `amounts` (`sfAmounts [ first: sfAmountEntry { .. }, second: optional
/// Second: sfAmountEntry { .. } ]`) always carries `first` (`remit`
/// always writes a real, constructible 1-drop native amount into it):
/// exactly one element with `AMT_ENTRY` absent, exactly two once it
/// enables `second`. Counted by the array's own `0xE1` element
/// terminators between `sfAmounts`'s header and its closing `0xF1`
/// (`AmountEntry` has no nested `object`/`array` of its own, so every
/// `0xE1` in that span is one element's terminator, not a false match
/// from an unrelated field elsewhere in the blob).
#[test]
fn amounts_has_one_element_absent_two_present() {
    let (amounts_hdr, amounts_hdr_len) = codec::field_header(sfAmounts);
    let (amount_hdr, amount_hdr_len) = codec::field_header(sfAmount);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let blob = emitted_absent[0].blob();
    assert_eq!(
        count_amounts_elements(blob, &amounts_hdr[..amounts_hdr_len]),
        1,
        "{blob:02x?}"
    );
    // `first` carries the real 1-drop native amount `remit` always writes.
    let mut expected_first = Vec::new();
    expected_first.extend_from_slice(&amount_hdr[..amount_hdr_len]);
    let mut native_value = 1u64.to_be_bytes();
    native_value[0] |= 0x40;
    expected_first.extend_from_slice(&native_value);
    assert!(
        blob.windows(expected_first.len())
            .any(|w| w == expected_first.as_slice()),
        "first's native 1-drop amount not found: {blob:02x?}"
    );

    let present = env().hook_param(b"AMT_ENTRY", &[1u8]);
    let exit = present.invoke::<TxnTemplateOptional>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_present = present.emitted();
    let blob = emitted_present[0].blob();
    assert_eq!(
        count_amounts_elements(blob, &amounts_hdr[..amounts_hdr_len]),
        2,
        "{blob:02x?}"
    );
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
