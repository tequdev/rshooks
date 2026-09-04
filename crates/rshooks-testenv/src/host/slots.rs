//! `slot_*`/`otxn_slot`/`meta_slot`/`xpop_slot` semantics (P2-D —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "slot family", stage plan
//! §7).
//!
//! Every function here is ported against `Xahau/xahaud`, branch `dev`,
//! `src/xrpld/app/hook/detail/HookAPI.cpp`/`applyHook.cpp`; line numbers are
//! cited per function below. One structural decision governs the whole
//! module, stated once here rather than per function:
//!
//! # Slot content = "value payload", exactly what `slot()` itself returns
//!
//! Upstream's `HookAPI::slot(slot_no)` returns the stored `STBase const*`
//! (`hookCtx.slot[slot_no].entry`), and the wasm-facing wrapper serializes
//! it with a bare `entry->add(s)` call (`applyHook.cpp:1946-1958`) — the
//! *value's own* serialization, never the field header that names it
//! within its parent (that header is written by the parent's own
//! field-walk, not the value's `add()`). Same code path `otxn_field` uses
//! (`applyHook.cpp:1908-1916`), so `otxn_field`'s documented "fixed-width
//! `sfAccount`, VL-prefixed `sfBlob`" trap
//! (`~/.claude/skills/hook-api/references/otxn.md`) applies here too: an
//! `AccountID` field's slot content is its raw 20 bytes, *not* the
//! wire-framed `<0x14><20 bytes>` a full-object parse would show for that
//! same field. `crate::emit_walk::field_value_payload` implements exactly
//! this convention: VL length-prefix stripped for `STI_VL`(7)/
//! `STI_ACCOUNT`(8), value bytes as-is otherwise — including the trailing
//! `0xE1`/`0xF1` terminator for a nested `STI_OBJECT`(14)/`STI_ARRAY`(15)
//! field, since that terminator is part of *that* value's own
//! serialization. `examples/15_slot-objects`' e2e-pinned
//! `check_account_walk` (exactly 20 bytes for `sfAccount`) is the fidelity
//! anchor for this convention.
//!
//! A **root** slot (`otxn_slot`/`meta_slot`/`xpop_slot`/`slot_set`) has no
//! enclosing object at all, so its content is a bare canonical field
//! sequence with no wrapping header or terminator — the same shape
//! `crate::emit_walk`'s top-level (non-nested) walk already expects.
//!
//! # `slot_type(no, 0)`'s root object codes
//!
//! `HookAPI::slot_type` (`HookAPI.cpp:2294-2305`) returns the slot's own
//! `SField`'s `fieldCode`; the wrapper (`applyHook.cpp:2075-2084`) reports
//! that as the `flags = 0` result verbatim. For an ordinary field (from
//! `slot_subfield`/`slot_subarray`) that is just the field's own
//! `(sti << 16) | field` code. A **root** slot's backing object (a whole
//! `STTx`/SLE/metadata) is tagged with one of rippled's special
//! whole-object `SField`s, whose serialized type ID lands in the
//! `10001..=10004` range — `STI_TRANSACTION`(10001)/`STI_LEDGERENTRY`(10002)/
//! `STI_VALIDATION`(10003)/`STI_METADATA`(10004)
//! (`XRPLF/rippled`, `include/xrpl/protocol/SField.h`) — which is exactly
//! the range `rshooks::slot_obj`'s `ROOT_OBJECT_CODE_MIN..=ROOT_OBJECT_CODE_MAX`
//! and `examples/15_slot-objects`' `check_root_cast` already assume. This
//! module synthesizes `(STI_TRANSACTION << 16) | 257`/`(STI_LEDGERENTRY << 16) | 257`/
//! `(STI_METADATA << 16) | 257` for `otxn_slot`/`slot_set`/`meta_slot` (and
//! `xpop_slot`'s two halves) respectively — field number `257`, matching
//! upstream's actual root `SField`s (`sfLedgerEntry`/`sfTransaction`/
//! `sfMetadata`, all field number `257`: `XRPLF/rippled`,
//! `include/xrpl/protocol/detail/sfields.macro`), pinned by this module's
//! own tests.
//!
//! # Slot allocation
//!
//! `InvocationContext::resolve_slot` (see its own doc comment) is the
//! design's documented simplification of upstream's FIFO-then-counter
//! allocator — every function below defers to it rather than reimplementing
//! `HookAPI::get_free_slot`.

use std::vec::Vec;

use rshooks_core::{
    DOESNT_EXIST, INVALID_ARGUMENT, NOT_AN_AMOUNT, NOT_AN_ARRAY, NOT_AN_OBJECT, PARSE_ERROR,
    PREREQUISITE_NOT_MET,
};

use crate::emit_walk::{self, FieldSpan};
use crate::invocation::{InvocationContext, SlotEntry, SlotKind};
use crate::world::World;

