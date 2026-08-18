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
