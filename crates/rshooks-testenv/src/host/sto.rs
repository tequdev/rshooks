//! `sto_*` semantics (P2-D — `.claude/design/TESTENV_PHASE2_DESIGN.md` §4
//! "sto_*", stage plan §7): pure byte-level STObject surgery over caller
//! buffers, extending `crate::emit_walk`'s canonical field walker to expose
//! offsets rather than forking it — see that module's own doc comment for
//! the shared [`crate::emit_walk::FieldSpan`]/[`crate::emit_walk::field_value_payload`]
//! primitives every function here reuses.
//!
//! Ported against `Xahau/xahaud`, branch `dev`,
//! `src/xrpld/app/hook/detail/HookAPI.cpp` (fetched and read directly for
//! this stage). All five functions share one upstream helper,
//! `HookAPI::get_stobject_length` (`HookAPI.cpp:2888-3179`) — a hand-rolled,
//! non-STObject-constructing single-field parser this module's own
//! `crate::emit_walk` walker already independently reimplements the same
//! grammar for (canonical field-header/VL decoding, per-STI payload-length
//! rules, recursive nested-object/array length computation stopping at the
//! matching `0xE1`/`0xF1` terminator) — hence "extend, don't fork".
//!
//! # `sto_subfield`'s offset/length convention
//!
//! `HookAPI::sto_subfield` (`HookAPI.cpp:98-156`) returns, for a matched
//! field (`HookAPI.cpp:143-147`):
//! ```cpp
//! if (type == 0xF)  // we return arrays fully formed
//!     return std::make_pair(upto - start, length.value());
//! // return pointers to all other objects as payloads
//! return std::make_pair(upto - start + payload_start, payload_length);
//! ```
//! i.e. an `STI_ARRAY`(15) field is returned **fully formed** (header +
//! elements + `0xF1`, offset at the field's own header); every other type
//! is returned as its **payload** (header excluded; VL length-prefix
//! excluded too for `STI_VL`(7)/`STI_ACCOUNT`(8), since
//! `get_stobject_length`'s `payload_start`/`payload_length` are computed
//! *after* decoding a VL type's own length prefix). This module's
//! [`sto_subfield`] mirrors that split exactly: `field.range` (full,
//! header-included) for `STI_ARRAY`, [`crate::emit_walk::field_value_payload`]
//! (VL-stripped payload) for everything else. Packed into the `int64_t`
//! return value as `(offset << 32) | length` (`applyHook.cpp:2940`,
//! matching `macro.h`'s `SUB_OFFSET`/`SUB_LENGTH`).
//!
//! # `sto_subarray`
//!
//! `HookAPI::sto_subarray` (`HookAPI.cpp:158-234`) always returns elements
//! **fully formed** (header + value + `0xE1`, `HookAPI.cpp:225`) — matching
//! `crate::emit_walk::walk_array_elements`'s own element-span convention
//! directly, no extra stripping needed. Upstream *optionally* strips a
//! leading `0xF...` array-type header from the input buffer by blindly
//! trimming bytes positionally, without verifying the trailing terminator
//! it assumes is there (`HookAPI.cpp:174-193`); this module keeps the same
//! optional header-stripping but — deliberately more strict, and documented
//! as a departure — verifies the remaining bytes actually end in `0xF1` via
//! [`crate::emit_walk::walk_array_elements`] rather than trusting position
//! alone. It also assumes `fixHookAPI20251128`'s corrected 2-byte-header
//! handling is always active (this harness does not model amendments).
//!
//! # `sto_validate`
//!
//! `HookAPI::sto_validate` (`HookAPI.cpp:68-96`) is a top-level-only
//! well-formedness + full-consumption check (no field-ordering or
//! duplicate-field check, even though nested objects/arrays are internally
//! bounded by their own terminators) — exactly
//! [`crate::emit_walk::walk_top_level_fields`]'s own contract, reused
//! as-is.
//!
//! # `sto_emplace`/`sto_erase`
//!
//! `HookAPI::sto_emplace` (`HookAPI.cpp:236-376`): the `field` argument is
//! **fully formed and wrapped** (header included, not just payload —
//! `applyHook.cpp:3094-3099`'s own comment), spliced in verbatim
//! (`HookAPI.cpp:359-364`) at the canonical position found by scanning
//! source fields for the first `code > field_id` (`HookAPI.cpp:310-337`).
//! This module always applies the `field_id`-vs-injected-header
//! cross-check upstream gates behind `fixHookAPI20251128`
//! (`HookAPI.cpp:262-286`) — again, no amendment model, so the corrected
//! behavior is the only behavior. `sto_erase` (`applyHook.cpp:3184-3216`)
//! is `sto_emplace` with an empty field, `DOESNT_EXIST` substituted when
//! the output size equals the input size (nothing was removed).
//!
//! Size limits (`HookAPI.cpp:243-259`): source `2..=16384` bytes, field (if
//! present) `2..=4096` bytes. `MEM_OVERLAP` (a wasm-linear-memory
//! caller-buffer-aliasing concept, `applyHook.cpp:3132-3159`) has no
//! counterpart here — every buffer this harness's `sto_*` functions see is
//! an owned Rust value, never two views into the same backing memory, so
//! the condition it guards against cannot arise.

