//! Off-chain unit tests for the `emit-txn` example, driven through
//! `TestEnv::invoke` against the real `EmitTxn` chain — no wasm build, no
//! node. `src/lib.rs` carries an equivalent in-crate `#[cfg(test)]`
//! variant; see `book/src/testing/unit-tests.md` for both layouts
//! documented side by side.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use emit_txn::EmitTxn;
use rshooks_testenv::prelude::*;

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
}

#[test]
fn emit_accepts_and_records_one_payment() {
    let env = env();
    let exit = env.invoke::<EmitTxn>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Payment));
    assert!(!emitted[0].blob().is_empty());
}

#[test]
fn each_invocation_emits_its_own_payment() {
    let env = env();
    env.invoke::<EmitTxn>(0);
    env.invoke::<EmitTxn>(0);
    assert_eq!(env.emitted().len(), 2);
}

// -- invoke_cbak (P2-E — `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "cbak
// execution"). `EmitTxn`'s `#[cbak(0)]` body (`src/lib.rs`) is
// `fn cbak(&self) -> HookResult { Ok(Accept::from_code(0)) }` — it unconditionally accepts,
// reading neither the wasm argument nor the callback otxn. Its real
// behavior to assert is exactly that: `invoke_cbak` reaches `Accept`
// regardless of `CbakOutcome::Success`/`Failure`, and leaves the
// surrounding `TestEnv` usable afterward (the callback's otxn swap is
// invocation-scoped and must not leak into a later `invoke`).

#[test]
fn invoke_cbak_success_reaches_the_real_accept_path() {
    let env = env();
    let exit = env.invoke::<EmitTxn>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<EmitTxn>(0, CbakOutcome::Success(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}

#[test]
fn invoke_cbak_failure_still_accepts_because_the_cbak_body_ignores_the_outcome() {
    let env = env();
    let _ = env.invoke::<EmitTxn>(0);
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<EmitTxn>(0, CbakOutcome::Failure(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}

#[test]
fn invoke_cbak_restores_the_original_otxn_for_a_later_invoke() {
    let env = env();
    let _ = env.invoke::<EmitTxn>(0);
    let txn = env.emitted()[0].clone();

    let _ = env.invoke_cbak::<EmitTxn>(0, CbakOutcome::Success(txn));

    // The seeded otxn (an Invoke transaction from [2u8; 20]) must still be
    // in effect for an ordinary invoke after the callback — not the
    // Payment the callback ran against.
    let exit = env.invoke::<EmitTxn>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert_eq!(env.emitted().len(), 2);
}
