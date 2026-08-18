//! The emission walker: design §5.6's normative acceptance grammar for a
//! blob passed to `emit`.
//!
//! A canonical serialized-field walk: fields in canonical `(type, field)`
//! order (strictly increasing — this also rejects a duplicated field),
//! canonical variable-length prefixes, correct inner-object/array
//! terminators, no trailing bytes, and a depth limit of 2 (enough for
//! `EmitDetails`/`Memos`, no general recursion). Required invariants: a
//! known `TransactionType`, `Sequence == 0`, an empty `SigningPubKey`,
//! `Fee` present, and an `EmitDetails` field whose bytes are **exactly**
//! the bytes this invocation's `etxn_details()` returned.
//!
//! What this walker does *not* check (documented, design §5.6): fee
//! sufficiency, ledger-window validity against real ledger progress, and
//! full STObject codec canonicality beyond the rules above — all e2e-only.

use std::vec::Vec;

use rshooks::tx_type::TxType;

/// The STObject terminator (see [`crate::details::OBJECT_END_MARKER`]).
const OBJECT_END_MARKER: u8 = 0xE1;
/// The STArray terminator: `sfArrayEndMarker`'s wire byte (type 15, field 1,
/// both `< 16` → a single byte `(15 << 4) | 1`).
const ARRAY_END_MARKER: u8 = 0xF1;

const SF_TRANSACTION_TYPE: u64 = sfcode(1, 2);
const SF_SEQUENCE: u64 = sfcode(2, 4);
const SF_FEE: u64 = sfcode(6, 8);
const SF_SIGNING_PUB_KEY: u64 = sfcode(7, 3);
const SF_EMIT_DETAILS: u64 = sfcode(14, 13);

const fn sfcode(ty: u32, field: u32) -> u64 {
    ((ty as u64) << 16) | (field as u64)
}

/// One field found while walking an object (top-level or nested): its
/// `(type, field)` code, and two byte ranges into the original blob — the
/// whole field (header + value) and the value alone (after the header).
struct FieldSpan {
    code: u64,
    range: (usize, usize),
    value_range: (usize, usize),
}

/// Decodes one field header at `*pos`, advancing `*pos` past it. Mirrors
/// [`rshooks::txn::codec::field_header`]'s encoding rules in reverse (see
/// that function's doc comment for the four-case grammar).
fn decode_header(data: &[u8], pos: &mut usize) -> Result<(u32, u32), ()> {
    let b0 = *data.get(*pos).ok_or(())?;
    *pos = pos.checked_add(1).ok_or(())?;
    let high = b0 >> 4;
    let low = b0 & 0x0F;
    if high != 0 && low != 0 {
        Ok((u32::from(high), u32::from(low)))
    } else if high != 0 {
        let b1 = *data.get(*pos).ok_or(())?;
        *pos = pos.checked_add(1).ok_or(())?;
        Ok((u32::from(high), u32::from(b1)))
    } else if low != 0 {
        let b1 = *data.get(*pos).ok_or(())?;
        *pos = pos.checked_add(1).ok_or(())?;
        Ok((u32::from(b1), u32::from(low)))
    } else {
        let b1 = *data.get(*pos).ok_or(())?;
        *pos = pos.checked_add(1).ok_or(())?;
        let b2 = *data.get(*pos).ok_or(())?;
        *pos = pos.checked_add(1).ok_or(())?;
        Ok((u32::from(b1), u32::from(b2)))
    }
}

/// Decodes a canonical variable-length prefix at `*pos`, advancing `*pos`
/// past it, and returns the payload length it encodes. The 1/2/3-byte
/// ranges are mutually exclusive by construction (a length representable in
/// fewer bytes has no larger-byte encoding), so any value this accepts is
/// automatically the canonical encoding for that length.
fn decode_vl_len(data: &[u8], pos: &mut usize) -> Result<usize, ()> {
    let b0 = usize::from(*data.get(*pos).ok_or(())?);
    *pos = pos.checked_add(1).ok_or(())?;
    if b0 <= 192 {
        Ok(b0)
    } else if b0 <= 240 {
        let b1 = usize::from(*data.get(*pos).ok_or(())?);
        *pos = pos.checked_add(1).ok_or(())?;
        Ok(193usize
            .wrapping_add((b0.wrapping_sub(193)).wrapping_mul(256))
            .wrapping_add(b1))
    } else if b0 <= 254 {
        let b1 = usize::from(*data.get(*pos).ok_or(())?);
        *pos = pos.checked_add(1).ok_or(())?;
        let b2 = usize::from(*data.get(*pos).ok_or(())?);
        *pos = pos.checked_add(1).ok_or(())?;
        Ok(12481usize
            .wrapping_add((b0.wrapping_sub(241)).wrapping_mul(65536))
            .wrapping_add(b1.wrapping_mul(256))
            .wrapping_add(b2))
    } else {
        Err(())
    }
}

