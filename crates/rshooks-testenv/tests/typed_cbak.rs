//! Runtime coverage for a typed (`HookResult`-returning) `#[cbak]` driven
//! through `TestEnv::invoke_cbak`: proves a typed cbak's `Ok(Accept)`/
//! `Err(Rollback)` really reach `HookExit` (message bytes included) once
//! `EntryReturn::finish` converts them into the real `accept`/`rollback`
//! host calls (`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` §1.3).
//!
//! Two entries, both with a typed `HookResult` hook *and* cbak: index 0's
//! cbak always accepts, index 1's always rejects. A `#[cbak(i)]`'s
//! `fn(&self)` never sees `CbakOutcome::Success`/`Failure` (the
//! macro-generated wrapper calls it as `Struct::fn(&Struct)`, discarding
//! `_reserved`), so both variants are driven against both cbaks to prove
//! the typed exit is what reaches `HookExit`, not which `CbakOutcome`
//! produced it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    missing_docs
)]

use rshooks::api::etxn;
use rshooks::exit::{Accept, HookResult, Rollback};
use rshooks::hooks;
use rshooks_testenv::prelude::*;

/// Reserves one emission slot and emits a minimal `ttPAYMENT` blob
/// (`TransactionType = 0` -> wire bytes `[0x12, 0x00, 0x00]`) — enough for
/// `TestEnv::invoke_cbak` to parse an otxn back out of it.
fn emit_minimal_payment() {
    let _ = etxn::etxn_reserve(1);
    let template: [u8; 3] = [0x12, 0x00, 0x00];
    let mut prepared = [0u8; 256];
    let n = etxn::prepare(&mut prepared, &template).expect("prepare");
    let mut hash = [0u8; 32];
    etxn::emit(&mut hash, &prepared[..n]).expect("emit");
}

#[hooks]
pub struct Chain;

#[hooks]
impl Chain {
    /// Emits, then accepts through the typed path.
    #[hook(0, on = [Invoke], can_emit = [Payment])]
    fn main(&self) -> HookResult {
        emit_minimal_payment();
        Ok(Accept::new(b"emitted", 0))
    }

    /// Always accepts through the typed path — proves a typed cbak's
    /// `Ok(Accept)` reaches `HookExit` with its message intact.
    #[cbak(0)]
    fn accept_cbak(&self) -> HookResult {
        Ok(Accept::new(b"cbak-accept", 7))
    }

    /// Second entry, same emit — paired with a cbak that always rejects.
    #[hook(1, on = [Invoke], can_emit = [Payment])]
    fn main_2(&self) -> HookResult {
        emit_minimal_payment();
        Ok(Accept::new(b"emitted", 0))
    }

    /// Always rejects through the typed path — proves a typed cbak's
    /// `Err(Rollback)` reaches `HookExit` with its message intact.
    #[cbak(1)]
    fn reject_cbak(&self) -> HookResult {
        Err(Rollback::new(b"cbak-reject", 9))
    }
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20])
}

#[test]
fn typed_entry_emits_and_accepts() {
    let env = env();
    let exit = env.invoke::<Chain>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert_eq!(exit.code, 0);
    assert_eq!(exit.msg, b"emitted");
    assert_eq!(env.emitted().len(), 1);
}

#[test]
fn typed_cbak_accept_reaches_hook_exit_with_msg_on_success_outcome() {
    let env = env();
    let _ = env.invoke::<Chain>(0);
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<Chain>(0, CbakOutcome::Success(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
    assert_eq!(cbak_exit.code, 7);
    assert_eq!(cbak_exit.msg, b"cbak-accept");
}

#[test]
fn typed_cbak_accept_reaches_hook_exit_with_msg_on_failure_outcome() {
    let env = env();
    let _ = env.invoke::<Chain>(0);
    let txn = env.emitted()[0].clone();

    // The cbak fn cannot see this outcome — it accepts unconditionally
    // either way. Driving `Failure` here (vs. `Success` above) proves
    // that's not incidental.
    let cbak_exit = env.invoke_cbak::<Chain>(0, CbakOutcome::Failure(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
    assert_eq!(cbak_exit.code, 7);
    assert_eq!(cbak_exit.msg, b"cbak-accept");
}

#[test]
fn typed_cbak_rollback_reaches_hook_exit_with_msg_on_success_outcome() {
    let env = env();
    let _ = env.invoke::<Chain>(1);
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<Chain>(1, CbakOutcome::Success(txn));
    assert_eq!(cbak_exit.exit, ExitType::Rollback, "{cbak_exit:?}");
    assert_eq!(cbak_exit.code, 9);
    assert_eq!(cbak_exit.msg, b"cbak-reject");
}

#[test]
fn typed_cbak_rollback_reaches_hook_exit_with_msg_on_failure_outcome() {
    let env = env();
    let _ = env.invoke::<Chain>(1);
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<Chain>(1, CbakOutcome::Failure(txn));
    assert_eq!(cbak_exit.exit, ExitType::Rollback, "{cbak_exit:?}");
    assert_eq!(cbak_exit.code, 9);
    assert_eq!(cbak_exit.msg, b"cbak-reject");
}
