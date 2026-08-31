//! The emission walker: field-sequence parsing shared by `emit`'s
//! acceptance check (design §5.6's normative grammar) and by
//! `crate::host::sto`/`crate::host::slots`' raw STObject/slot navigation.
//!
//! A structural serialized-field walk: fields in **any** order — real
//! xahaud's raw parse (`STObject::set`; every `sto_*` function goes through
//! `HookAPI::get_stobject_length`, `HookAPI.cpp:2888-3179`) decodes each
//! field's own header in turn, never requiring ascending `(type, field)`
//! order. `crate::host::sto`'s module doc cites `HookAPI::sto_validate`
//! (`HookAPI.cpp:68-96`) as having "no field-ordering or duplicate-field
//! check"; this walker (which `sto_validate` calls directly) matches that.
//! Structural rules enforced: canonical variable-length prefixes, correct
//! inner-object/array terminators, no trailing bytes, and a depth limit of
//! 2 (enough for `EmitDetails`/`Memos`, no general recursion).
//! [`validate_emit_blob`] layers additional rules on top for `emit`'s
//! acceptance grammar — see its own doc comment.
//!
//! What this walker does *not* check (documented, design §5.6): fee
//! sufficiency, ledger-window validity against real ledger progress, and
//! full STObject codec canonicality beyond the rules above — all e2e-only.
//!
//! # Order is tolerant everywhere; duplicate tolerance differs by real path
//!
//! Every caller below is order-tolerant, but real xahaud's duplicate
//! handling is *not* uniform across the two host-function families this
//! walker backs, so this module's tolerance isn't either:
//!
//! - [`validate_emit_blob`] (`crate::backend::Backend::emit`) and
//!   `crate::backend::Backend::prepare` both parse a hook-authored buffer
//!   through real xahaud's `STObject::set`-family deserialization — `emit`
//!   via `STTx(SerialIter)` -> `STObject::set` (`STObject.cpp:203`),
//!   `prepare` via `HookAPI::prepare`'s own `SerialIter`-based construction
//!   (`HookAPI.cpp:392-396`). `STObject::set` consumes fields in whatever
//!   order they appear, but afterward sorts every field by code and throws
//!   `"Duplicate field detected"` if any two share one
//!   (`STObject.cpp:266-276`). This walker itself stays order/
//!   duplicate-tolerant (shared with the `sto_*` family below);
//!   [`validate_emit_blob`] alone layers that duplicate rejection back on,
//!   scoped to top-level fields only — not recursed into nested
//!   objects/arrays the way `STObject::set` does at every depth.
//! - `crate::host::sto::sto_validate`/`sto_subfield`/`sto_subarray`: cited
//!   above and in `crate::host::sto`'s module doc (`HookAPI::sto_validate`,
//!   `HookAPI.cpp:68-96`, "no field-ordering or duplicate-field check";
//!   every other `sto_*` function shares the same underlying parser,
//!   `HookAPI::get_stobject_length`, `HookAPI.cpp:2888-3179`) — genuinely
//!   duplicate-tolerant on real xahaud, unlike the `STObject::set` family
//!   above.
//! - `crate::otxn::deserialize`/`from_emitted`: parses a blob that already
//!   passed [`validate_emit_blob`], so tolerates whatever order that
//!   already accepted.
//! - `crate::host::slots` (`slot_subfield`/`slot_set`/`otxn_slot`/
//!   `meta_slot`/`xpop_slot`): root slot content always comes from a real,
//!   already-canonically-serialized ledger object or transaction (see that
//!   module's "slot content = value payload" doc section), so this
//!   walker's order/duplicate tolerance is inert here.
//!
//! # P2-D extension: field/array navigation primitives
//!
//! [`FieldSpan`], [`walk_top_level_fields`]/[`walk_object_fields`],
//! [`walk_array_elements`] (per-element spans), and [`field_value_payload`]
//! (the **value-only** payload range a stored slot or `sto_subfield`
//! reports, VL length-prefix stripped for VL/AccountID fields) are
//! `crate::host::slots`/`crate::host::sto`'s shared foundation
//! (`.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "slot family", "sto_*").
//! See those modules' doc comments for the upstream citations behind the
//! per-type payload convention `field_value_payload` implements.

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
///
/// `value_range` already carries the walker's own inclusion rules: for an
/// `STI_OBJECT`(14)/`STI_ARRAY`(15) field it includes the nested
/// terminator (`0xE1`/`0xF1`); for every other type it is exactly the
/// header-excluded value bytes, VL length-prefix (for `STI_VL`(7)/
/// `STI_ACCOUNT`(8)) included. [`field_value_payload`] strips that VL
/// prefix where a "payload" (as opposed to raw wire value) is wanted — see
/// its own doc comment.
pub(crate) struct FieldSpan {
    pub(crate) code: u64,
    pub(crate) range: (usize, usize),
    pub(crate) value_range: (usize, usize),
}