fn skip_fixed(data: &[u8], pos: &mut usize, len: usize) -> Result<(), ()> {
    let end = pos.checked_add(len).ok_or(())?;
    if end > data.len() {
        return Err(());
    }
    *pos = end;
    Ok(())
}

fn skip_amount(data: &[u8], pos: &mut usize) -> Result<(), ()> {
    let b0 = *data.get(*pos).ok_or(())?;
    let len = if b0 & 0x80 == 0 { 8 } else { 48 };
    skip_fixed(data, pos, len)
}

fn skip_vl(data: &[u8], pos: &mut usize) -> Result<(), ()> {
    let len = decode_vl_len(data, pos)?;
    skip_fixed(data, pos, len)
}

/// Fixed value length for STI types with no length prefix. Types this
/// harness does not model (arbitrary-width or protocol-specific types
/// beyond this Phase-1 subset) return `None`, rejecting the blob — a
/// documented walker limitation (design §5.6).
fn fixed_len_for_type(ty: u32) -> Option<usize> {
    match ty {
        1 => Some(2),   // UInt16
        2 => Some(4),   // UInt32
        3 => Some(8),   // UInt64
        4 => Some(16),  // Hash128
        5 => Some(32),  // Hash256
        16 => Some(1),  // UInt8
        17 => Some(20), // Hash160
        20 => Some(12), // UInt96
        21 => Some(24), // UInt192
        22 => Some(48), // UInt384
        23 => Some(64), // UInt512
        _ => None,
    }
}

fn dispatch_value(data: &[u8], pos: &mut usize, ty: u32, depth: u32) -> Result<(), ()> {
    match ty {
        6 => skip_amount(data, pos),
        7 | 8 => skip_vl(data, pos),
        14 => {
            if depth.checked_add(1).ok_or(())? > 2 {
                return Err(());
            }
            walk_object_body(data, pos, depth.wrapping_add(1)).map(|_| ())
        }
        15 => {
            if depth.checked_add(1).ok_or(())? > 2 {
                return Err(());
            }
            walk_array_body(data, pos, depth.wrapping_add(1))
        }
        other => {
            let len = fixed_len_for_type(other).ok_or(())?;
            skip_fixed(data, pos, len)
        }
    }
}

/// Walks one field sequence (a top-level transaction, or the inside of a
/// nested object) starting at `*pos`, in canonical strictly-increasing
/// `(type, field)` order. For a nested object (`in_object == true`),
/// consumes through the [`OBJECT_END_MARKER`]; for the top level
/// (`in_object == false`), consumes until `data.len()` — the "no trailing
/// bytes" rule is exactly this loop's termination condition.
fn walk_fields(
    data: &[u8],
    pos: &mut usize,
    depth: u32,
    in_object: bool,
) -> Result<Vec<FieldSpan>, ()> {
    let mut fields = Vec::new();
    let mut last_code: Option<u64> = None;
    loop {
        if in_object {
            match data.get(*pos) {
                Some(&OBJECT_END_MARKER) => {
                    *pos = pos.checked_add(1).ok_or(())?;
                    break;
                }
                None => return Err(()),
                _ => {}
            }
        } else if *pos >= data.len() {
            break;
        }
        let start = *pos;
        let (ty, field) = decode_header(data, pos)?;
        let value_start = *pos;
        let code = sfcode(ty, field);
        if let Some(last) = last_code {
            if code <= last {
                return Err(()); // out of order, or a duplicate field
            }
        }
        last_code = Some(code);
        dispatch_value(data, pos, ty, depth)?;
        fields.push(FieldSpan {
            code,
            range: (start, *pos),
            value_range: (value_start, *pos),
        });
    }
    Ok(fields)
}