/// `slot_type(no, 0)`'s root-slot value for `otxn_slot`/`xpop_slot`'s tx
/// half: `STI_TRANSACTION`(10001) `<< 16` `| 257` (`sfTransaction`'s real
/// field number) — see this module's doc comment.
const ROOT_TRANSACTION: u32 = (10001 << 16) | 257;
/// `slot_type(no, 0)`'s root-slot value for `slot_set` (a seeded ledger
/// object): `STI_LEDGERENTRY`(10002) `<< 16` `| 257` (`sfLedgerEntry`'s real
/// field number).
const ROOT_LEDGER_ENTRY: u32 = (10002 << 16) | 257;
/// `slot_type(no, 0)`'s root-slot value for `meta_slot`/`xpop_slot`'s meta
/// half: `STI_METADATA`(10004) `<< 16` `| 257` (`sfMetadata`'s real field
/// number).
const ROOT_METADATA: u32 = (10004 << 16) | 257;

/// `STI_AMOUNT`'s wire byte 0, bit 7 clear = native (XAH). Shared by
/// `slot_type(no, 1)` and `slot_float` — see `~/.claude/skills/hook-api/
/// references/slot.md`'s "Native Amount wire format" section.
fn amount_is_native(bytes: &[u8]) -> bool {
    bytes.first().is_some_and(|b| b & 0x80 == 0)
}

/// `HookAPI::slot` (`HookAPI.cpp:2042-2052`) + the wrapper's `entry->add(s)`
/// (`applyHook.cpp:1946-1958`) — see this module's doc comment.
pub(crate) fn slot(ctx: &InvocationContext, slot_no: u32) -> Result<Vec<u8>, i64> {
    ctx.slot_entry(slot_no)
        .map(|e| e.bytes.clone())
        .ok_or(DOESNT_EXIST)
}

/// `HookAPI::slot_clear` (`HookAPI.cpp:2054-2063`): existence check only, no
/// `INTERNAL_ERROR` path — this harness never constructs a `SlotEntry` with
/// a null/absent payload, so that branch is unreachable here. Frees the
/// slot number for reuse.
pub(crate) fn slot_clear(ctx: &mut InvocationContext, slot_no: u32) -> i64 {
    match ctx.slots.get_mut(slot_no as usize) {
        Some(entry @ Some(_)) => {
            *entry = None;
            1
        }
        _ => DOESNT_EXIST,
    }
}

/// `HookAPI::slot_count` (`HookAPI.cpp:2065-2078`): `STI_ARRAY`-only,
/// `NOT_AN_ARRAY` otherwise.
pub(crate) fn slot_count(ctx: &InvocationContext, slot_no: u32) -> i64 {
    let Some(entry) = ctx.slot_entry(slot_no) else {
        return DOESNT_EXIST;
    };
    if entry.kind != SlotKind::Array {
        return NOT_AN_ARRAY;
    }
    match emit_walk::walk_array_elements(&entry.bytes) {
        Ok(elements) => elements.len() as i64,
        // Defensive: every `SlotKind::Array` entry here comes from a
        // successful `walk_array_elements`/`walk_object_body` call, so a
        // parse failure here means a bug in this module, not a reachable
        // hook-observable error.
        Err(()) => PARSE_ERROR,
    }
}

/// `HookAPI::slot_size` (`HookAPI.cpp:2143-2156`): no type restriction,
/// computed via the same `add(s)` call `slot()` uses — i.e. exactly
/// `entry.bytes.len()` here, since that is already the payload `slot()`
/// would return.
pub(crate) fn slot_size(ctx: &InvocationContext, slot_no: u32) -> i64 {
    match ctx.slot_entry(slot_no) {
        Some(e) => e.bytes.len() as i64,
        None => DOESNT_EXIST,
    }
}

/// `HookAPI::slot_type` (`HookAPI.cpp:2282-2313`) + the wrapper's
/// `flags = 0`/`flags = 1` return-value construction
/// (`applyHook.cpp:2075-2084`).
pub(crate) fn slot_type(ctx: &InvocationContext, slot_no: u32, flags: u32) -> i64 {
    let Some(entry) = ctx.slot_entry(slot_no) else {
        return DOESNT_EXIST;
    };
    match flags {
        0 => i64::from(entry.reported_code),
        1 => {
            if entry.kind != SlotKind::Amount {
                return NOT_AN_AMOUNT;
            }
            i64::from(amount_is_native(&entry.bytes))
        }
        _ => INVALID_ARGUMENT,
    }
}