/// Decodes one field header at `*pos`, advancing `*pos` past it. Mirrors
/// [`rshooks::txn::codec::field_header`]'s encoding rules in reverse (see
/// that function's doc comment for the four-case grammar).
pub(crate) fn decode_header(data: &[u8], pos: &mut usize) -> Result<(u32, u32), ()> {
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
pub(crate) fn decode_vl_len(data: &[u8], pos: &mut usize) -> Result<usize, ()> {
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
/// nested object) starting at `*pos` — real xahaud's raw STObject parse
/// reads each field sequentially by decoding its own header, independent
/// of field-code order or repetition (see this module's doc comment); this
/// walker does the same. For a nested object (`in_object == true`),
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
        dispatch_value(data, pos, ty, depth)?;
        fields.push(FieldSpan {
            code,
            range: (start, *pos),
            value_range: (value_start, *pos),
        });
    }
    Ok(fields)
}

pub(crate) fn walk_object_body(
    data: &[u8],
    pos: &mut usize,
    depth: u32,
) -> Result<Vec<FieldSpan>, ()> {
    walk_fields(data, pos, depth, true)
}

/// The STI_OBJECT type code — every STArray element's field header must
/// decode to this type (rippled's real STArray deserialization requires
/// each element to be an STObject field, e.g. `sfMemo`/`sfSigner`; nothing
/// else is a legal array element).
const STI_OBJECT: u32 = 14;

/// The STI_VL / STI_ACCOUNT type codes — [`field_value_payload`]'s two VL
/// length-prefix-stripped cases.
const STI_VL: u32 = 7;
const STI_ACCOUNT: u32 = 8;

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
        if ty != STI_OBJECT {
            return Err(());
        }
        dispatch_value(data, pos, ty, depth)?;
    }
}

/// Walks a **standalone** array body (element bytes ending in the
/// [`ARRAY_END_MARKER`], with no leading array-type header — the exact
/// shape a slot's/`sto_subarray`'s array-typed content has, per
/// `crate::host::slots`'/`crate::host::sto`'s module doc comments) and
/// returns each element's own `(start, end)` span — header and footer
/// (`0xE1`, since every element is itself an `STObject`) included: the
/// "fully formed" convention `sto_subarray`/`slot_subarray` both use.
/// Requires the whole buffer to parse as element spans with nothing left
/// over; any parse failure — including a buffer that doesn't end in
/// [`ARRAY_END_MARKER`] — is `Err(())`, matching [`walk_top_level_fields`]'s
/// full-consumption contract for the top-level case.
pub(crate) fn walk_array_elements(data: &[u8]) -> Result<Vec<(usize, usize)>, ()> {
    let mut pos = 0usize;
    let mut spans = Vec::new();
    loop {
        match data.get(pos) {
            Some(&ARRAY_END_MARKER) => {
                pos = pos.checked_add(1).ok_or(())?;
                return if pos == data.len() {
                    Ok(spans)
                } else {
                    Err(())
                };
            }
            None => return Err(()),
            _ => {}
        }
        let start = pos;
        let (ty, _field) = decode_header(data, &mut pos)?;
        if ty != STI_OBJECT {
            return Err(());
        }
        dispatch_value(data, &mut pos, ty, 0)?;
        spans.push((start, pos));
    }
}

/// Walks a top-level field sequence (a whole transaction, ledger object, or
/// root slot's content — no wrapping header or terminator), returning every
/// field found. Requires full consumption of `data`; any parse failure is
/// `Err(())`. `pub(crate)`-exported for `crate::host::slots`/
/// `crate::host::sto`'s navigation (P2-D) in addition to this module's own
/// [`validate_emit_blob`].
pub(crate) fn walk_top_level_fields(data: &[u8]) -> Result<Vec<FieldSpan>, ()> {
    walk_top_level(data)
}

/// [`walk_top_level_fields`] when `in_object` is `false`, or
/// [`walk_object_body`] (depth `0`, a fresh budget — see `crate::host::slots`'
/// module doc for why each slot's content is parsed with its own fresh
/// depth budget rather than one shared across slot hops) when `true`.
/// `crate::host::slots::slot_subfield`'s one call site: a
/// [`crate::invocation::SlotKind::Root`] parent has no wrapping terminator
/// (`in_object = false`); a [`crate::invocation::SlotKind::Object`] parent's
/// stored bytes already end in `0xE1` (`in_object = true`).
pub(crate) fn walk_top_level_fields_or_object(
    data: &[u8],
    in_object: bool,
) -> Result<Vec<FieldSpan>, ()> {
    if in_object {
        let mut pos = 0usize;
        walk_object_body(data, &mut pos, 0)
    } else {
        walk_top_level_fields(data)
    }
}