use std::vec::Vec;

use rshooks_core::{DOESNT_EXIST, PARSE_ERROR, TOO_BIG, TOO_SMALL};

use crate::emit_walk;

const STI_ARRAY: u32 = 15;

/// `SUB_OFFSET`/`SUB_LENGTH`'s packing (`macro.h`,
/// `crates/rshooks-core/vendor/xahaud-hook/macro.h:145-146`): offset in the
/// upper 32 bits, length in the lower 32 — `applyHook.cpp:2940`/`2964`.
fn pack(offset: usize, length: usize) -> i64 {
    ((offset as i64) << 32) | (length as i64 & 0xFFFF_FFFF)
}

/// `HookAPI::sto_validate` — see this module's doc comment.
pub(crate) fn sto_validate(sto: &[u8]) -> i64 {
    if sto.len() < 2 {
        return TOO_SMALL;
    }
    match emit_walk::walk_top_level_fields(sto) {
        Ok(_) => 1,
        Err(()) => 0,
    }
}

/// `HookAPI::sto_subfield` — see this module's doc comment.
pub(crate) fn sto_subfield(sto: &[u8], field_id: u32) -> i64 {
    if sto.len() < 2 {
        return TOO_SMALL;
    }
    let fields = match emit_walk::walk_top_level_fields(sto) {
        Ok(f) => f,
        Err(()) => return PARSE_ERROR,
    };
    let target = u64::from(field_id);
    let Some(field) = fields.iter().find(|f| f.code == target) else {
        return DOESNT_EXIST;
    };
    let ty = (field.code >> 16) as u32;
    let range = if ty == STI_ARRAY {
        Ok(field.range)
    } else {
        emit_walk::field_value_payload(sto, field)
    };
    match range {
        Ok((start, end)) => pack(start, end.wrapping_sub(start)),
        Err(()) => PARSE_ERROR,
    }
}

/// `HookAPI::sto_subarray` — see this module's doc comment.
pub(crate) fn sto_subarray(array: &[u8], index: u32) -> i64 {
    if array.len() < 2 {
        return TOO_SMALL;
    }
    // Optional leading array-type header: strip it if present, tracking the
    // stripped prefix length so returned offsets translate back into
    // `array`'s own coordinate space.
    let base = match array.first() {
        Some(&b0) if b0 & 0xF0 == 0xF0 => {
            if b0 & 0x0F == 0 {
                2 // two-byte header: field number >= 16 (`fixHookAPI20251128` form)
            } else {
                1 // one-byte header: field number < 16
            }
        }
        _ => 0,
    };
    let Some(body) = array.get(base..) else {
        return PARSE_ERROR;
    };
    let elements = match emit_walk::walk_array_elements(body) {
        Ok(e) => e,
        Err(()) => return PARSE_ERROR,
    };
    match elements.get(index as usize) {
        Some(&(start, end)) => pack(start.wrapping_add(base), end.wrapping_sub(start)),
        None => DOESNT_EXIST,
    }
}

