//! `_g`'s call count for a given guard id is cumulative for the whole
//! invocation (`applyHook.cpp:3297-3331`): once it exceeds `maxiter`, the
//! invocation rolls back with `GUARD_VIOLATION`. `txn_template!`'s
//! `vl(sfX, MIN, MAX)` setters share one guard id across every call in an
//! invocation, so calling one twice in a single invocation trips the same
//! budget.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::prelude::*;
use rshooks::*;
use rshooks_testenv::prelude::*;

txn_template! {
    /// A minimal, otherwise-valid Payment template with one
    /// `vl(sfX, MIN, MAX)` field, for the guard-budget test below.
    pub struct VlProbe {
        transaction_type = ttPAYMENT,
        sequence: sfSequence = 0,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        amount: sfAmount = NativeAmount(0),
        fee: sfFee = NativeAmount(0),
        signing_pub_key: sfSigningPubKey = [],
        memo_format: vl(sfMemoFormat, 1, 4),
        account: sfAccount,
        destination: sfDestination,
        emit_details: emit_details,
    }
}

#[hooks]
pub struct GuardBudget {}

#[hooks]
impl GuardBudget {
    #[hook(0, on = [Invoke])]
    fn guard_within_budget(&self) -> HookResult {
        let mut i = 0u32;
        while i < 5 {
            guard!(5);
            i += 1;
        }
        accept!(b"", 0)
    }

    #[hook(1, on = [Invoke])]
    fn guard_exceeds_budget(&self) -> HookResult {
        let mut i = 0u32;
        while i < 10 {
            guard!(3);
            i += 1;
        }
        accept!(b"", 0)
    }

    #[hook(2, on = [Invoke])]
    fn vl_setter_called_once(&self) -> HookResult {
        let mut tpl = VlProbe::new();
        if tpl.set_memo_format(&[0xAAu8; 4]).is_err() {
            rollback!(b"set failed", 1);
        }
        accept!(b"", 0)
    }

    #[hook(3, on = [Invoke])]
    fn vl_setter_called_twice(&self) -> HookResult {
        let mut tpl = VlProbe::new();
        if tpl.set_memo_format(&[0xAAu8; 4]).is_err() {
            rollback!(b"set failed", 1);
        }
        if tpl.set_memo_format(&[0xBBu8; 2]).is_err() {
            rollback!(b"set failed", 2);
        }
        accept!(b"", 0)
    }
}

#[test]
fn guard_budget_is_cumulative_across_the_whole_invocation() {
    let env = TestEnv::new();
    assert_eq!(env.invoke::<GuardBudget>(0).exit, ExitType::Accept);

    let env = TestEnv::new();
    let exit = env.invoke::<GuardBudget>(1);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, rshooks_core::GUARD_VIOLATION);
}

#[test]
fn vl_setter_shares_its_guard_budget_across_calls() {
    let env = TestEnv::new();
    assert_eq!(env.invoke::<GuardBudget>(2).exit, ExitType::Accept);

    let env = TestEnv::new();
    let exit = env.invoke::<GuardBudget>(3);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, rshooks_core::GUARD_VIOLATION);
}
