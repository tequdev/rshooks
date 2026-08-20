//! End-to-end: a real `#[hooks]` chain (a counter entry exercising
//! accept/rollback, an emitter entry using `txn_template!`) driven through
//! `TestEnv::invoke` — state assertions, exit codes, emission capture.

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
    /// A simple payout template: destination + amount, everything else
    /// filled in by `prepare_for_emit`.
    pub struct PayoutTemplate {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        amount: native_amount(sfAmount) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        destination: account_id(sfDestination),
        emit_details: emit_details,
    }
}

#[hooks]
pub struct Payments {
    /// Persistent invocation counter.
    #[state(key = b"counter")]
    counter: State<u64>,
}

#[hooks]
impl Payments {
    /// Increments the counter and accepts with the new count — rolls back
    /// if the store itself fails.
    #[hook(0, on = [Payment])]
    fn on_payment(&self) -> HookResult {
        let count = self.counter.get().unwrap_or(None).unwrap_or(0);
        let next = count.wrapping_add(1);
        if self.counter.set(&next).is_err() {
            rollback!(b"counter: store failed", 1);
        }
        accept!(b"counted", next as i64)
    }

    /// Writes the counter, then unconditionally rolls back — proves the
    /// state write is undone (design §5's "rollback reverts state" rule).
    #[hook(1, on = [Invoke])]
    fn write_then_force_rollback(&self) -> HookResult {
        let _ = self.counter.set(&999u64);
        rollback!(b"forced rollback", 42)
    }

    /// Reserves one emission slot and emits a single Payment.
    #[hook(2, on = [Invoke], can_emit = [Payment])]
    fn payout(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(b"reserve failed", 1);
        }
        let mut tpl = PayoutTemplate::new();
        tpl.set_destination(&AccountId([0xABu8; 20]));
        if tpl.set_amount(1_000_000).is_err() {
            rollback!(b"amount out of range", 2);
        }
        let prepared = match tpl.prepare_for_emit() {
            Ok(p) => p,
            Err(_) => rollback!(b"prepare_for_emit failed", 3),
        };
        if prepared.emit().is_err() {
            rollback!(b"emit failed", 4);
        }
        accept!(b"paid", 0)
    }
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .amount_drops(1_000_000),
    )
}

#[test]
fn first_payment_counts_to_one() {
    let env = env();
    let exit = env.invoke::<Payments>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, 1);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(1));
}

#[test]
fn counter_persists_across_invocations() {
    let env = env();
    env.invoke::<Payments>(0);
    env.invoke::<Payments>(0);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(2));
}

#[test]
fn rollback_reverts_state_writes() {
    let env = env().state_entry(b"counter", &7u64.to_le_bytes());
    let exit = env.invoke::<Payments>(1);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, 42);
    // The write to 999 inside the entry must be undone.
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(7));
}

#[test]
fn payout_emits_one_payment() {
    let env = env();
    let exit = env.invoke::<Payments>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Payment));
    assert!(!emitted[0].blob().is_empty());
}
