//! Off-chain unit tests for the `sto-writer` example, driven through
//! `TestEnv::invoke` against the real `StoWriterRemit` chain — no wasm
//! build, no node. `src/lib.rs` carries an equivalent in-crate
//! `#[cfg(test)]` variant (which additionally covers `build_remit`/
//! `prepare_for_emit` directly, since those are private and only reachable
//! from an in-crate test) — see `book/src/testing/unit-tests.md` for both
//! layouts documented side by side.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks_testenv::prelude::*;
use sto_writer::StoWriterRemit;

const DEST: [u8; 20] = [3u8; 20];

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
        .hook_param(b"DEST", &DEST)
}

#[test]
fn accepts_and_emits_one_remit() {
    let env = env();
    let exit = env.invoke::<StoWriterRemit>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
    assert!(!emitted[0].blob().is_empty());
}

#[test]
fn each_invocation_emits_its_own_remit() {
    let env = env();
    env.invoke::<StoWriterRemit>(0);
    env.invoke::<StoWriterRemit>(0);
    assert_eq!(env.emitted().len(), 2);
}

#[test]
fn missing_destination_rolls_back_without_emitting() {
    let env = TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]));
    let exit = env.invoke::<StoWriterRemit>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(env.emitted().len(), 0);
}

#[test]
fn cur_and_issuer_parameters_add_a_second_amounts_entry() {
    let native_only = env();
    let native_exit = native_only.invoke::<StoWriterRemit>(0);
    assert_eq!(native_exit.exit, ExitType::Accept, "{native_exit:?}");
    let native_len = native_only.emitted()[0].blob().len();

    let with_issued = env()
        .hook_param(b"CUR", &[0u8; 20])
        .hook_param(b"ISSUER", &[4u8; 20]);
    let issued_exit = with_issued.invoke::<StoWriterRemit>(0);
    assert_eq!(issued_exit.exit, ExitType::Accept, "{issued_exit:?}");
    let issued_len = with_issued.emitted()[0].blob().len();

    assert!(issued_len > native_len);
}

// -- invoke_cbak (P2-E — `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "cbak
// execution"). `StoWriterRemit`'s `#[cbak(0)]` body (`src/lib.rs`) is
// `fn cbak(&self) -> HookResult { Ok(Accept::from_code(0)) }` — it
// unconditionally accepts, so the only real behavior to assert is exactly
// that: `invoke_cbak` reaches `Accept` regardless of `CbakOutcome`, and
// leaves the surrounding `TestEnv` usable afterward.

#[test]
fn invoke_cbak_success_reaches_the_real_accept_path() {
    let env = env();
    let exit = env.invoke::<StoWriterRemit>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<StoWriterRemit>(0, CbakOutcome::Success(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}

#[test]
fn invoke_cbak_failure_still_accepts_because_the_cbak_body_ignores_the_outcome() {
    let env = env();
    let _ = env.invoke::<StoWriterRemit>(0);
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<StoWriterRemit>(0, CbakOutcome::Failure(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
}