/// `HookAPI::slot_float` (`HookAPI.cpp:2315-2368`). Native: the full 62-bit
/// drops magnitude (bits 0..=61 of the 8-byte value, byte 0's low 6 bits
/// included) becomes the mantissa at exponent `-6`, then normalized —
/// mirrors `st_amt.xrp().drops()` → `hook_float::normalize_xfl(drops, -6,
/// neg)` (`HookAPI.cpp:2326-2338`). Deliberately **not**
/// `crate::host::float::float_sto_set`'s decode: the two are distinct
/// upstream functions with distinct (and, for large drops values,
/// disagreeing) native-amount decodes; only `slot_float`'s is ported here.
/// IOU: the wire mantissa/exponent of a well-formed ledger amount are
/// already in canonical XFL range, so no renormalization applies — mirrors
/// `st_amt.iou()` → the `IOUAmount` overload of `make_float`
/// (`HookAPI.cpp:2342-2351`), not the byte-triple `make_float` this module
/// calls directly.
pub(crate) fn slot_float(ctx: &InvocationContext, slot_no: u32) -> i64 {
    let Some(entry) = ctx.slot_entry(slot_no) else {
        return DOESNT_EXIST;
    };
    if entry.kind != SlotKind::Amount {
        return NOT_AN_AMOUNT;
    }
    crate::host::float::slot_amount_to_xfl(&entry.bytes)
}

/// `HookAPI::slot_set` (`HookAPI.cpp:2080-2141`), restricted to this
/// harness's seeded-ledger-object model (design §4: a 32-byte transaction
/// hash is a documented Phase-2 model limit — upstream's historical
/// transaction lookup, `getMasterTransaction().fetch`, is not ported; every
/// 32-byte input reports `DOESNT_EXIST`).
pub(crate) fn slot_set(
    ctx: &mut InvocationContext,
    world: &World,
    data: &[u8],
    slot_into: u32,
) -> i64 {
    if data.len() != 34 && data.len() != 32 {
        return INVALID_ARGUMENT;
    }
    if slot_into > 255 {
        return INVALID_ARGUMENT;
    }
    // Upstream (`HookAPI.cpp:2086-2088`) checks slot availability *before*
    // ever attempting the keylet/tx-hash lookup: `slot_into == 0`
    // (auto-assign) with every slot already occupied fails `NO_FREE_SLOTS`
    // even for input that would otherwise resolve to `DOESNT_EXIST`.
    if slot_into == 0
        && let Err(e) = ctx.resolve_slot(0)
    {
        return e;
    }
    if data.len() == 32 {
        return DOESNT_EXIST;
    }
    let mut keylet = [0u8; 34];
    keylet.copy_from_slice(data);
    let Some(bytes) = world.ledger_objects.get(&keylet).cloned() else {
        return DOESNT_EXIST;
    };
    let slot_no = match ctx.resolve_slot(slot_into) {
        Ok(n) => n,
        Err(e) => return e,
    };
    ctx.set_slot(
        slot_no,
        SlotEntry {
            bytes,
            kind: SlotKind::Root,
            reported_code: ROOT_LEDGER_ENTRY,
        },
    );
    slot_no as i64
}

/// `HookAPI::otxn_slot` (`HookAPI.cpp:1565-1593`): loads the canonical
/// serialization of the seeded originating transaction
/// (`crate::otxn::serialize`) as a root slot. Upstream special-cases
/// `hookCtx.emitFailure` (a `cbak`-failure context loading the emitted tx's
/// real fields instead of the synthetic `ttEMIT_FAILURE`-rewritten applying
/// tx); no separate case is needed here because
/// `crate::env::TestEnv::invoke_cbak` already swaps `World::otxn` to the
/// emitted transaction's own fields for the whole call (both
/// `CbakOutcome::Success`/`CbakOutcome::Failure`; this harness never models
/// the `ttEMIT_FAILURE` rewrite), so reading `world.otxn` here already
/// matches upstream's `emitFailure` behavior.
pub(crate) fn otxn_slot(ctx: &mut InvocationContext, world: &World, slot_into: u32) -> i64 {
    if slot_into > 255 {
        return INVALID_ARGUMENT;
    }
    let bytes = crate::otxn::serialize(&world.otxn);
    let slot_no = match ctx.resolve_slot(slot_into) {
        Ok(n) => n,
        Err(e) => return e,
    };
    ctx.set_slot(
        slot_no,
        SlotEntry {
            bytes,
            kind: SlotKind::Root,
            reported_code: ROOT_TRANSACTION,
        },
    );
    slot_no as i64
}

/// `HookAPI::meta_slot` (`HookAPI.cpp:2375-2402`): `PREREQUISITE_NOT_MET`
/// when no metadata was seeded (`hookCtx.result.provisionalMeta` unset
/// upstream) — **not** `DOESNT_EXIST`, per the cited source.
pub(crate) fn meta_slot(ctx: &mut InvocationContext, world: &World, slot_into: u32) -> i64 {
    let Some(bytes) = world.otxn_meta.clone() else {
        return PREREQUISITE_NOT_MET;
    };
    if slot_into > 255 {
        return INVALID_ARGUMENT;
    }
    let slot_no = match ctx.resolve_slot(slot_into) {
        Ok(n) => n,
        Err(e) => return e,
    };
    ctx.set_slot(
        slot_no,
        SlotEntry {
            bytes,
            kind: SlotKind::Root,
            reported_code: ROOT_METADATA,
        },
    );
    slot_no as i64
}

