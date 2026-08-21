//! `TestEnv::strict_can_emit` (design §5.6) and `NativeEntry::can_emit`'s
//! three-state contract (design review FIX 5): an entry with no `can_emit`
//! declaration at all (`None`) is unrestricted even under strict mode — the
//! on-chain semantics of an absent `HookCanEmit` — while a declared list
//! (`Some(&[..])`, empty slice included) is enforced exactly.

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
    /// A minimal, otherwise-valid emitted-Payment template.
    pub struct Probe {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        amount: native_amount(sfAmount) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

/// Reserves and emits one `Payment`, shared by every entry below — only
/// their `can_emit` declaration differs.
fn emit_a_payment() -> HookResult {
    if etxn_reserve(1).is_err() {
        rollback!(b"reserve", 1);
    }
    let mut tpl = Probe::new();
    let prepared = match tpl.prepare_for_emit() {
        Ok(p) => p,
        Err(_) => rollback!(b"prepare", 2),
    };
    let bytes = prepared.as_bytes().to_vec();
    let mut out = [0u8; 32];
    if emit(&mut out, &bytes).is_err() {
        rollback!(b"emit", 3);
    }
    accept!(b"", 0)
}

#[hooks]
pub struct CanEmitChecks;

#[hooks]
impl CanEmitChecks {
    /// No `can_emit` declared at all — unrestricted, even in strict mode.
    #[hook(0, on = [Invoke])]
    fn no_declaration(&self) -> HookResult {
        emit_a_payment()
    }

    /// Declares `Payment`, and emits exactly that — allowed.
    #[hook(1, on = [Invoke], can_emit = [Payment])]
    fn declared_and_matching(&self) -> HookResult {
        emit_a_payment()
    }

    /// Declares `Invoke` but emits a `Payment` — a strict-mode violation.
    #[hook(2, on = [Invoke], can_emit = [Invoke])]
    fn declared_but_mismatched(&self) -> HookResult {
        emit_a_payment()
    }
}

#[test]
fn strict_mode_allows_emission_with_no_can_emit_declaration() {
    let env = TestEnv::new().strict_can_emit(true);
    let exit = env.invoke::<CanEmitChecks>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(env.emitted().len(), 1);
}

#[test]
fn strict_mode_allows_emission_within_the_declared_list() {
    let env = TestEnv::new().strict_can_emit(true);
    let exit = env.invoke::<CanEmitChecks>(1);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(env.emitted().len(), 1);
}

#[test]
#[should_panic(expected = "not declared in its `can_emit` list")]
fn strict_mode_panics_on_emission_outside_the_declared_list() {
    let env = TestEnv::new().strict_can_emit(true);
    let _ = env.invoke::<CanEmitChecks>(2);
}

#[test]
fn non_strict_mode_never_checks_can_emit_regardless_of_declaration() {
    let env = TestEnv::new(); // strict_can_emit left at its default (off)
    let exit = env.invoke::<CanEmitChecks>(2);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(env.emitted().len(), 1);
}