fn walk_top_level(data: &[u8]) -> Result<Vec<FieldSpan>, ()> {
    let mut pos = 0usize;
    walk_fields(data, &mut pos, 0, false)
}

fn field_bytes(data: &[u8], range: (usize, usize)) -> Option<&[u8]> {
    data.get(range.0..range.1)
}

/// The **value-only** payload range a stored slot / `sto_subfield` reports
/// for `field` within `data` — [`FieldSpan::value_range`] as-is for every
/// type except `STI_VL`(7)/`STI_ACCOUNT`(8), where the VL length-prefix
/// (present in `value_range`, since real wire bytes carry it) is stripped:
/// a slot's content is exactly what the host's `entry->add(s)` reports for
/// that field's value alone (`crate::host::slots`' module doc cites
/// `otxn_field`'s identically-shaped documented behavior — a `sfAccount`
/// field reads back as exactly its 20 raw bytes, matching
/// `examples/15_slot-objects`' e2e-pinned `check_account_walk`), and
/// `sto_subfield`'s "payload" convention strips the same prefix
/// (`HookAPI::get_stobject_length`'s `payload_start`/`payload_length` are
/// computed *after* decoding a VL type's own length prefix — see
/// `crate::host::sto`'s module doc for the citation).
///
/// Does **not** special-case `STI_ARRAY`(15) into the "fully formed"
/// (header-included) shape `sto_subfield` uses for arrays — the one
/// caller that needs that special-cases it itself using `field.range`
/// directly; every other caller wants the uniform value-only meaning this
/// function gives.
pub(crate) fn field_value_payload(data: &[u8], field: &FieldSpan) -> Result<(usize, usize), ()> {
    let ty = (field.code >> 16) as u32;
    if ty == STI_VL || ty == STI_ACCOUNT {
        let mut p = field.value_range.0;
        let len = decode_vl_len(data, &mut p)?;
        let end = p.checked_add(len).ok_or(())?;
        if end > field.value_range.1 {
            return Err(());
        }
        return Ok((p, end));
    }
    Ok(field.value_range)
}

