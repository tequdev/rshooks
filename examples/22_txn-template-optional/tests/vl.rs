//! Off-chain unit tests for `TxnTemplateOptional`'s `tplvl` entry
//! (`#[hook(2, ..)]`, `OptionalVl`) — driven through `TestEnv::invoke`
//! against the real chain, no wasm build, no node. `env.emitted()`
//! blobs are canonical (NOP-free), so a `vl` write at `MIN` (fewer than
//! `MAX` bytes) leaves no trailing padding to account for here — see
//! `src/lib.rs`'s in-crate `#[cfg(test)]` module for the raw,
//! NOP-padded pre-emit layout (`OptionalVl` is private, so that's the
//! only place it's reachable).

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
fn accepts_and_emits_one_vl_remit() {
    let env = env();
    let exit = env.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
}

/// `note` (`vl(sfBlob, 190, 194)`) defaults to its `MIN` length (190
/// bytes, a one-byte `VL` prefix) and, with `NOTE_LONG` present, writes
/// the full `MAX` (194 bytes, a two-byte prefix) — crossing the
/// 192/193-byte length-prefix-width boundary from one declaration.
#[test]
fn note_defaults_to_min_length_note_long_selects_max() {
    let (hdr, hdr_len) = codec::field_header(sfBlob);

    let short = env();
    let exit = short.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = short.emitted();
    let blob = emitted[0].blob();
    let mut expected_short = Vec::new();
    expected_short.extend_from_slice(&hdr[..hdr_len]);
    expected_short.push(190); // one-byte VL prefix, 190 <= 192
    expected_short.extend_from_slice(&[b'n'; 190]);
    assert!(
        blob.windows(expected_short.len())
            .any(|w| w == expected_short.as_slice()),
        "190-byte note (one-byte prefix) region not found: {blob:02x?}"
    );

    let long = env().hook_param(b"NOTE_LONG", &[1u8]);
    let exit = long.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = long.emitted();
    let blob = emitted[0].blob();
    let mut expected_long = Vec::new();
    expected_long.extend_from_slice(&hdr[..hdr_len]);
    // Two-byte VL prefix for 194 (193..=12480): `193 + ((194-193) >> 8)`,
    // `(194-193) & 0xFF`.
    expected_long.extend_from_slice(&[0xC1, 0x01]);
    expected_long.extend_from_slice(&[b'n'; 194]);
    assert!(
        blob.windows(expected_long.len())
            .any(|w| w == expected_long.as_slice()),
        "194-byte note (two-byte prefix) region not found: {blob:02x?}"
    );
}

/// `invoice_id` (`optional sfInvoiceID`) is absent from the canonical
/// blob by default and carries the supplied hash once `INVOICE` is
/// present.
#[test]
fn invoice_id_absent_by_default_present_when_supplied() {
    let (hdr, hdr_len) = codec::field_header(sfInvoiceID);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let absent_len = emitted_absent[0].blob().len();

    let invoice = [7u8; 32];
    let present = env().hook_param(b"INVOICE", &invoice);
    let exit = present.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_present = present.emitted();
    let blob = emitted_present[0].blob();
    let mut expected = Vec::new();
    expected.extend_from_slice(&hdr[..hdr_len]);
    expected.extend_from_slice(&invoice);
    assert!(
        blob.windows(expected.len())
            .any(|w| w == expected.as_slice()),
        "present InvoiceID field not found: {blob:02x?}"
    );
    assert_eq!(blob.len(), absent_len + expected.len());
}

/// `mint` (`optional Mint: sfMintURIToken { flags: optional sfFlags,
/// uri: fixed_vl(sfURI, 4) = *b"ipfs" }`) is absent from the canonical
/// blob by default and, once `MINT` enables it, carries the baked
/// `uri = "ipfs"` default plus the `flags = 1` `remit` always sets when
/// enabling it.
#[test]
fn mint_absent_by_default_present_when_supplied() {
    let (mint_hdr, mint_hdr_len) = codec::field_header(sfMintURIToken);
    let (flags_hdr, flags_hdr_len) = codec::field_header(sfFlags);
    let (uri_hdr, uri_hdr_len) = codec::field_header(sfURI);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let absent_len = emitted_absent[0].blob().len();

    let present = env().hook_param(b"MINT", &[1u8]);
    let exit = present.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_present = present.emitted();
    let blob = emitted_present[0].blob();
    let mut expected = Vec::new();
    expected.extend_from_slice(&mint_hdr[..mint_hdr_len]);
    expected.extend_from_slice(&flags_hdr[..flags_hdr_len]);
    expected.extend_from_slice(&1u32.to_be_bytes());
    expected.extend_from_slice(&uri_hdr[..uri_hdr_len]);
    expected.push(4); // fixed_vl(sfURI, 4)'s length prefix
    expected.extend_from_slice(b"ipfs");
    expected.push(0xE1); // object end marker
    assert!(
        blob.windows(expected.len())
            .any(|w| w == expected.as_slice()),
        "present mint region not found: {blob:02x?}"
    );
    assert_eq!(
        blob.len(),
        absent_len + expected.len(),
        "the canonical blob must be exactly the region's own bytes longer with it present"
    );
}

/// `amounts` (`sfAmounts [ Entry: optional sfAmountEntry { .. } ; 1 ]`,
/// a homogeneous array of `0` or `1` elements): no `0xE1` element
/// terminator between `sfAmounts`'s header and its closing `0xF1` with
/// `AMT_ENTRY` absent, exactly one once it enables the sole element —
/// the "`0` or `1` entries" case, contrasting `OptionalRemit::amounts`'s
/// named "one required, one `optional`" shape.
#[test]
fn amounts_has_zero_elements_absent_one_present() {
    let (amounts_hdr, amounts_hdr_len) = codec::field_header(sfAmounts);

    let absent = env();
    let exit = absent.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_absent = absent.emitted();
    let blob = emitted_absent[0].blob();
    assert_eq!(
        count_amounts_elements(blob, &amounts_hdr[..amounts_hdr_len]),
        0,
        "{blob:02x?}"
    );

    let present = env().hook_param(b"AMT_ENTRY", &[1u8]);
    let exit = present.invoke::<TxnTemplateOptional>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted_present = present.emitted();
    let blob = emitted_present[0].blob();
    assert_eq!(
        count_amounts_elements(blob, &amounts_hdr[..amounts_hdr_len]),
        1,
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
