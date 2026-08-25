//! Off-chain unit tests for the `remit` example, driven through
//! `TestEnv::invoke` against the real `EmitRemit` chain — no wasm build, no
//! node. `src/lib.rs` carries an equivalent in-crate `#[cfg(test)]`
//! variant; see `examples/10_emit-txn/tests/emit.rs` and
//! `book/src/testing/unit-tests.md` for the same two-layout pattern
//! documented side by side.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use remit::EmitRemit;
use rshooks_testenv::prelude::*;

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
}

#[test]
fn emit_accepts_and_records_one_remit() {
    let env = env();
    let exit = env.invoke::<EmitRemit>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
    assert!(!emitted[0].blob().is_empty());
}

#[test]
fn each_invocation_emits_its_own_remit() {
    let env = env();
    env.invoke::<EmitRemit>(0);
    env.invoke::<EmitRemit>(0);
    assert_eq!(env.emitted().len(), 2);
}

#[test]
fn invoke_cbak_success_reaches_the_real_accept_path() {
    let env = env();
    let exit = env.invoke::<EmitRemit>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<EmitRemit>(0, CbakOutcome::Success(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}

#[test]
fn invoke_cbak_failure_still_accepts_because_the_cbak_body_ignores_the_outcome() {
    let env = env();
    let _ = env.invoke::<EmitRemit>(0);
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<EmitRemit>(0, CbakOutcome::Failure(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}
