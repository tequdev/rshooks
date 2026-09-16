//! Regression test: an emitted transaction that carries NOP (`0x99`) bytes
//! inside a *nested* `STObject` field must still navigate correctly from a
//! `#[cbak]` body via `otxn_slot` + `slot_subfield`, end to end — `emit`
//! canonicalizes the nested-NOP-padded blob before storing it
//! (`crate::backend::Backend::emit`), and the stored, already-canonical
//! bytes are what a `#[cbak]`'s otxn reconstruction (`crate::otxn::
//! from_emitted`/`deserialize`) then navigates. This test asserts on that
//! stored blob (`!txn.blob().contains(&0x99)`) and on the callback's own
//! successful navigation; it does not by itself exercise `deserialize`'s
//! *own* canonicalization step in isolation on a raw, not-yet-canonical
//! blob — `crate::otxn::tests::deserialize_canonicalizes_a_nested_nop_
//! supplied_directly` is the narrower unit test that does, feeding a
//! hand-built nested-NOP blob straight into `deserialize`. Hand-rolled
//! `NativeEntry` table here, matching `tests/invoke_cbak.rs`'s own
//! pattern, so this does not depend on `txn_template!`'s `optional`/
//! `any_amount` NOP-padded field kinds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::decl::{HookChainEntries, NativeEntry};
use rshooks_testenv::prelude::*;

/// An arbitrary top-level `STObject` field code (type 14, field 14) this
/// test nests a scalar payload field inside. Nothing in the emission
/// grammar restricts which object-typed field codes may appear beyond
/// `sfEmitDetails`'s own reserved one, so an otherwise-unused code is safe
/// to use here.
const CUSTOM_OBJECT_FIELD: u32 = (14 << 16) | 14;
/// `sfSequence`'s code (type 2, field 4) — reused *inside* the custom
/// nested object purely as a convenient, already-typed scalar field to
/// write and read back. Nesting scopes field codes per container, so this
/// does not collide with the top-level `Sequence` `prepare` overwrites.
const INNER_SEQUENCE_FIELD: u32 = (2 << 16) | 4;
/// The value written into [`INNER_SEQUENCE_FIELD`], read back in the
/// callback.
const INNER_SEQUENCE_VALUE: u32 = 7;

/// Reserves an emission slot, hand-builds a template with one custom
/// nested `STObject` field (holding one scalar field), `prepare`s it, then
/// splices one NOP byte inside that nested object's body — after its own
/// scalar field, before its own `0xE1` terminator — before emitting the
/// result directly via `emit`. `prepare`'s own top-level field
/// manipulation never touches the interior of an opaque nested field, so
/// the nested object's bytes reach the returned buffer untouched; the
/// splice happens *after* `prepare` returns, mirroring how a
/// `txn_template!`-baked NOP-padded field actually reaches the host.
fn emit_nop_padded_nested_object(_r: u32) -> i64 {
    let _ = rshooks::api::etxn::etxn_reserve(1);

    // TransactionType = ttPAYMENT(0), plus `sfAmount`/`sfDestination` --
    // both `presence: "required"` for Payment in `protocol_formats.json`,
    // so `validate_emit_blob`'s required-field check accepts the result --
    // then one custom nested STObject field (CUSTOM_OBJECT_FIELD) holding
    // one scalar field (INNER_SEQUENCE_FIELD).
    let mut template = [0u8; 41];
    template[0] = 0x12; // TransactionType header
    template[1] = 0x00;
    template[2] = 0x00; // TransactionType = 0 (ttPAYMENT)
    template[3] = 0x61; // sfAmount header: (6 << 4) | 1
    template[4..12].copy_from_slice(&0x4000_0000_0000_0001u64.to_be_bytes()); // native, 1 drop
    template[12] = 0x83; // sfDestination header: (8 << 4) | 3
    template[13] = 20; // AccountID VL length prefix
    template[14..34].copy_from_slice(&[2u8; 20]);
    template[34] = 0xEE; // CUSTOM_OBJECT_FIELD header: (14 << 4) | 14
    template[35] = 0x24; // INNER_SEQUENCE_FIELD header: (2 << 4) | 4
    template[36..40].copy_from_slice(&INNER_SEQUENCE_VALUE.to_be_bytes());
    template[40] = 0xE1; // nested object terminator

    let mut prepared = [0u8; 256];
    let n = rshooks::api::etxn::prepare(&mut prepared, &template).expect("prepare");
    let base = &prepared[..n];

    // Locate the nested object's own bytes (unmodified by `prepare`), then
    // splice one NOP right before its terminator.
    let marker: [u8; 7] = [0xEE, 0x24, 0, 0, 0, 7, 0xE1];
    let pos = base
        .windows(marker.len())
        .position(|w| w == marker)
        .expect("nested object bytes not found");
    let mut spliced = std::vec::Vec::with_capacity(n + 1);
    spliced.extend_from_slice(&base[..pos + 6]); // header + inner field value, before 0xE1
    spliced.push(0x99); // NOP inside the nested object
    spliced.extend_from_slice(&base[pos + 6..]); // 0xE1 onward

    let mut hash = [0u8; 32];
    rshooks::api::etxn::emit(&mut hash, &spliced).expect("emit");

    rshooks::api::control::accept(b"emitted", 0);
}