/// `HookAPI::sto_emplace` — see this module's doc comment. `field.is_empty()`
/// is the deletion form (`sto_erase`'s own call shape).
pub(crate) fn sto_emplace(source: &[u8], field: &[u8], field_id: u32) -> Result<Vec<u8>, i64> {
    if source.len() > 16384 {
        return Err(TOO_BIG);
    }
    if source.len() < 2 {
        return Err(TOO_SMALL);
    }
    if !field.is_empty() {
        if field.len() > 4096 {
            return Err(TOO_BIG);
        }
        if field.len() < 2 {
            return Err(TOO_SMALL);
        }
        // `fixHookAPI20251128`'s field_id/injected-header cross-check,
        // always applied (see module doc comment): `field` must itself
        // parse as exactly one fully-formed top-level field, whose own
        // header code matches `field_id`.
        let injected = emit_walk::walk_top_level_fields(field).map_err(|()| PARSE_ERROR)?;
        let [only] = injected.as_slice() else {
            return Err(PARSE_ERROR);
        };
        if only.range != (0, field.len()) || only.code != u64::from(field_id) {
            return Err(PARSE_ERROR);
        }
    }

    let fields = emit_walk::walk_top_level_fields(source).map_err(|()| PARSE_ERROR)?;
    let target = u64::from(field_id);
    let mut inject_start = source.len();
    let mut inject_end = source.len();
    for f in &fields {
        if f.code == target {
            inject_start = f.range.0;
            inject_end = f.range.1;
            break;
        }
        if f.code > target {
            inject_start = f.range.0;
            inject_end = f.range.0;
            break;
        }
    }

    let mut out = Vec::with_capacity(source.len().wrapping_add(field.len()));
    out.extend_from_slice(source.get(..inject_start).ok_or(PARSE_ERROR)?);
    out.extend_from_slice(field);
    out.extend_from_slice(source.get(inject_end..).ok_or(PARSE_ERROR)?);
    Ok(out)
}

