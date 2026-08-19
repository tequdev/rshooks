//! Off-chain unit tests for the `state-counter` example, driven through
//! `TestEnv::invoke` against the real `StateCounter` chain — no wasm build,
//! no node. See `book/src/testing/unit-tests.md` for the full walkthrough
//! this file mirrors.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks_testenv::prelude::*;
use state_counter::{StateCounter, StateCounterError};

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
}

#[test]
fn first_invoke_counts_to_one() {
    let env = env();
    let exit = env.invoke::<StateCounter>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, 1);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(1));
}

#[test]
fn counter_persists_across_invocations() {
    let env = env();
    env.invoke::<StateCounter>(0);
    env.invoke::<StateCounter>(0);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(2));
}

#[test]
fn state_set_failure_rolls_back_without_persisting() {
    // Cap the value size below the 8-byte `u64` write this hook always
    // attempts, forcing `state_set` to fail (`TOO_BIG`) so the hook's own
    // rollback path runs.
    let env = env().max_state_value_len(4);
    let exit = env.invoke::<StateCounter>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, StateCounterError::StateSetFailed.code());
    assert_eq!(env.state_typed::<u64>(b"counter"), None);
}
