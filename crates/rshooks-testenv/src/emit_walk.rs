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
//! inner-object/array terminators, no trailing bytes, and a recursion-depth
//! limit of [`STO_MAX_RECURSION_DEPTH`] matching real xahaud's
//! `get_stobject_length` (`HookAPI.cpp:2901`).
//! [`validate_emit_blob`] layers additional rules on top for `emit`'s
//! acceptance grammar — see its own doc comment.
//!
//! [`validate_emit_blob`] does check fee sufficiency and the
//! `FirstLedgerSequence`/`LastLedgerSequence` window (against this
//! harness's own `World` fee/ledger state, matching `HookAPI::emit`'s own
//! rules 5-7 — see that function's own doc comment). What it does *not*
//! check: full STObject codec canonicality beyond the structural rules
//! above, and anything `ripple::preflight` validates beyond field
//! presence (amount signs, flag combinations, currency/issuer validity,
//! and so on) — all e2e-only.
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
//!   recursed into every nested `STI_OBJECT` and `STI_ARRAY` element's own
//!   body — each object scope (top level, a nested object, one array
//!   element) has its own independent field-code set, matching
//!   `STObject::set`'s real per-depth invariant: the same field code may
//!   repeat across sibling array elements, or between a scope and its
//!   parent, but not twice within one scope.
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
const SF_FIRST_LEDGER_SEQUENCE: u64 = sfcode(2, 26);
const SF_LAST_LEDGER_SEQUENCE: u64 = sfcode(2, 27);
const SF_ACCOUNT_TXN_ID: u64 = sfcode(5, 9);
const SF_FEE: u64 = sfcode(6, 8);
const SF_SIGNING_PUB_KEY: u64 = sfcode(7, 3);
const SF_TXN_SIGNATURE: u64 = sfcode(7, 4);
const SF_ACCOUNT: u64 = sfcode(8, 1);
const SF_EMIT_DETAILS: u64 = sfcode(14, 13);
const SF_SIGNERS: u64 = sfcode(15, 3);
const SF_TICKET_SEQUENCE: u64 = sfcode(2, 41);

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

/// The recursion-depth bound real xahaud's `HookAPI::get_stobject_length`
/// enforces (`HookAPI.cpp:2901`: `if (recursion_depth > 10) return
/// Unexpected(pe_excessive_nesting);`, checked on entry before parsing that
/// level's field). This walker's `depth` parameter uses the identical
/// convention — top-level fields parse at `depth == 0`, and each
/// `STI_OBJECT`(14)/`STI_ARRAY`(15) recursion increments it by one before
/// parsing the nested body — so the same bound applies unmodified here.
const STO_MAX_RECURSION_DEPTH: u32 = 10;