/// `sto_erase` — `sto_emplace` with an empty field
/// (`applyHook.cpp:3196-3216`); `DOESNT_EXIST` when nothing shrank (the
/// field was never present — an insert-only `sto_emplace` with an empty
/// field never grows the output, so equal length unambiguously means "not
/// found").
pub(crate) fn sto_erase(source: &[u8], field_id: u32) -> Result<Vec<u8>, i64> {
    let out = sto_emplace(source, &[], field_id)?;
    if out.len() == source.len() {
        return Err(DOESNT_EXIST);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    fn sf_sequence(v: u32) -> Vec<u8> {
        let mut out = vec![0x24]; // (type 2, field 4)
        out.extend_from_slice(&v.to_be_bytes());
        out
    }

    fn sf_flags(v: u32) -> Vec<u8> {
        let mut out = vec![0x22]; // (type 2, field 2)
        out.extend_from_slice(&v.to_be_bytes());
        out
    }

    fn sf_fee(drops: u64) -> Vec<u8> {
        let mut out = vec![0x68]; // (type 6, field 8)
        let mut amt = drops.to_be_bytes();
        amt[0] |= 0x40;
        out.extend_from_slice(&amt);
        out
    }

    const SF_SEQUENCE_CODE: u32 = (2 << 16) + 4;
    const SF_FLAGS_CODE: u32 = (2 << 16) + 2;
    const SF_FEE_CODE: u32 = (6 << 16) + 8;
    const SF_ACCOUNT_CODE: u32 = (8 << 16) + 1;

    fn sample() -> Vec<u8> {
        // Flags(2,2) < Sequence(2,4) < Fee(6,8), in canonical order.
        let mut out = sf_flags(1);
        out.extend_from_slice(&sf_sequence(5));
        out.extend_from_slice(&sf_fee(10));
        out
    }

    // -- sto_validate --

    #[test]
    fn sto_validate_accepts_well_formed_and_rejects_malformed() {
        assert_eq!(sto_validate(&sample()), 1);
        assert_eq!(sto_validate(&[0xE2, 0xE2]), 0); // truncated nested object
        let mut trailing = sample();
        trailing.push(0xFF);
        assert_eq!(sto_validate(&trailing), 0);
    }

    #[test]
    fn sto_validate_too_short_is_too_small() {
        assert_eq!(sto_validate(&[0x01]), TOO_SMALL);
        assert_eq!(sto_validate(&[]), TOO_SMALL);
    }

    // -- sto_subfield --

    #[test]
    fn sto_subfield_locates_a_scalar_fields_payload() {
        let data = sample();
        let packed = sto_subfield(&data, SF_SEQUENCE_CODE);
        assert!(packed >= 0);
        let offset = (packed >> 32) as usize;
        let length = (packed & 0xFFFF_FFFF) as usize;
        assert_eq!(length, 4);
        assert_eq!(&data[offset..offset + length], &5u32.to_be_bytes());
    }

    #[test]
    fn sto_subfield_strips_vl_prefix_for_account() {
        let mut data = vec![0x81, 20]; // sfAccount, VL-prefixed
        data.extend_from_slice(&[7u8; 20]);
        let packed = sto_subfield(&data, SF_ACCOUNT_CODE);
        let offset = (packed >> 32) as usize;
        let length = (packed & 0xFFFF_FFFF) as usize;
        assert_eq!((offset, length), (2, 20));
    }

    #[test]
    fn sto_subfield_missing_field_is_doesnt_exist() {
        assert_eq!(sto_subfield(&sample(), 0x0002_0099), DOESNT_EXIST);
    }

    #[test]
    fn sto_subfield_malformed_is_parse_error() {
        assert_eq!(sto_subfield(&[0xE2, 0xE2], SF_SEQUENCE_CODE), PARSE_ERROR);
    }

    #[test]
    fn sto_subfield_too_short_is_too_small() {
        assert_eq!(sto_subfield(&[0x01], SF_SEQUENCE_CODE), TOO_SMALL);
    }

    #[test]
    fn sto_subfield_returns_arrays_fully_formed() {
        // sfMemos-shaped array field: (type 15, field 9) = 0xF9, one empty
        // element, terminator.
        let data: &[u8] = &[0xF9, 0xE2, 0xE1, 0xF1];
        let code = (15u32 << 16) + 9;
        let packed = sto_subfield(data, code);
        let offset = (packed >> 32) as usize;
        let length = (packed & 0xFFFF_FFFF) as usize;
        // Fully formed: header included, through the array terminator.
        assert_eq!((offset, length), (0, 4));
    }

    // -- sto_subarray --

    #[test]
    fn sto_subarray_returns_each_element_fully_formed_and_bare() {
        // Bare element sequence (no leading array header), two elements.
        let data: &[u8] = &[0xE2, 0xE1, 0xE3, 0xE1, 0xF1];
        let packed0 = sto_subarray(data, 0);
        assert_eq!(packed0, pack(0, 2));
        let packed1 = sto_subarray(data, 1);
        assert_eq!(packed1, (2i64 << 32) | 2);
        assert_eq!(sto_subarray(data, 2), DOESNT_EXIST);
    }

    #[test]
    fn sto_subarray_strips_an_optional_leading_header() {
        // Leading array-type header (type 15, field 9 -> 0xF9), then the
        // same bare element sequence.
        let mut data = vec![0xF9];
        data.extend_from_slice(&[0xE2, 0xE1, 0xF1]);
        let packed = sto_subarray(&data, 0);
        let offset = (packed >> 32) as usize;
        let length = (packed & 0xFFFF_FFFF) as usize;
        // Offset translated back into the original (header-included) buffer.
        assert_eq!((offset, length), (1, 2));
    }

    #[test]
    fn sto_subarray_too_short_is_too_small() {
        assert_eq!(sto_subarray(&[0x01], 0), TOO_SMALL);
    }

    // -- sto_emplace / sto_erase --

    #[test]
    fn sto_emplace_inserts_at_canonical_position() {
        // Insert Flags(2,2) into a source that only has Sequence(2,4) and
        // Fee(6,8) — must land *before* Sequence.
        let mut source = sf_sequence(5);
        source.extend_from_slice(&sf_fee(10));
        let field = sf_flags(9);
        let out = sto_emplace(&source, &field, SF_FLAGS_CODE).unwrap();
        let mut expected = sf_flags(9);
        expected.extend_from_slice(&sf_sequence(5));
        expected.extend_from_slice(&sf_fee(10));
        assert_eq!(out, expected);
    }

    #[test]
    fn sto_emplace_appends_when_code_is_greater_than_every_existing_field() {
        let source = sf_flags(1);
        let field = sf_fee(20);
        let out = sto_emplace(&source, &field, SF_FEE_CODE).unwrap();
        let mut expected = sf_flags(1);
        expected.extend_from_slice(&sf_fee(20));
        assert_eq!(out, expected);
    }

    #[test]
    fn sto_emplace_replaces_an_existing_field_in_place() {
        let source = sample();
        let field = sf_sequence(999);
        let out = sto_emplace(&source, &field, SF_SEQUENCE_CODE).unwrap();
        let mut expected = sf_flags(1);
        expected.extend_from_slice(&sf_sequence(999));
        expected.extend_from_slice(&sf_fee(10));
        assert_eq!(out, expected);
    }

    #[test]
    fn sto_emplace_rejects_field_id_mismatch_with_injected_header() {
        let source = sample();
        let field = sf_sequence(1); // header says Sequence...
        assert_eq!(
            sto_emplace(&source, &field, SF_FLAGS_CODE), // ...but field_id says Flags
            Err(PARSE_ERROR)
        );
    }

    #[test]
    fn sto_emplace_size_limits() {
        assert_eq!(sto_emplace(&[0x01], &[], 0), Err(TOO_SMALL));
        let huge = vec![0u8; 16385];
        assert_eq!(sto_emplace(&huge, &[], 0), Err(TOO_BIG));
        let source = sample();
        assert_eq!(
            sto_emplace(&source, &[0x01], SF_SEQUENCE_CODE),
            Err(TOO_SMALL)
        );
        let huge_field = vec![0u8; 4097];
        assert_eq!(
            sto_emplace(&source, &huge_field, SF_SEQUENCE_CODE),
            Err(TOO_BIG)
        );
    }

    #[test]
    fn sto_erase_removes_the_field_at_its_canonical_position() {
        let source = sample();
        let out = sto_erase(&source, SF_SEQUENCE_CODE).unwrap();
        let mut expected = sf_flags(1);
        expected.extend_from_slice(&sf_fee(10));
        assert_eq!(out, expected);
    }

    #[test]
    fn sto_erase_missing_field_is_doesnt_exist() {
        assert_eq!(sto_erase(&sample(), 0x0002_0099), Err(DOESNT_EXIST));
    }

    #[test]
    fn sto_emplace_then_erase_round_trips() {
        let source = sf_flags(1);
        let inserted = sto_emplace(&source, &sf_sequence(7), SF_SEQUENCE_CODE).unwrap();
        let erased = sto_erase(&inserted, SF_SEQUENCE_CODE).unwrap();
        assert_eq!(erased, source);
    }
}
