//! Off-chain unit tests for the `xfl-math` example, driven through
//! `TestEnv::invoke` against the real `XflMath` chain — no wasm build, no
//! node. Mirrors `examples/02_state-counter/tests/counter.rs`'s pattern.
//!
//! # Why this only exercises the rollback path (for now)
//!
//! `XflMath::main` (`src/lib.rs`) reaches its `Amount` via
//! `SlotObject::from_otxn()`, i.e. the numbered-slot host API
//! (`otxn_slot`/`slot_subfield`/`slot_float`). That family is **not yet
//! implemented** in `rshooks-testenv`'s mock backend — it lands in a later
//! stage (`.claude/design/TESTENV_PHASE2_DESIGN.md` stage P2-D; this test
//! file was added in P2-B, which covers `float_*` only). Every
//! `HostBackend::otxn_slot`/`slot_subfield`/`slot_float` call still falls
//! through to the trait's own `NOT_IMPLEMENTED` default, so
//! `SlotObject::from_otxn()` fails at the very first step and the hook
//! rolls back with `XflMathError::OtxnSlotFailed` — deterministically,
//! regardless of what the seeded originating transaction actually
//! contains, since execution never gets far enough to read it.
//!
//! Per this stage's instructions ("do not change the hook source; if the
//! entry's assertions can't be observed externally, assert exit
//! code/type and traces"), the tests below assert exactly that: the real
//! entry really does run under `TestEnv`, and its externally-observable
//! behavior (exit type, exit code, message, and the absence of any state
//! or trace side effects) is pinned. Once P2-D lands `otxn_slot`/
//! `slot_subfield`/`slot_float`, this file should gain the full
//! accept-path coverage (percentage share computed, compared against the
//! minimum, compounded, and validated against `remaining`) the design
//! doc's verification bar calls for.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks_testenv::prelude::*;
use xfl_math::{XflMath, XflMathError};

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(
            Otxn::new(TxType::Payment)
                .account([2u8; 20])
                .destination([1u8; 20])
                .amount_drops(1_000_000_000),
        )
}

#[test]
fn entry_runs_and_rolls_back_at_otxn_slot_not_yet_implemented() {
    let exit = env().invoke::<XflMath>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, XflMathError::OtxnSlotFailed.code());
    assert_eq!(exit.msg, b"xfl-math: otxn_slot failed");
}

#[test]
fn rollback_leaves_no_trace_or_state_side_effects() {
    let e = env();
    let _ = e.invoke::<XflMath>(0);
    assert!(e.traces().is_empty());
    // The hook never reaches its own `state`/`state_set` calls (it has
    // none), and the harness itself records nothing on a pre-slot
    // rollback -- this assertion exists so a future change that makes the
    // hook progress further is caught by a diff here, not silently.
    assert_eq!(e.state_typed::<u64>(b"counter"), None);
}

#[test]
fn behavior_is_independent_of_the_seeded_amount() {
    // `otxn_slot` fails before the hook ever inspects the seeded
    // transaction's `Amount` field, so the outcome is identical whether
    // that field is present, absent, zero, or an IOU amount.
    let no_amount = TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Payment).account([2u8; 20]));
    let exit = no_amount.invoke::<XflMath>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, XflMathError::OtxnSlotFailed.code());
}
