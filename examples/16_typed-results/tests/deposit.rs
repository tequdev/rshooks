//! Off-chain unit tests for the `typed-results` example, driven through
//! `TestEnv::invoke` against the real `TypedResults` chain — no wasm build,
//! no node. Covers both entries: the typed `deposit` (`Ok`/`?`-rollback
//! paths, and that the msg-clause message from `DepositError` reaches
//! `HookExit.msg` byte-for-byte) and the raw-style `reset`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks_testenv::prelude::*;
use typed_results::{DepositError, TypedResults};

fn env_with_amount(amount: u64) -> TestEnv {
    TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Invoke)
            .account([2u8; 20])
            .param(b"AMT", &amount.to_be_bytes()),
    )
}

#[test]
fn deposit_accepts_and_persists_the_running_total() {
    let env = env_with_amount(7);
    let exit = env.invoke::<TypedResults>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert_eq!(exit.code, 7);
    assert_eq!(exit.msg, b"typed-results: deposited");
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(7));
}

#[test]
fn deposit_sums_across_invocations() {
    let env = env_with_amount(3);
    env.invoke::<TypedResults>(0);
    env.invoke::<TypedResults>(0);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(6));
}

#[test]
fn missing_amount_rolls_back_with_the_msg_clause_text() {
    // No `AMT` param configured: `get_required()` fails, `?` propagates
    // `DepositError::BadAmount` into `HookResult`'s `Err` side, and
    // `EntryReturn::finish` calls `rollback` with the `hook_errors!`
    // msg-clause bytes and the variant's code — proving both travel through
    // the `From<DepositError> for Rollback` conversion intact.
    let env = TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]));
    let exit = env.invoke::<TypedResults>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, DepositError::BadAmount.code());
    assert_eq!(exit.msg, b"typed-results: bad AMT parameter");
    assert_eq!(env.state_typed::<u64>(b"counter"), None);
}

#[test]
fn state_set_failure_rolls_back_without_persisting() {
    // Cap the value size below the 8-byte `u64` write `bump_counter`
    // attempts, forcing `state_set` to fail so the `?` rollback path runs.
    let env = env_with_amount(1).max_state_value_len(4);
    let exit = env.invoke::<TypedResults>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, DepositError::StateSetFailed.code());
    assert_eq!(exit.msg, b"typed-results: state_set failed");
    assert_eq!(env.state_typed::<u64>(b"counter"), None);
}

#[test]
fn reset_zeroes_a_nonzero_counter() {
    let env = env_with_amount(9);
    env.invoke::<TypedResults>(0);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(9));

    let exit = env.invoke::<TypedResults>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert_eq!(exit.msg, b"typed-results: reset");
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(0));
}