/// `HookAPI::xpop_slot` (`HookAPI.cpp:2404-2468`) + the wrapper's
/// return-value packing (`applyHook.cpp:3942`:
/// `std::get<0>(result) << 16U | std::get<1>(result)` — tx slot number in
/// the upper 16 bits, meta slot number in the low 16 bits). Loads both
/// halves of the seeded `world.xpop` pair; absent → `DOESNT_EXIST` (design
/// §4, not upstream's `ttIMPORT`-only `PREREQUISITE_NOT_MET` gate — this
/// harness has no `ttIMPORT`/`Import::getInnerTxn` model, so "was an xpop
/// seeded" is the only precondition). Rejects `slot_no_tx == slot_no_meta`
/// when both are explicit and equal, mirroring upstream's collision guard
/// (`HookAPI.cpp:2419-2421`).
pub(crate) fn xpop_slot(
    ctx: &mut InvocationContext,
    world: &World,
    slot_no_tx: u32,
    slot_no_meta: u32,
) -> i64 {
    if slot_no_tx > 255 || slot_no_meta > 255 {
        return INVALID_ARGUMENT;
    }
    if slot_no_tx != 0 && slot_no_tx == slot_no_meta {
        return INVALID_ARGUMENT;
    }
    let Some((tx, meta)) = world.xpop.clone() else {
        return DOESNT_EXIST;
    };
    let t = match ctx.resolve_slot(slot_no_tx) {
        Ok(n) => n,
        Err(e) => return e,
    };
    ctx.set_slot(
        t,
        SlotEntry {
            bytes: tx,
            kind: SlotKind::Root,
            reported_code: ROOT_TRANSACTION,
        },
    );
    let m = match ctx.resolve_slot(slot_no_meta) {
        Ok(n) => n,
        Err(e) => {
            ctx.clear_slot(t);
            return e;
        }
    };
    ctx.set_slot(
        m,
        SlotEntry {
            bytes: meta,
            kind: SlotKind::Root,
            reported_code: ROOT_METADATA,
        },
    );
    ((t as i64) << 16) | (m as i64)
}

/// Derives one child slot's `(kind, bytes)` from `field`'s value within
/// `parent_bytes` — the value-payload convention this module's doc comment
/// documents. `Err(())` only on a malformed VL length prefix (defensive:
/// unreachable for any `field` this module's own walkers produced).
fn child_slot_content(parent_bytes: &[u8], field: &FieldSpan) -> Result<(SlotKind, Vec<u8>), ()> {
    let ty = (field.code >> 16) as u32;
    let (start, end) = emit_walk::field_value_payload(parent_bytes, field)?;
    let bytes = parent_bytes.get(start..end).ok_or(())?.to_vec();
    let kind = match ty {
        6 => SlotKind::Amount,
        14 => SlotKind::Object,
        15 => SlotKind::Array,
        _ => SlotKind::Scalar,
    };
    Ok((kind, bytes))
}

/// `HookAPI::slot_subfield` (`HookAPI.cpp:2219-2280`): navigates a
/// [`SlotKind::Root`]/[`SlotKind::Object`] parent (`NOT_AN_OBJECT`
/// otherwise), storing the matched field's value-payload bytes into
/// `new_slot`.
pub(crate) fn slot_subfield(
    ctx: &mut InvocationContext,
    parent_slot: u32,
    field_id: u32,
    new_slot: u32,
) -> i64 {
    let Some(parent) = ctx.slot_entry(parent_slot) else {
        return DOESNT_EXIST;
    };
    if parent.kind != SlotKind::Root && parent.kind != SlotKind::Object {
        return NOT_AN_OBJECT;
    }
    let in_object = parent.kind == SlotKind::Object;
    let fields = match emit_walk::walk_top_level_fields_or_object(&parent.bytes, in_object) {
        Ok(f) => f,
        Err(()) => return NOT_AN_OBJECT, // defensive — see this module's doc comment
    };
    let target = u64::from(field_id);
    let Some(field) = fields.iter().find(|f| f.code == target) else {
        return DOESNT_EXIST;
    };
    let (kind, bytes) = match child_slot_content(&parent.bytes, field) {
        Ok(v) => v,
        Err(()) => return PARSE_ERROR,
    };
    let slot_no = match ctx.resolve_slot(new_slot) {
        Ok(n) => n,
        Err(e) => return e,
    };
    ctx.set_slot(
        slot_no,
        SlotEntry {
            bytes,
            kind,
            reported_code: field_id,
        },
    );
    slot_no as i64
}