fn dispatch_value(data: &[u8], pos: &mut usize, ty: u32, depth: u32) -> Result<(), ()> {
    match ty {
        6 => skip_amount(data, pos),
        7 | 8 => skip_vl(data, pos),
        14 => {
            if depth.checked_add(1).ok_or(())? > STO_MAX_RECURSION_DEPTH {
                return Err(());
            }
            walk_object_body(data, pos, depth.wrapping_add(1)).map(|_| ())
        }
        15 => {
            if depth.checked_add(1).ok_or(())? > STO_MAX_RECURSION_DEPTH {
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

/// The STI_ARRAY type code — [`reject_duplicate_fields`]'s array-element
/// recursion case.
const STI_ARRAY: u32 = 15;

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

/// Rejects a repeated field code within any single object scope — a direct
/// citation of real xahaud's `STObject::set`, which sorts every
/// deserialized field by code and throws `"Duplicate field detected"` if
/// any two share one (`STObject.cpp:266-276`; see this module's doc
/// comment), applied at every depth `STObject::set` itself recurses into.
/// `fields` is one scope's own field list (top level, a nested
/// `STI_OBJECT`'s body, or one `STI_ARRAY` element's body) and `data` is
/// the byte range that scope's spans index into; each recursive call gets
/// a fresh scope, so the same field code repeating across sibling array
/// elements, or between a scope and its parent, is left alone — only two
/// fields sharing a code within the same scope are rejected. The
/// underlying field walk itself (shared with `sto_validate`/`sto_subfield`,
/// whose real host implementation genuinely tolerates a repeat at every
/// depth) stays permissive; this rejection is layered on top, scoped to
/// [`validate_emit_blob`]'s callers only.
fn reject_duplicate_fields(fields: &[FieldSpan], data: &[u8]) -> Result<(), ()> {
    for (i, f) in fields.iter().enumerate() {
        let earlier = fields.get(..i).ok_or(())?;
        if earlier.iter().any(|e| e.code == f.code) {
            return Err(());
        }
    }
    for f in fields {
        let ty = (f.code >> 16) as u32;
        if ty == STI_OBJECT {
            let body = data.get(f.value_range.0..f.value_range.1).ok_or(())?;
            let mut pos = 0usize;
            let nested = walk_object_body(body, &mut pos, 0)?;
            reject_duplicate_fields(&nested, body)?;
        } else if ty == STI_ARRAY {
            let body = data.get(f.value_range.0..f.value_range.1).ok_or(())?;
            for (start, end) in walk_array_elements(body)? {
                let element = body.get(start..end).ok_or(())?;
                let mut pos = 0usize;
                decode_header(element, &mut pos)?;
                let nested = walk_object_body(element, &mut pos, 0)?;
                reject_duplicate_fields(&nested, element)?;
            }
        }
    }
    Ok(())
}

/// Validates `blob` against the emission grammar — a port of real xahaud's
/// `HookAPI::emit` acceptance checks (`Xahau/xahaud` `dev`,
/// `src/xrpld/app/hook/detail/HookAPI.cpp`, the `stpTrans->isFieldPresent`/
/// `getField*` block right after the `STTx(SerialIter)` parse, rules 0
/// through 8 per that function's own enumerating comment). `hook_account`
/// and `ledger_seq` are the invoking hook's account and the world's current
/// ledger sequence (rules 0, 5, 6); `min_fee` is this blob's
/// `etxn_fee_base` result (rule 7, computed by the caller since it needs
/// `Backend`'s own state); `expected_emit_details` is the exact bytes this
/// invocation's `etxn_details()` returned (`None` if it was never called
/// this invocation) — `blob`'s `EmitDetails` field must match those bytes
/// exactly, header and terminator included (a byte-exact stand-in for rule
/// 3's per-subfield `EmitGeneration`/`EmitBurden`/`EmitParentTxnID`/
/// `EmitNonce`/`EmitHookHash` checks, since this harness's `etxn_details()`
/// already computes the one legal value for each).
///
/// What real `HookAPI::emit` also checks that this does **not**:
/// `hook::canEmit` (this harness's `strict_can_emit` enforces that
/// separately, above this function — see `crate::env`), the emitted
/// `Transaction`'s `NEW`-status dedup check (no transaction cache exists in
/// this harness), and the final `ripple::preflight` call — full per-type
/// preflight (amount signs, flag combinations, currency/issuer validity,
/// and so on) is out of scope; [`crate::protocol_formats`]'s required-field
/// table stands in for the field-presence subset of it.
///
/// See [`reject_duplicate_fields`] for the duplicate-field rule this
/// applies, at every nesting depth — matching `emit`'s own
/// `STTx(SerialIter)` -> `STObject::set` parse path.
pub(crate) fn validate_emit_blob(
    blob: &[u8],
    expected_emit_details: Option<&[u8]>,
    hook_account: &[u8; 20],
    ledger_seq: u32,
    min_fee: u64,
) -> Result<(), ()> {
    let fields = walk_top_level(blob)?;

    reject_duplicate_fields(&fields, blob)?;

    let tx_type_field = fields
        .iter()
        .find(|f| f.code == SF_TRANSACTION_TYPE)
        .ok_or(())?;
    let tx_type_bytes = field_bytes(blob, tx_type_field.value_range).ok_or(())?;
    let tx_type_arr: [u8; 2] = tx_type_bytes.try_into().map_err(|_| ())?;
    let tx_type_value = u16::from_be_bytes(tx_type_arr);
    let tx_type = TxType::from(tx_type_value);
    if matches!(tx_type, TxType::Unknown(_)) || is_pseudo_tx_type(tx_type) {
        return Err(());
    }

    // rule 0: sfAccount must be present and equal the emitting hook's own
    // account.
    let account_field = fields.iter().find(|f| f.code == SF_ACCOUNT).ok_or(())?;
    let (a_start, a_end) = field_value_payload(blob, account_field)?;
    let account_bytes = blob.get(a_start..a_end).ok_or(())?;
    if account_bytes != hook_account {
        return Err(());
    }

    let seq_field = fields.iter().find(|f| f.code == SF_SEQUENCE).ok_or(())?;
    let seq_bytes = field_bytes(blob, seq_field.value_range).ok_or(())?;
    if seq_bytes != [0u8, 0, 0, 0] {
        return Err(());
    }

    // rule 2: sfSigningPubKey must be present and either empty or 33
    // zero bytes.
    let spk_field = fields
        .iter()
        .find(|f| f.code == SF_SIGNING_PUB_KEY)
        .ok_or(())?;
    let mut spk_pos = spk_field.value_range.0;
    let spk_len = decode_vl_len(blob, &mut spk_pos)?;
    if spk_len != 0 && spk_len != 33 {
        return Err(());
    }
    if spk_len > 0 {
        let end = spk_pos.checked_add(spk_len).ok_or(())?;
        let spk_bytes = blob.get(spk_pos..end).ok_or(())?;
        if spk_bytes.iter().any(|&b| b != 0) {
            return Err(());
        }
    }

    // rules 2.a-2.c and 4: none of these fields may be present in an
    // emitted transaction.
    if fields.iter().any(|f| {
        matches!(
            f.code,
            SF_SIGNERS | SF_TICKET_SEQUENCE | SF_ACCOUNT_TXN_ID | SF_TXN_SIGNATURE
        )
    }) {
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

    // rule 5: sfLastLedgerSequence must be present and within
    // [ledger_seq + 1, ledger_seq + 5].
    let lls_field = fields
        .iter()
        .find(|f| f.code == SF_LAST_LEDGER_SEQUENCE)
        .ok_or(())?;
    let lls_bytes = field_bytes(blob, lls_field.value_range).ok_or(())?;
    let lls = u32::from_be_bytes(lls_bytes.try_into().map_err(|_| ())?);
    let min_lls = ledger_seq.checked_add(1).ok_or(())?;
    let max_lls = ledger_seq.checked_add(5).ok_or(())?;
    if lls < min_lls || lls > max_lls {
        return Err(());
    }

    // rule 6: sfFirstLedgerSequence must be present and <=
    // sfLastLedgerSequence.
    let fls_field = fields
        .iter()
        .find(|f| f.code == SF_FIRST_LEDGER_SEQUENCE)
        .ok_or(())?;
    let fls_bytes = field_bytes(blob, fls_field.value_range).ok_or(())?;
    let fls = u32::from_be_bytes(fls_bytes.try_into().map_err(|_| ())?);
    if fls > lls {
        return Err(());
    }

    // rule 7: sfFee must be present, a native (XRP) amount, and at least
    // `min_fee`.
    let fee_field = fields.iter().find(|f| f.code == SF_FEE).ok_or(())?;
    let fee_bytes = field_bytes(blob, fee_field.value_range).ok_or(())?;
    let fee_arr: [u8; 8] = fee_bytes.try_into().map_err(|_| ())?;
    if fee_arr[0] & 0x80 != 0 {
        // Non-native (issued-currency) amount: not a legal Fee.
        return Err(());
    }
    let fee_drops = u64::from_be_bytes(fee_arr) & 0x3FFF_FFFF_FFFF_FFFF;
    if fee_drops < min_fee {
        return Err(());
    }

    // Field-presence subset of `ripple::preflight`: every field
    // `protocol_formats.json` marks `required` for this transaction type
    // (common fields plus the type's own) must be present.
    let required =
        crate::protocol_formats::required_top_level_field_codes(tx_type_value).ok_or(())?;
    for code in required {
        if !fields.iter().any(|f| f.code == code) {
            return Err(());
        }
    }

    Ok(())
}

/// The pseudo transaction types real xahaud's `isPseudoTx` rejects before
/// any other `HookAPI::emit` check (`HookAPI.cpp`'s `isPseudoTx` call,
/// right after the `STTx` parse) — a hook can never emit one of these.
fn is_pseudo_tx_type(tx_type: TxType) -> bool {
    matches!(
        tx_type,
        TxType::EnableAmendment
            | TxType::SetFee
            | TxType::UNLModify
            | TxType::EmitFailure
            | TxType::UNLReport
    )
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

    /// The emitting hook's account, shared by every test below.
    const HOOK_ACCOUNT: [u8; 20] = [9u8; 20];
    /// A minimal Payment's `sfDestination` — required by
    /// `protocol_formats.json`'s Payment format, distinct from
    /// `HOOK_ACCOUNT`.
    const DESTINATION: [u8; 20] = [8u8; 20];
    /// The world's current ledger sequence, shared by every test below.
    const LEDGER_SEQ: u32 = 100;
    /// `LEDGER_SEQ + 1`: the only legal `sfFirstLedgerSequence` value below
    /// (rule 6 only requires `<= sfLastLedgerSequence`, but pinning it here
    /// keeps every fixture unambiguous).
    const FIRST_LEDGER_SEQUENCE: u32 = LEDGER_SEQ + 1;
    /// `LEDGER_SEQ + 5`: the top of rule 5's legal `sfLastLedgerSequence`
    /// window.
    const LAST_LEDGER_SEQUENCE: u32 = LEDGER_SEQ + 5;
    /// The minimum fee `validate_emit_blob`'s caller (`Backend::emit`) would
    /// compute via `etxn_fee_base` — a fixed stand-in here since this
    /// module tests the fee *comparison*, not fee computation itself.
    const MIN_FEE: u64 = 10;

    /// Encodes `drops` as an 8-byte native (XRP) amount: bit 63 clear
    /// (native), bit 62 set (positive), the value in the low 62 bits,
    /// big-endian — mirrors `rshooks::txn::encode_native_amount`.
    fn native_amount(drops: u64) -> [u8; 8] {
        (0x4000_0000_0000_0000u64 | drops).to_be_bytes()
    }

    /// Builds a minimal, otherwise-valid emitted-Payment blob so each test
    /// can mutate exactly one thing and re-validate. Every field
    /// `validate_emit_blob` requires (including `protocol_formats.json`'s
    /// Payment-specific `sfDestination`) is present and legal against
    /// `HOOK_ACCOUNT`/`LEDGER_SEQ`/`MIN_FEE` above.
    fn minimal_payment(emit_details: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]); // TransactionType = 0 (Payment)
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]); // Sequence = 0
        out.push(0x20); // FirstLedgerSequence (2, 26)
        out.push(26);
        out.extend_from_slice(&FIRST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x20); // LastLedgerSequence (2, 27)
        out.push(27);
        out.extend_from_slice(&LAST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x61); // Amount (6, 1): native 1 drop
        out.extend_from_slice(&native_amount(1));
        out.push(0x68); // Fee (6, 8): native, exactly MIN_FEE
        out.extend_from_slice(&native_amount(MIN_FEE));
        out.extend_from_slice(&[0x73, 0x00]); // SigningPubKey: empty VL
        out.push(0x81); // Account (8, 1)
        out.push(20);
        out.extend_from_slice(&HOOK_ACCOUNT);
        out.push(0x83); // Destination (8, 3)
        out.push(20);
        out.extend_from_slice(&DESTINATION);
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

    fn validate(blob: &[u8], expected_emit_details: Option<&[u8]>) -> Result<(), ()> {
        validate_emit_blob(
            blob,
            expected_emit_details,
            &HOOK_ACCOUNT,
            LEDGER_SEQ,
            MIN_FEE,
        )
    }

    #[test]
    fn accepts_a_well_formed_blob() {
        let d = details();
        let blob = minimal_payment(&d);
        assert!(validate(&blob, Some(&d)).is_ok());
    }

    #[test]
    fn rejects_missing_emit_details_expectation() {
        let d = details();
        let blob = minimal_payment(&d);
        assert!(validate(&blob, None).is_err());
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
        assert!(validate(&blob, Some(&other)).is_err());
    }

    #[test]
    fn rejects_nonzero_sequence() {
        let d = details();
        let mut blob = minimal_payment(&d);
        if let Some(b) = blob.get_mut(7) {
            *b = 1;
        }
        assert!(validate(&blob, Some(&d)).is_err());
    }

    /// Locates `code`'s value payload range in `blob` and overwrites it with
    /// `value` — a mutate-one-field helper shared by the tests below that
    /// need to flip a field sitting past `minimal_payment`'s fixed early
    /// offsets, without hand-computing byte positions.
    fn set_field_value(blob: &mut [u8], code: u64, value: &[u8]) {
        let fields = walk_top_level_fields(blob).unwrap();
        let field = fields.iter().find(|f| f.code == code).unwrap();
        let (start, end) = field_value_payload(blob, field).unwrap();
        assert_eq!(end.checked_sub(start).unwrap(), value.len());
        blob[start..end].copy_from_slice(value);
    }

    #[test]
    fn rejects_account_mismatch() {
        // rule 0: sfAccount must equal the emitting hook's own account.
        let d = details();
        let mut blob = minimal_payment(&d);
        set_field_value(&mut blob, SF_ACCOUNT, &[0xAAu8; 20]);
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn accepts_a_33_byte_all_zero_signing_pub_key() {
        // rule 2: an unsigned emitted txn may carry either an empty
        // SigningPubKey or a 33-byte all-zero one.
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.push(0x20);
        out.push(26);
        out.extend_from_slice(&FIRST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x20);
        out.push(27);
        out.extend_from_slice(&LAST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x61);
        out.extend_from_slice(&native_amount(1));
        out.push(0x68);
        out.extend_from_slice(&native_amount(MIN_FEE));
        out.push(0x73);
        out.push(33);
        out.extend_from_slice(&[0u8; 33]); // 33 all-zero bytes
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&HOOK_ACCOUNT);
        out.push(0x83);
        out.push(20);
        out.extend_from_slice(&DESTINATION);
        out.extend_from_slice(&d);
        assert!(validate(&out, Some(&d)).is_ok());
    }

    #[test]
    fn rejects_nonempty_signing_pub_key() {
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.push(0x20);
        out.push(26);
        out.extend_from_slice(&FIRST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x20);
        out.push(27);
        out.extend_from_slice(&LAST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x68);
        out.extend_from_slice(&native_amount(MIN_FEE));
        out.push(0x73);
        out.push(1);
        out.push(0xAB); // non-empty, non-zero-length SigningPubKey
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&HOOK_ACCOUNT);
        out.push(0x83);
        out.push(20);
        out.extend_from_slice(&DESTINATION);
        out.extend_from_slice(&d);
        assert!(validate(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_a_signers_field() {
        // rule 2.a: sfSigners is never allowed in an emitted txn.
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.extend_from_slice(&[0xF3, ARRAY_END_MARKER]); // Signers (15, 3): empty array
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_a_ticket_sequence_field() {
        // rule 2.b: sfTicketSequence is never allowed in an emitted txn.
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0x20); // TicketSequence (2, 41): field >= 16, two-byte header
        blob.push(41);
        blob.extend_from_slice(&[0, 0, 0, 1]);
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_an_account_txn_id_field() {
        // rule 2.c: sfAccountTxnID is never allowed in an emitted txn.
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0x59); // AccountTxnID (5, 9)
        blob.extend_from_slice(&[0u8; 32]);
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_a_txn_signature_field() {
        // rule 4: sfTxnSignature is never allowed in an emitted txn.
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.extend_from_slice(&[0x74, 0x00]); // TxnSignature (7, 4): empty VL
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_last_ledger_sequence_at_current_ledger() {
        // rule 5: sfLastLedgerSequence must be strictly greater than the
        // current ledger sequence (>= ledger_seq + 1).
        let d = details();
        let mut blob = minimal_payment(&d);
        set_field_value(
            &mut blob,
            SF_LAST_LEDGER_SEQUENCE,
            &LEDGER_SEQ.to_be_bytes(),
        );
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_last_ledger_sequence_past_the_window() {
        // rule 5: sfLastLedgerSequence must be <= ledger_seq + 5.
        let d = details();
        let mut blob = minimal_payment(&d);
        set_field_value(
            &mut blob,
            SF_LAST_LEDGER_SEQUENCE,
            &(LAST_LEDGER_SEQUENCE + 1).to_be_bytes(),
        );
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_first_ledger_sequence_past_last() {
        // rule 6: sfFirstLedgerSequence must be <= sfLastLedgerSequence.
        let d = details();
        let mut blob = minimal_payment(&d);
        set_field_value(
            &mut blob,
            SF_FIRST_LEDGER_SEQUENCE,
            &(LAST_LEDGER_SEQUENCE + 1).to_be_bytes(),
        );
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_fee_below_the_minimum() {
        // rule 7: sfFee must be at least the caller-computed minimum.
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.push(0x20);
        out.push(26);
        out.extend_from_slice(&FIRST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x20);
        out.push(27);
        out.extend_from_slice(&LAST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x68);
        out.extend_from_slice(&native_amount(MIN_FEE - 1));
        out.extend_from_slice(&[0x73, 0x00]);
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&HOOK_ACCOUNT);
        out.push(0x83);
        out.push(20);
        out.extend_from_slice(&DESTINATION);
        out.extend_from_slice(&d);
        assert!(validate(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_missing_fee() {
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.push(0x20);
        out.push(26);
        out.extend_from_slice(&FIRST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x20);
        out.push(27);
        out.extend_from_slice(&LAST_LEDGER_SEQUENCE.to_be_bytes());
        out.extend_from_slice(&[0x73, 0x00]);
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&HOOK_ACCOUNT);
        out.push(0x83);
        out.push(20);
        out.extend_from_slice(&DESTINATION);
        out.extend_from_slice(&d);
        assert!(validate(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_a_payment_missing_the_required_destination() {
        // The field-presence subset of `ripple::preflight`, driven by
        // `protocol_formats.json`: Payment's own `sfDestination` is
        // `presence: "required"`.
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x12, 0x00, 0x00]);
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]);
        out.push(0x20);
        out.push(26);
        out.extend_from_slice(&FIRST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x20);
        out.push(27);
        out.extend_from_slice(&LAST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x61);
        out.extend_from_slice(&native_amount(1));
        out.push(0x68);
        out.extend_from_slice(&native_amount(MIN_FEE));
        out.extend_from_slice(&[0x73, 0x00]);
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&HOOK_ACCOUNT);
        out.extend_from_slice(&d);
        assert!(validate(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_a_pseudo_transaction_type() {
        // real xahaud's `isPseudoTx` rejects an emitted SetFee (101)
        // outright, before any of the field-level rules run.
        let d = details();
        let mut blob = minimal_payment(&d);
        blob[1..3].copy_from_slice(&101u16.to_be_bytes());
        assert!(validate(&blob, Some(&d)).is_err());
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
        assert!(validate(&blob, Some(&d)).is_err());
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
        out.push(0x20);
        out.push(26);
        out.extend_from_slice(&FIRST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x20);
        out.push(27);
        out.extend_from_slice(&LAST_LEDGER_SEQUENCE.to_be_bytes());
        out.push(0x61);
        out.extend_from_slice(&native_amount(1));
        out.push(0x68);
        out.extend_from_slice(&native_amount(MIN_FEE));
        out.extend_from_slice(&[0x73, 0x00]);
        out.push(0x83);
        out.push(20);
        out.extend_from_slice(&DESTINATION);
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&HOOK_ACCOUNT);
        out.extend_from_slice(&d);
        assert!(validate(&out, Some(&d)).is_ok());
    }

    #[test]
    fn rejects_duplicate_field() {
        // Citing `STObject::set`'s real "Duplicate field detected" throw —
        // see `validate_emit_blob`'s doc comment; the underlying walk
        // itself tolerates a repeat, matching `sto_validate`/`sto_subfield`
        // (see `walk_top_level_fields_accepts_a_duplicate_field` below).
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.extend_from_slice(&[0x24, 0, 0, 0, 0]); // duplicate Sequence
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn rejects_a_non_adjacent_duplicate_field() {
        // The duplicate-Sequence check does not depend on adjacency: every
        // other field sits between the two Sequence occurrences.
        let d = details();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x24, 0, 0, 0, 0]); // Sequence
        out.extend_from_slice(&minimal_payment(&d)); // every field, including Sequence again
        assert!(validate(&out, Some(&d)).is_err());
    }

    #[test]
    fn rejects_duplicate_field_inside_a_nested_object() {
        // Two Sequence-coded (2,4) fields inside one nested STObject
        // (14,9): a repeat within the same object scope is rejected even
        // though it is nested, matching `STObject::set`'s per-depth
        // invariant (see `reject_duplicate_fields`'s doc comment).
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0xE9); // nested object field: (14, 9)
        blob.extend_from_slice(&[0x24, 0, 0, 0, 1]);
        blob.extend_from_slice(&[0x24, 0, 0, 0, 2]); // duplicate within the nested object
        blob.push(OBJECT_END_MARKER);
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn accepts_the_same_field_code_in_two_different_array_elements() {
        // Each array element is its own object scope: a Sequence-coded
        // (2,4) field repeating across sibling elements is legal.
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0xFF); // array field: (15, 15)
        blob.push(0xE2); // element 1 header: (14, 2)
        blob.extend_from_slice(&[0x24, 0, 0, 0, 1]);
        blob.push(OBJECT_END_MARKER);
        blob.push(0xE3); // element 2 header: (14, 3)
        blob.extend_from_slice(&[0x24, 0, 0, 0, 2]); // same field code as element 1's
        blob.push(OBJECT_END_MARKER);
        blob.push(ARRAY_END_MARKER);
        assert!(validate(&blob, Some(&d)).is_ok());
    }

    #[test]
    fn accepts_the_same_field_code_at_top_level_and_inside_a_nested_object() {
        // A scope's field-code set is independent of its parent's: the
        // top-level Sequence (2,4) and a Sequence-coded field inside a
        // nested object (14,9) do not collide.
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0xE9); // nested object field: (14, 9)
        blob.extend_from_slice(&[0x24, 0, 0, 0, 7]);
        blob.push(OBJECT_END_MARKER);
        assert!(validate(&blob, Some(&d)).is_ok());
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
        assert!(validate(&blob, Some(&d)).is_err());
    }

    /// A single top-level `Object`-typed(14) field nested `depth` levels
    /// deep (an empty innermost object): `depth` copies of the
    /// `(type 14, field 2)` header, each opening one more level, followed
    /// by `depth` [`OBJECT_END_MARKER`]s closing them all back out. Its
    /// fields parse at [`dispatch_value`]'s `depth` running from `0`
    /// (the outer header itself) up to `depth - 1` (the innermost, empty
    /// body) — see [`STO_MAX_RECURSION_DEPTH`]'s doc comment for the
    /// convention.
    fn nested_object_chain(depth: u32) -> Vec<u8> {
        let depth = depth as usize;
        let mut out = vec![0xE2u8; depth]; // (type 14, field 2), repeated
        out.extend(vec![OBJECT_END_MARKER; depth]);
        out
    }

    #[test]
    fn accepts_nesting_up_to_the_real_hosts_limit() {
        // Real xahaud's `get_stobject_length` accepts recursion depths
        // 0..=10 (`HookAPI.cpp:2901`); depth 3 (previously rejected by this
        // walker's stricter, now-removed depth-2 limit) and depth 10 (the
        // boundary) must both parse.
        for depth in [2u32, 3, 10] {
            let d = details();
            let mut blob = minimal_payment(&d);
            blob.extend_from_slice(&nested_object_chain(depth));
            assert!(
                validate(&blob, Some(&d)).is_ok(),
                "depth {depth} should be accepted"
            );
        }
    }

    #[test]
    fn rejects_depth_beyond_the_real_hosts_limit() {
        // A chain of 11 nested Object-typed(14) headers: the 11th would
        // open recursion depth 11, over `get_stobject_length`'s bound
        // (`HookAPI.cpp:2901`). The depth check fires as soon as the 11th
        // header's value is dispatched, before any terminator is needed.
        let depth_violation: [u8; 11] = [0xE2u8; 11];

        let d = details();
        let mut blob = minimal_payment(&d);
        blob.extend_from_slice(&depth_violation);
        assert!(validate(&blob, Some(&d)).is_err());
    }

    #[test]
    fn walk_top_level_fields_matches_the_same_depth_bound() {
        // `walk_top_level_fields` (the shared parser `sto_validate`/
        // `sto_subfield`/`slot_subfield` all call directly) uses the exact
        // same bound as [`validate_emit_blob`]'s tests above.
        assert!(walk_top_level_fields(&nested_object_chain(10)).is_ok());
        assert!(walk_top_level_fields(&[0xE2u8; 11]).is_err());
    }

    #[test]
    fn accepts_an_array_field_of_object_elements() {
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0xFF); // array field: (type 15, field 15)
        blob.push(0xE2); // element header: (type 14, field 2) -- STObject
        blob.push(OBJECT_END_MARKER); // empty object body
        blob.push(ARRAY_END_MARKER);
        assert!(validate(&blob, Some(&d)).is_ok());
    }

    #[test]
    fn rejects_an_array_element_that_is_not_an_object() {
        let d = details();
        let mut blob = minimal_payment(&d);
        blob.push(0xFF); // array field: (type 15, field 15)
        blob.push(0x21); // element header: (type 2, field 1) -- UInt32, not STObject
        blob.extend_from_slice(&[0, 0, 0, 1]);
        blob.push(ARRAY_END_MARKER);
        assert!(validate(&blob, Some(&d)).is_err());
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