/// Navigates `otxn_slot(0)` -> [`CUSTOM_OBJECT_FIELD`] -> [`INNER_SEQUENCE_FIELD`]
/// and checks the read-back value, rolling back with a distinct code at
/// whichever step fails. Success here proves the callback's otxn (built
/// via `crate::otxn::from_emitted`/`deserialize`) is NOP-free at the
/// nested level, not just the top level — real xahaud's callback otxn is
/// the emitted transaction's own canonical (re-serialized, NOP-free) form,
/// so `slot_subfield` navigating into it must never see a stray `0x99`.
fn navigate_nested_field_in_cbak(_r: u32) -> i64 {
    let root = match rshooks::api::otxn::otxn_slot(0) {
        Ok(n) => n,
        Err(_) => rshooks::api::control::rollback(b"otxn_slot failed", 1),
    };
    let nested = match rshooks::api::slot::slot_subfield(root, CUSTOM_OBJECT_FIELD, 0) {
        Ok(n) => n,
        Err(_) => rshooks::api::control::rollback(b"slot_subfield (nested) failed", 2),
    };
    let inner = match rshooks::api::slot::slot_subfield(nested, INNER_SEQUENCE_FIELD, 0) {
        Ok(n) => n,
        Err(_) => rshooks::api::control::rollback(b"slot_subfield (inner) failed", 3),
    };
    let mut buf = [0u8; 4];
    let read = match rshooks::api::slot::slot(&mut buf, inner) {
        Ok(n) => n,
        Err(_) => rshooks::api::control::rollback(b"slot read failed", 4),
    };
    if read != 4 || u32::from_be_bytes(buf) != INNER_SEQUENCE_VALUE {
        rshooks::api::control::rollback(b"unexpected value", 5);
    }
    rshooks::api::control::accept(b"cbak ok", 0);
}

struct Chain;
impl HookChainEntries for Chain {
    const ENTRIES: &'static [NativeEntry] = &[NativeEntry {
        index: 0,
        name: "emit_nop_padded_nested_object",
        hook: emit_nop_padded_nested_object,
        cbak: Some(navigate_nested_field_in_cbak),
        can_emit: None,
    }];
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20])
}

#[test]
fn cbak_navigates_a_nested_field_that_carried_a_nop() {
    let env = env();
    let exit = env.invoke::<Chain>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let txn = env.emitted()[0].clone();

    // The stored blob is already canonical (NOP-free) — `Backend::emit`
    // canonicalizes before storing (see `crate::emit_walk::canonicalize`'s
    // doc comment) — but the nested NOP was still genuinely present in the
    // bytes the hook passed to `emit`.
    assert!(!txn.blob().contains(&0x99));

    let cbak_exit = env.invoke_cbak::<Chain>(0, CbakOutcome::Success(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}