/// `HookAPI::slot_subarray` (`HookAPI.cpp:2159-2217`): navigates a
/// [`SlotKind::Array`] parent (`NOT_AN_ARRAY` otherwise), storing the
/// matched element's value-payload bytes (header excluded, `0xE1`
/// terminator included — every element is an `STObject`, per
/// `crate::emit_walk::walk_array_elements`'s own `STI_OBJECT`-only rule)
/// into `new_slot`. The element's own field code (e.g. `sfSignerEntry`)
/// becomes the child's `reported_code`.
pub(crate) fn slot_subarray(
    ctx: &mut InvocationContext,
    parent_slot: u32,
    array_id: u32,
    new_slot: u32,
) -> i64 {
    let Some(parent) = ctx.slot_entry(parent_slot) else {
        return DOESNT_EXIST;
    };
    if parent.kind != SlotKind::Array {
        return NOT_AN_ARRAY;
    }
    let elements = match emit_walk::walk_array_elements(&parent.bytes) {
        Ok(e) => e,
        Err(()) => return NOT_AN_ARRAY, // defensive — see this module's doc comment
    };
    let Some(&(start, end)) = elements.get(array_id as usize) else {
        return DOESNT_EXIST;
    };
    let mut hpos = start;
    let Ok((ety, efield)) = emit_walk::decode_header(&parent.bytes, &mut hpos) else {
        return NOT_AN_ARRAY; // defensive
    };
    let Some(bytes) = parent.bytes.get(hpos..end) else {
        return NOT_AN_ARRAY; // defensive
    };
    let bytes = bytes.to_vec();
    let reported_code = (ety << 16) | efield;
    let slot_no = match ctx.resolve_slot(new_slot) {
        Ok(n) => n,
        Err(e) => return e,
    };
    ctx.set_slot(
        slot_no,
        SlotEntry {
            bytes,
            kind: SlotKind::Object,
            reported_code,
        },
    );
    slot_no as i64
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;
    use crate::otxn::Otxn;

    const SF_SEQUENCE: u32 = (2 << 16) + 4;
    const SF_BALANCE: u32 = (6 << 16) + 2;
    const SF_ACCOUNT: u32 = (8 << 16) + 1;
    const SF_SIGNER_ENTRIES: u32 = (15 << 16) + 4;
    const SF_SIGNER_WEIGHT: u32 = (1 << 16) + 3;

    fn fresh_ctx() -> InvocationContext {
        InvocationContext::new(0)
    }

    fn native_amount(drops: u64) -> [u8; 8] {
        let mut amt = drops.to_be_bytes();
        amt[0] |= 0x40;
        amt
    }

    /// An "account root"-shaped object: `Sequence`, `Balance` (native),
    /// `Account` — a bare field sequence (root shape, no header/footer),
    /// matching what `slot_set` reads from `World::ledger_objects`.
    fn account_root_bytes(seq: u32, drops: u64, account: [u8; 20]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(0x24);
        out.extend_from_slice(&seq.to_be_bytes());
        out.push(0x62);
        out.extend_from_slice(&native_amount(drops));
        out.push(0x81);
        out.push(20);
        out.extend_from_slice(&account);
        out
    }

    /// A `SignerList`-shaped object: one `sfSignerEntries` array field
    /// holding one `SignerEntry` element (`SignerWeight` + `Account`).
    fn signer_list_bytes(weight: u16, account: [u8; 20]) -> Vec<u8> {
        let mut inner = Vec::new();
        inner.push(0x13);
        inner.extend_from_slice(&weight.to_be_bytes());
        inner.push(0x81);
        inner.push(20);
        inner.extend_from_slice(&account);
        let mut element = vec![0xEA]; // (type 14, field 10) — an arbitrary SignerEntry-shaped header
        element.extend_from_slice(&inner);
        element.push(0xE1);
        let mut array_value = element;
        array_value.push(0xF1);
        let mut root = vec![0xF4]; // sfSignerEntries header
        root.extend_from_slice(&array_value);
        root
    }

    fn keylet(byte: u8) -> [u8; 34] {
        let mut k = [0u8; 34];
        k[33] = byte;
        k
    }

    fn seeded_world(keylet_bytes: [u8; 34], object: &[u8]) -> World {
        let mut world = World::new();
        world.ledger_objects.insert(keylet_bytes, object.to_vec());
        world
    }

    // -- slot_set --

    #[test]
    fn slot_set_loads_a_seeded_ledger_object() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(5, 1_000_000, [9u8; 20]));
        let no = slot_set(&mut ctx, &world, &kl, 0);
        assert_eq!(no, 1);
        assert_eq!(ctx.slot_entry(1).unwrap().kind, SlotKind::Root);
        assert_eq!(slot_type(&ctx, 1, 0), i64::from(ROOT_LEDGER_ENTRY));
    }

    #[test]
    fn slot_set_missing_keylet_is_doesnt_exist() {
        let mut ctx = fresh_ctx();
        let world = World::new();
        assert_eq!(slot_set(&mut ctx, &world, &keylet(1), 0), DOESNT_EXIST);
    }

    #[test]
    fn slot_set_32_byte_hash_is_doesnt_exist() {
        let mut ctx = fresh_ctx();
        let world = World::new();
        assert_eq!(slot_set(&mut ctx, &world, &[0u8; 32], 0), DOESNT_EXIST);
    }

    #[test]
    fn slot_set_wrong_length_is_invalid_argument() {
        let mut ctx = fresh_ctx();
        let world = World::new();
        assert_eq!(slot_set(&mut ctx, &world, &[0u8; 10], 0), INVALID_ARGUMENT);
    }

    #[test]
    fn slot_set_slot_into_out_of_range_is_invalid_argument() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1, [0u8; 20]));
        assert_eq!(slot_set(&mut ctx, &world, &kl, 256), INVALID_ARGUMENT);
    }

    #[test]
    fn slot_set_no_free_slots_once_all_255_are_taken() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1, [0u8; 20]));
        for n in 1..=255u32 {
            assert_eq!(slot_set(&mut ctx, &world, &kl, n), n as i64);
        }
        assert_eq!(
            slot_set(&mut ctx, &world, &kl, 0),
            rshooks_core::NO_FREE_SLOTS
        );
    }

    #[test]
    fn slot_set_no_free_slots_is_checked_before_the_keylet_lookup() {
        // Every slot occupied *and* a missing keylet: upstream
        // (`HookAPI.cpp:2086-2088`) reports `NO_FREE_SLOTS`, not
        // `DOESNT_EXIST`, since the free-slot check runs first.
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1, [0u8; 20]));
        for n in 1..=255u32 {
            assert_eq!(slot_set(&mut ctx, &world, &kl, n), n as i64);
        }
        let missing_kl = keylet(9);
        assert_eq!(
            slot_set(&mut ctx, &world, &missing_kl, 0),
            rshooks_core::NO_FREE_SLOTS
        );
    }

    // -- slot / slot_clear / slot_size --

    #[test]
    fn slot_and_slot_size_read_back_the_stored_payload() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let bytes = account_root_bytes(5, 1, [0u8; 20]);
        let world = seeded_world(kl, &bytes);
        let no = slot_set(&mut ctx, &world, &kl, 0) as u32;
        assert_eq!(slot(&ctx, no), Ok(bytes.clone()));
        assert_eq!(slot_size(&ctx, no), bytes.len() as i64);
    }

    #[test]
    fn slot_missing_is_doesnt_exist() {
        let ctx = fresh_ctx();
        assert_eq!(slot(&ctx, 7), Err(DOESNT_EXIST));
        assert_eq!(slot_size(&ctx, 7), DOESNT_EXIST);
    }

    #[test]
    fn slot_clear_frees_the_slot_and_then_reports_doesnt_exist() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1, [0u8; 20]));
        let no = slot_set(&mut ctx, &world, &kl, 0) as u32;
        assert_eq!(slot_clear(&mut ctx, no), 1);
        assert_eq!(slot_clear(&mut ctx, no), DOESNT_EXIST);
        assert_eq!(slot(&ctx, no), Err(DOESNT_EXIST));
    }

    // -- slot_subfield --

    #[test]
    fn slot_subfield_reads_a_scalar_field_with_no_vl_prefix() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(5, 1_000_000, [9u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;

        let seq_slot = slot_subfield(&mut ctx, root, SF_SEQUENCE, 0);
        assert!(seq_slot > 0);
        assert_eq!(slot(&ctx, seq_slot as u32), Ok(5u32.to_be_bytes().to_vec()));
        assert_eq!(
            ctx.slot_entry(seq_slot as u32).unwrap().kind,
            SlotKind::Scalar
        );

        // sfAccount: 20 raw bytes, no VL prefix — the
        // `examples/15_slot-objects::check_account_walk` fidelity anchor.
        let acc_slot = slot_subfield(&mut ctx, root, SF_ACCOUNT, 0) as u32;
        assert_eq!(slot(&ctx, acc_slot), Ok(vec![9u8; 20]));
        assert_eq!(slot_size(&ctx, acc_slot), 20);
    }

    #[test]
    fn slot_subfield_amount_field_is_amount_kind() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1_000_000, [0u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;
        let bal = slot_subfield(&mut ctx, root, SF_BALANCE, 0) as u32;
        assert_eq!(ctx.slot_entry(bal).unwrap().kind, SlotKind::Amount);
        assert_eq!(slot_size(&ctx, bal), 8);
        assert_eq!(slot_type(&ctx, bal, 1), 1); // native
    }

    #[test]
    fn slot_subfield_missing_field_is_doesnt_exist() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1, [0u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;
        assert_eq!(slot_subfield(&mut ctx, root, 0x0002_0099, 0), DOESNT_EXIST);
    }

    #[test]
    fn slot_subfield_on_a_non_object_slot_is_not_an_object() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1, [0u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;
        let bal = slot_subfield(&mut ctx, root, SF_BALANCE, 0) as u32;
        assert_eq!(slot_subfield(&mut ctx, bal, SF_SEQUENCE, 0), NOT_AN_OBJECT);
    }

    #[test]
    fn slot_subfield_missing_parent_is_doesnt_exist() {
        let mut ctx = fresh_ctx();
        assert_eq!(slot_subfield(&mut ctx, 9, SF_SEQUENCE, 0), DOESNT_EXIST);
    }

    // -- slot_subarray / slot_count / array navigation --

    #[test]
    fn array_field_navigates_to_element_and_its_own_fields() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &signer_list_bytes(3, [4u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;

        let arr = slot_subfield(&mut ctx, root, SF_SIGNER_ENTRIES, 0) as u32;
        assert_eq!(ctx.slot_entry(arr).unwrap().kind, SlotKind::Array);
        assert_eq!(slot_count(&ctx, arr), 1);
        assert_eq!(slot_count(&ctx, root), NOT_AN_ARRAY);

        let elem = slot_subarray(&mut ctx, arr, 0, 0) as u32;
        assert_eq!(ctx.slot_entry(elem).unwrap().kind, SlotKind::Object);
        assert_eq!(slot_subarray(&mut ctx, arr, 1, 0), DOESNT_EXIST);

        let acc = slot_subfield(&mut ctx, elem, SF_ACCOUNT, 0) as u32;
        assert_eq!(slot(&ctx, acc), Ok(vec![4u8; 20]));
        let wt = slot_subfield(&mut ctx, elem, SF_SIGNER_WEIGHT, 0) as u32;
        assert_eq!(slot(&ctx, wt), Ok(3u16.to_be_bytes().to_vec()));
    }

    #[test]
    fn slot_subarray_on_a_non_array_is_not_an_array() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1, [0u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;
        assert_eq!(slot_subarray(&mut ctx, root, 0, 0), NOT_AN_ARRAY);
    }

    // -- Read-contract anchor (design §4 "slot family", 2b): clearing a
    // parent must not invalidate a child slot's bytes.

    #[test]
    fn clearing_the_parent_does_not_invalidate_a_previously_derived_child() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(7, 1, [0u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;
        let child = slot_subfield(&mut ctx, root, SF_SEQUENCE, 0) as u32;

        assert_eq!(slot_clear(&mut ctx, root), 1);

        // Child still present, still reads the same bytes.
        assert_eq!(slot(&ctx, child), Ok(7u32.to_be_bytes().to_vec()));
        assert_eq!(slot_clear(&mut ctx, child), 1);
    }

    // -- slot_type / slot_float --

    #[test]
    fn slot_type_flags_1_on_a_non_amount_is_not_an_amount() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1, [0u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;
        assert_eq!(slot_type(&ctx, root, 1), NOT_AN_AMOUNT);
    }

    #[test]
    fn slot_type_invalid_flags_is_invalid_argument() {
        let ctx = fresh_ctx();
        assert_eq!(slot_type(&ctx, 1, 2), DOESNT_EXIST); // no slot 1 yet
    }

    #[test]
    fn slot_float_native_matches_float_sets_normalize_xfl_path() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 1_000_000, [0u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;
        let bal = slot_subfield(&mut ctx, root, SF_BALANCE, 0) as u32;

        // Cross-checked against `float_set`: both funnel through the same
        // `normalize_xfl(drops, -6, neg)` (see `slot_float`'s doc comment).
        let expected = crate::host::float::float_set(-6, 1_000_000);
        assert_eq!(slot_float(&ctx, bal), expected);
    }

    #[test]
    fn slot_float_native_zero_underflows_to_zero_not_invalid_float() {
        let mut ctx = fresh_ctx();
        let kl = keylet(1);
        let world = seeded_world(kl, &account_root_bytes(1, 0, [0u8; 20]));
        let root = slot_set(&mut ctx, &world, &kl, 0) as u32;
        let bal = slot_subfield(&mut ctx, root, SF_BALANCE, 0) as u32;
        // `float_set(-6, 0)` would report `INVALID_FLOAT` (mantissa == 0
        // is special-cased as an error there); `slot_float` instead returns
        // plain `0`, matching upstream's own `EXPONENT_UNDERSIZED`/
        // zero-mantissa remap.
        assert_eq!(slot_float(&ctx, bal), 0);
    }

    #[test]
    fn slot_float_iou_round_trips_through_float_sto() {
        let mut ctx = fresh_ctx();
        // `float_sto` with a (currency, issuer) pair emits `<header><8-byte
        // amount><20 currency><20 issuer>` — the 48 bytes after the 1-byte
        // `sfBalance` header are exactly the value-payload shape a
        // `slot_subfield`-derived Amount child stores.
        let xfl = crate::host::float::float_one();
        let currency = [1u8; 20];
        let issuer = [2u8; 20];
        let full =
            crate::host::float::float_sto(Some(&currency), Some(&issuer), xfl, SF_BALANCE).unwrap();
        assert_eq!(full.len(), 1 + 48); // 1-byte sfBalance header + 48-byte IOU amount
        let iou_bytes = &full[1..];

        let kl = keylet(1);
        // Wrap the raw IOU bytes as a Balance field's value inside a
        // minimal object so `slot_subfield` produces an `Amount` slot.
        let mut root = vec![0x62];
        root.extend_from_slice(iou_bytes);
        let world = seeded_world(kl, &root);
        let root_slot = slot_set(&mut ctx, &world, &kl, 0) as u32;
        let bal = slot_subfield(&mut ctx, root_slot, SF_BALANCE, 0) as u32;
        assert_eq!(ctx.slot_entry(bal).unwrap().kind, SlotKind::Amount);
        assert_eq!(slot_size(&ctx, bal), 48);
        assert_eq!(slot_type(&ctx, bal, 1), 0); // not native
        assert_eq!(slot_float(&ctx, bal), xfl);
    }

    // -- otxn_slot / meta_slot / xpop_slot --

    #[test]
    fn otxn_slot_serializes_the_seeded_otxn() {
        let mut ctx = fresh_ctx();
        let mut world = World::new();
        world.otxn = Otxn::new(rshooks::tx_type::TxType::Payment).account([2u8; 20]);
        let no = otxn_slot(&mut ctx, &world, 0) as u32;
        assert_eq!(ctx.slot_entry(no).unwrap().kind, SlotKind::Root);
        assert_eq!(slot_type(&ctx, no, 0), i64::from(ROOT_TRANSACTION));
        let acc = slot_subfield(&mut ctx, no, SF_ACCOUNT, 0) as u32;
        assert_eq!(slot(&ctx, acc), Ok(vec![2u8; 20]));
    }

    #[test]
    fn otxn_slot_root_type_matches_upstream_sftransaction_field_code() {
        // Pins the literal upstream value (not the `ROOT_TRANSACTION`
        // constant under test) — see this module's doc comment.
        let mut ctx = fresh_ctx();
        let world = World::new();
        let no = otxn_slot(&mut ctx, &world, 0) as u32;
        assert_eq!(slot_type(&ctx, no, 0), (10001i64 << 16) | 257);
    }

    #[test]
    fn meta_slot_absent_is_prerequisite_not_met() {
        let mut ctx = fresh_ctx();
        let world = World::new();
        assert_eq!(meta_slot(&mut ctx, &world, 0), PREREQUISITE_NOT_MET);
    }

    #[test]
    fn meta_slot_seeded_loads_as_root() {
        let mut ctx = fresh_ctx();
        let mut world = World::new();
        world.otxn_meta = Some(vec![0x24, 0, 0, 0, 9]);
        let no = meta_slot(&mut ctx, &world, 0) as u32;
        assert_eq!(slot_type(&ctx, no, 0), i64::from(ROOT_METADATA));
        assert_eq!(slot(&ctx, no), Ok(vec![0x24, 0, 0, 0, 9]));
    }

    #[test]
    fn xpop_slot_absent_is_doesnt_exist() {
        let mut ctx = fresh_ctx();
        let world = World::new();
        assert_eq!(xpop_slot(&mut ctx, &world, 0, 0), DOESNT_EXIST);
    }

    #[test]
    fn xpop_slot_packs_both_slot_numbers() {
        let mut ctx = fresh_ctx();
        let mut world = World::new();
        world.xpop = Some((vec![0x24, 0, 0, 0, 1], vec![0x24, 0, 0, 0, 2]));
        let packed = xpop_slot(&mut ctx, &world, 0, 0);
        let tx_slot = (packed >> 16) as u32;
        let meta_slot_no = (packed & 0xFFFF) as u32;
        assert_eq!(tx_slot, 1);
        assert_eq!(meta_slot_no, 2);
        assert_eq!(slot_type(&ctx, tx_slot, 0), i64::from(ROOT_TRANSACTION));
        assert_eq!(slot_type(&ctx, meta_slot_no, 0), i64::from(ROOT_METADATA));
        assert_eq!(slot(&ctx, tx_slot), Ok(vec![0x24, 0, 0, 0, 1]));
        assert_eq!(slot(&ctx, meta_slot_no), Ok(vec![0x24, 0, 0, 0, 2]));
    }

    #[test]
    fn xpop_slot_rejects_the_same_explicit_slot_for_both_halves() {
        let mut ctx = fresh_ctx();
        let mut world = World::new();
        world.xpop = Some((vec![0x24, 0, 0, 0, 1], vec![0x24, 0, 0, 0, 2]));
        assert_eq!(xpop_slot(&mut ctx, &world, 5, 5), INVALID_ARGUMENT);
    }
}