fn walk_object_body(data: &[u8], pos: &mut usize, depth: u32) -> Result<Vec<FieldSpan>, ()> {
    walk_fields(data, pos, depth, true)
}

fn walk_array_body(data: &[u8], pos: &mut usize, depth: u32) -> Result<(), ()> {
    loop {
        match data.get(*pos) {
            Some(&ARRAY_END_MARKER) => {
                *pos = pos.checked_add(1).ok_or(())?;
                return Ok(());
            }
            None => return Err(()),
            _ => {}
        }
        let (ty, _field) = decode_header(data, pos)?;
        dispatch_value(data, pos, ty, depth)?;
    }
}

fn walk_top_level(data: &[u8]) -> Result<Vec<FieldSpan>, ()> {
    let mut pos = 0usize;
    walk_fields(data, &mut pos, 0, false)
}

fn field_bytes(data: &[u8], range: (usize, usize)) -> Option<&[u8]> {
    data.get(range.0..range.1)
}

/// Validates `blob` against the emission grammar. `expected_emit_details`
/// is the exact bytes this invocation's `etxn_details()` returned (`None`
/// if it was never called this invocation) — `blob`'s `EmitDetails` field
/// must match those bytes exactly, header and terminator included.
pub(crate) fn validate_emit_blob(
    blob: &[u8],
    expected_emit_details: Option<&[u8]>,
) -> Result<(), ()> {
    let fields = walk_top_level(blob)?;

    let tx_type_field = fields
        .iter()
        .find(|f| f.code == SF_TRANSACTION_TYPE)
        .ok_or(())?;
    let tx_type_bytes = field_bytes(blob, tx_type_field.value_range).ok_or(())?;
    let tx_type_arr: [u8; 2] = tx_type_bytes.try_into().map_err(|_| ())?;
    if matches!(
        TxType::from(u16::from_be_bytes(tx_type_arr)),
        TxType::Unknown(_)
    ) {
        return Err(());
    }

    let seq_field = fields.iter().find(|f| f.code == SF_SEQUENCE).ok_or(())?;
    let seq_bytes = field_bytes(blob, seq_field.value_range).ok_or(())?;
    if seq_bytes != [0u8, 0, 0, 0] {
        return Err(());
    }

    let spk_field = fields
        .iter()
        .find(|f| f.code == SF_SIGNING_PUB_KEY)
        .ok_or(())?;
    let mut spk_pos = spk_field.value_range.0;
    let spk_len = decode_vl_len(blob, &mut spk_pos)?;
    if spk_len != 0 {
        return Err(());
    }

    if !fields.iter().any(|f| f.code == SF_FEE) {
        return Err(());
    }

    let ed_field = fields
        .iter()
        .find(|f| f.code == SF_EMIT_DETAILS)
        .ok_or(())?;
    let ed_bytes = field_bytes(blob, ed_field.range).ok_or(())?;
    let expected = expected_emit_details.ok_or(())?;
    if ed_bytes != expected {
        return Err(());
    }

    Ok(())
}

