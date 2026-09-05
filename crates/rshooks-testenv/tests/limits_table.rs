//! Boundary tests for the design §4 `InvocationContext` limits table and
//! the §5 fidelity rules, driven through a real `#[hooks]` chain and
//! `TestEnv::invoke`.

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
    /// A minimal, otherwise-valid emitted-Payment template for the
    /// reserve/emit-count boundary tests.
    pub struct Probe {
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
pub struct Limits {
    #[state(key = b"scratch")]
    scratch: State<u64>,
}

#[hooks]
impl Limits {
    #[hook(0, on = [Invoke])]
    fn details_without_reserve(&self) -> HookResult {
        let mut buf = [0u8; EMIT_DETAILS_MAX_LEN];
        let code = match etxn_details(&mut buf) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(1, on = [Invoke])]
    fn fee_base_without_reserve(&self) -> HookResult {
        let code = match etxn_fee_base(&[]) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(2, on = [Invoke])]
    fn burden_without_reserve(&self) -> HookResult {
        let code = match etxn_burden() {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(3, on = [Invoke], can_emit = [Payment])]
    fn emit_without_reserve(&self) -> HookResult {
        let mut out = [0u8; 32];
        let code = match emit(&mut out, &[0u8; 4]) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(4, on = [Invoke], can_emit = [Payment])]
    fn reserve_then_emit_twice(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(b"reserve", 1);
        }
        let mut tpl = Probe::new();
        tpl.set_destination(&AccountId::default());
        let prepared = match tpl.prepare_for_emit() {
            Ok(p) => p,
            Err(_) => rollback!(b"prepare", 2),
        };
        let bytes = prepared.as_bytes().to_vec();
        let mut first_out = [0u8; 32];
        let first = emit(&mut first_out, &bytes);
        let mut second_out = [0u8; 32];
        let second_code = match emit(&mut second_out, &bytes) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        if first.is_err() {
            rollback!(b"first emit failed", 3);
        }
        accept!(b"", second_code)
    }

    #[hook(5, on = [Invoke])]
    fn burden_overflow(&self) -> HookResult {
        if etxn_reserve(2).is_err() {
            rollback!(b"reserve", 1);
        }
        let code = match etxn_burden() {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(6, on = [Invoke])]
    fn hook_param_absent(&self) -> HookResult {
        let code = match hook_param_exact::<[u8; 4]>(b"missing") {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(7, on = [Invoke])]
    fn read_hook_hash(&self) -> HookResult {
        match hook_hash_buf(0) {
            Ok(h) => {
                let n = u64::from(h[0]);
                if self.state.scratch.set(&n).is_err() {
                    rollback!(b"store failed", 1);
                }
                accept!(b"", 0)
            }
            Err(e) => accept!(b"", e.code()),
        }
    }

    #[hook(8, on = [Invoke])]
    fn emit_a_trace(&self) -> HookResult {
        let _ = rshooks::api::trace::trace(b"hello", b"world", false);
        let _ = rshooks::api::trace::trace_num(b"num", 7);
        accept!(b"", 0)
    }

    #[hook(9, on = [Invoke])]
    fn too_small_buffer(&self) -> HookResult {
        let mut out = [0u8; 4];
        let code = match rshooks::api::state::state(&mut out, b"K") {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }
}

#[test]
fn etxn_prerequisites_without_reserve() {
    let env = TestEnv::new();
    assert_eq!(
        env.invoke::<Limits>(0).code,
        rshooks_core::PREREQUISITE_NOT_MET
    );
    let env = TestEnv::new();
    assert_eq!(
        env.invoke::<Limits>(1).code,
        rshooks_core::PREREQUISITE_NOT_MET
    );
    let env = TestEnv::new();
    assert_eq!(
        env.invoke::<Limits>(2).code,
        rshooks_core::PREREQUISITE_NOT_MET
    );
    let env = TestEnv::new();
    assert_eq!(
        env.invoke::<Limits>(3).code,
        rshooks_core::PREREQUISITE_NOT_MET
    );
}

#[test]
fn emit_beyond_reserve_is_too_many_emitted_txn() {
    let env = TestEnv::new();
    let exit = env.invoke::<Limits>(4);
    assert_eq!(
        exit.exit,
        ExitType::Accept,
        "first emit must have succeeded: {exit:?}"
    );
    assert_eq!(exit.code, rshooks_core::TOO_MANY_EMITTED_TXN);
    assert_eq!(env.emitted().len(), 1);
}

#[test]
fn burden_overflow_is_fee_too_large() {
    // `otxn_emitted` validates its burden fits in an i64, so the largest
    // legal seed is `i64::MAX` — `reserve(2)` still multiplies it past
    // `i64::MAX`, so `etxn_burden()` must report `FEE_TOO_LARGE`.
    let env = TestEnv::new().otxn_emitted(i64::MAX as u64, 0);
    let exit = env.invoke::<Limits>(5);
    assert_eq!(exit.code, rshooks_core::FEE_TOO_LARGE);
}

#[test]
#[should_panic(expected = "burden must fit in an i64")]
fn otxn_emitted_rejects_a_burden_that_does_not_fit_in_i64() {
    let _ = TestEnv::new().otxn_emitted(u64::MAX, 0);
}

#[test]
fn absent_hook_param_is_doesnt_exist() {
    let env = TestEnv::new();
    let exit = env.invoke::<Limits>(6);
    assert_eq!(exit.code, rshooks_core::DOESNT_EXIST);
}

#[test]
fn hook_hash_is_readable_per_position() {
    let env = TestEnv::new().hook_hash(0, [7u8; 32]);
    let exit = env.invoke::<Limits>(7);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(env.state_typed::<u64>(b"scratch"), Some(7));
}

#[test]
fn traces_are_captured_not_printed() {
    let env = TestEnv::new();
    let exit = env.invoke::<Limits>(8);
    assert_eq!(exit.exit, ExitType::Accept);
    let traces = env.traces();
    assert_eq!(traces.len(), 2);
    assert_eq!(traces[0].message, b"hello");
    assert_eq!(traces[0].data, b"world");
    assert_eq!(traces[1].message, b"num");
    assert_eq!(traces[1].data, 7i64.to_be_bytes());
}

#[test]
fn undersized_destination_is_too_small_never_truncates() {
    let env = TestEnv::new().state_entry(b"K", &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    let exit = env.invoke::<Limits>(9);
    assert_eq!(exit.code, rshooks_core::TOO_SMALL);
}