/// Validates `blob` against the emission grammar. `expected_emit_details`
/// is the exact bytes this invocation's `etxn_details()` returned (`None`
/// if it was never called this invocation) — `blob`'s `EmitDetails` field
/// must match those bytes exactly, header and terminator included.
///
/// Rejects a repeated top-level field code — a direct citation of real
/// xahaud's `STObject::set`, which sorts every deserialized field by code
/// and throws `"Duplicate field detected"` if any two share one
/// (`STObject.cpp:266-276`; see this module's doc comment). The underlying
/// field walk itself (shared with `sto_validate`/`sto_subfield`, whose real
/// host implementation genuinely tolerates a repeat) stays permissive;
/// this rejection is layered on here, matching `emit`'s own
/// `STTx(SerialIter)` -> `STObject::set` parse path. Scoped to top-level
/// fields only — nested objects/arrays are not independently checked,
/// narrower than `STObject::set`'s real per-depth invariant.
pub(crate) fn validate_emit_blob(
    blob: &[u8],
    expected_emit_details: Option<&[u8]>,
) -> Result<(), ()> {
    let fields = walk_top_level(blob)?;

    for (i, f) in fields.iter().enumerate() {
        let earlier = fields.get(..i).ok_or(())?;
        if earlier.iter().any(|e| e.code == f.code) {
            return Err(());
        }
    }

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
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

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
    fn accepts_out_of_order_fields() {
        let d = details();
        let mut out = Vec::new();
        // Sequence (2,4) placed before TransactionType (1,2): outside
        // ascending (type, field) order, which real xahaud does not
        // require (see this module's doc comment).
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x68, 0x40, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x73, 0x00]);
        out.extend_from_slice(&d);
        assert!(validate_emit_blob(&out, Some(&d)).is_ok());
    }

    #[test]
    fn rejects_duplicate_field() {
        // Citing `STObject::set`'s real "Duplicate field detected" throw —
        // see `validate_emit_blob`'s doc comment; the underlying walk
        // itself tolerates a repeat, matching `sto_validate`/`sto_subfield`
        // (see `walk_top_level_fields_accepts_a_duplicate_field` below).
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
    fn rejects_a_non_adjacent_duplicate_field() {
        // The duplicate-Sequence check does not depend on adjacency: a
        // TransactionType field sits between the two Sequence occurrences.
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]); // Sequence
        out.extend_from_slice(&[0x12, 0x00, 0x00]); // TransactionType
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]); // duplicate Sequence
        out.extend_from_slice(&[0x68, 0x40, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&[0x73, 0x00]);
        out.extend_from_slice(&d);
        assert!(validate_emit_blob(&out, Some(&d)).is_err());
    }

    #[test]
    fn walk_top_level_fields_accepts_a_duplicate_field() {
        // The general field walk (shared with `sto_validate`/
        // `sto_subfield`) is more permissive than `validate_emit_blob`'s
        // own grammar — see this module's doc comment.
        let data: &[u8] = &[0x24, 0, 0, 0, 1, 0x24, 0, 0, 0, 2];
        let fields = walk_top_level_fields(data).unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].code, fields[1].code);
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
    fn accepts_an_array_field_of_object_elements() {
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0xFF); // array field: (type 15, field 15)
        blob.push(0xE2); // element header: (type 14, field 2) -- STObject
        blob.push(OBJECT_END_MARKER); // empty object body
        blob.push(ARRAY_END_MARKER);
        assert!(validate_emit_blob(&blob, Some(&d)).is_ok());
    }

    #[test]
    fn rejects_an_array_element_that_is_not_an_object() {
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0xFF); // array field: (type 15, field 15)
        blob.push(0x21); // element header: (type 2, field 1) -- UInt32, not STObject
        blob.extend_from_slice(&[0, 0, 0, 1]);
        blob.push(ARRAY_END_MARKER);
        assert!(validate_emit_blob(&blob, Some(&d)).is_err());
    }

    #[test]
    fn top_level_transaction_type_reads_back_payment() {
        let d = details();
        let blob = minimal_payment(&d);
        assert_eq!(top_level_transaction_type(&blob), Some(TxType::Payment));
    }

    // -- P2-D navigation primitives --

    #[test]
    fn walk_array_elements_returns_each_elements_full_span() {
        // Two empty-body STObject elements: (type 14, field 2) then
        // (type 14, field 3), each immediately closed, then the array
        // terminator.
        let data: &[u8] = &[
            0xE2,
            OBJECT_END_MARKER,
            0xE3,
            OBJECT_END_MARKER,
            ARRAY_END_MARKER,
        ];
        let spans = walk_array_elements(data).unwrap();
        assert_eq!(spans, vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn walk_array_elements_rejects_a_non_object_element() {
        let data: &[u8] = &[0x21, 0, 0, 0, 1, ARRAY_END_MARKER]; // UInt32, not STObject
        assert!(walk_array_elements(data).is_err());
    }

    #[test]
    fn walk_array_elements_rejects_trailing_bytes_after_the_terminator() {
        let data: &[u8] = &[0xE2, OBJECT_END_MARKER, ARRAY_END_MARKER, 0xFF];
        assert!(walk_array_elements(data).is_err());
    }

    #[test]
    fn walk_array_elements_rejects_a_missing_terminator() {
        let data: &[u8] = &[0xE2, OBJECT_END_MARKER];
        assert!(walk_array_elements(data).is_err());
    }

    #[test]
    fn field_value_payload_strips_vl_prefix_for_account_and_blob_fields() {
        // sfAccount (type 8, field 1) = 0x81, VL-prefixed 20-byte payload.
        let mut data = vec![0x81, 20];
        data.extend_from_slice(&[7u8; 20]);
        let fields = walk_top_level_fields(&data).unwrap();
        assert_eq!(fields.len(), 1);
        let (start, end) = field_value_payload(&data, &fields[0]).unwrap();
        assert_eq!((start, end), (2, 22));
        assert_eq!(data.get(start..end).unwrap(), &[7u8; 20]);
    }

    #[test]
    fn field_value_payload_leaves_amount_and_object_fields_untouched() {
        // sfFee (type 6, field 8) = 0x68, native amount, 8 raw bytes, no VL.
        let data: &[u8] = &[0x68, 0x40, 0, 0, 0, 0, 0, 0, 5];
        let fields = walk_top_level_fields(data).unwrap();
        let (start, end) = field_value_payload(data, &fields[0]).unwrap();
        assert_eq!((start, end), (1, 9));

        // A nested object field: (type 14, field 2) = 0xE2, empty body,
        // 0xE1 terminator — the terminator stays part of the "value".
        let obj: &[u8] = &[0xE2, OBJECT_END_MARKER];
        let of = walk_top_level_fields(obj).unwrap();
        let (ostart, oend) = field_value_payload(obj, &of[0]).unwrap();
        assert_eq!((ostart, oend), (1, 2));
    }

    #[test]
    fn walk_top_level_fields_or_object_dispatches_on_in_object() {
        let root: &[u8] = &[0x68, 0x40, 0, 0, 0, 0, 0, 0, 5]; // sfFee, no wrapping
        assert_eq!(
            walk_top_level_fields_or_object(root, false).unwrap().len(),
            1
        );
        let nested: &[u8] = &[0x24, 0, 0, 0, 1, OBJECT_END_MARKER]; // sfSequence then 0xE1
        assert_eq!(
            walk_top_level_fields_or_object(nested, true).unwrap().len(),
            1
        );
    }
}