/// The `TransactionType` field of a blob that has already passed
/// [`validate_emit_blob`] — used by [`crate::world::EmittedTxn::tx_type`].
/// `None` only if the blob is malformed in a way the walker should already
/// have rejected (defensive).
pub(crate) fn top_level_transaction_type(blob: &[u8]) -> Option<TxType> {
    let fields = walk_top_level(blob).ok()?;
    let field = fields.iter().find(|f| f.code == SF_TRANSACTION_TYPE)?;
    let bytes = field_bytes(blob, field.value_range)?;
    let arr: [u8; 2] = bytes.try_into().ok()?;
    Some(TxType::from(u16::from_be_bytes(arr)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::details::{EmitDetailsInputs, build_etxn_details};

    /// Builds a minimal, otherwise-valid emitted-Payment blob so each test
    /// can mutate exactly one thing and re-validate.
    fn minimal_payment(emit_details: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]); // TransactionType = 0 (Payment)
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]); // Sequence = 0
        out.extend_from_slice(&[0x61, 0x40, 0, 0, 0, 0, 0, 0, 1]); // Amount: native 1 drop
        out.extend_from_slice(&[0x68, 0x40, 0, 0, 0, 0, 0, 0, 0]); // Fee: native 0 drops
        out.extend_from_slice(&[0x73, 0x00]); // SigningPubKey: empty VL
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&[0u8; 20]); // Account
        out.extend_from_slice(emit_details);
        out
    }

    fn details() -> Vec<u8> {
        build_etxn_details(&EmitDetailsInputs {
            generation: 1,
            burden: 1,
            parent_txn_id: [1u8; 32],
            nonce: [2u8; 32],
            hook_hash: [3u8; 32],
            callback: None,
        })
    }

    #[test]
    fn accepts_a_well_formed_blob() {
        let d = details();
        let blob = minimal_payment(&d);
        assert!(validate_emit_blob(&blob, Some(&d)).is_ok());
    }

    #[test]
    fn rejects_missing_emit_details_expectation() {
        let d = details();
        let blob = minimal_payment(&d);
        assert!(validate_emit_blob(&blob, None).is_err());
    }

    #[test]
    fn rejects_emit_details_byte_mismatch() {
        let d = details();
        let other = build_etxn_details(&EmitDetailsInputs {
            generation: 2,
            burden: 1,
            parent_txn_id: [1u8; 32],
            nonce: [2u8; 32],
            hook_hash: [3u8; 32],
            callback: None,
        });
        let blob = minimal_payment(&d);
        assert!(validate_emit_blob(&blob, Some(&other)).is_err());
    }

    #[test]
    fn rejects_nonzero_sequence() {
        let d = details();
        let mut blob = minimal_payment(&d);
        if let Some(b) = blob.get_mut(7) {
            *b = 1;
        }
        assert!(validate_emit_blob(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_nonempty_signing_pub_key() {
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x68, 0x40, 0, 0, 0, 0, 0, 0, 0]);
        out.push(0x73);
        out.push(1);
        out.push(0xAB); // non-empty SigningPubKey
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&[0u8; 20]);
        out.extend_from_slice(&d);
        assert!(validate_emit_blob(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_missing_fee() {
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x73, 0x00]);
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&[0u8; 20]);
        out.extend_from_slice(&d);
        assert!(validate_emit_blob(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_unknown_transaction_type() {
        let d = details();
        let mut blob = minimal_payment(&d);
        // TransactionType value bytes are at offset 1..3 (after the 1-byte
        // header) — set to an out-of-range code.
        if let Some(b) = blob.get_mut(2) {
            *b = 0xFF;
        }
        assert!(validate_emit_blob(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_out_of_order_fields() {
        let d = details();
        let mut out = Vec::new();
        // Sequence (2,4) placed before TransactionType (1,2): violates
        // canonical increasing order.
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x68, 0x40, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x73, 0x00]);
        out.extend_from_slice(&d);
        assert!(validate_emit_blob(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_duplicate_field() {
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]); // duplicate Sequence
        out.extend_from_slice(&[0x68, 0x40, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x73, 0x00]);
        out.extend_from_slice(&d);
        assert!(validate_emit_blob(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_trailing_bytes() {
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0xFF);
        assert!(validate_emit_blob(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_depth_beyond_two() {
        // Three nested Object-typed (type 14) field headers in a row: an
        // outer field at top level (depth 0 -> opens depth 1), one nested
        // inside it (depth 1 -> opens depth 2), and one nested inside that
        // (depth 2 -> would open depth 3, over the limit). The depth check
        // fires as soon as the third header's value is dispatched, before
        // any terminator is needed.
        let depth_violation: &[u8] = &[
            0xEE, // top level: (type 14, field 14) -> opens depth 1
            0xE2, // depth 1:   (type 14, field 2)  -> opens depth 2
            0xE5, // depth 2:   (type 14, field 5)  -> would open depth 3
        ];

        let d = details();
        let mut blob = minimal_payment(&d);
        blob.extend_from_slice(depth_violation);
        assert!(validate_emit_blob(&blob, Some(&d)).is_err());
    }

    #[test]
    fn top_level_transaction_type_reads_back_payment() {
        let d = details();
        let blob = minimal_payment(&d);
        assert_eq!(top_level_transaction_type(&blob), Some(TxType::Payment));
    }
}
