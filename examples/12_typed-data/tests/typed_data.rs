//! Off-chain unit tests for the `typed-data` example, driven through
//! `TestEnv::invoke` against the real `TypedData` chain — no wasm build, no
//! node.
//!
//! `Config`/`Instruction`/`PauseSwitch`/`DepositValue` are private to
//! `src/lib.rs`, so every Hook API parameter and state value here is
//! encoded/decoded by hand, byte-for-byte matching the wire layout the
//! crate's own README documents ("every field, in declaration order,
//! little-endian, back-to-back").

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks_testenv::prelude::*;
use typed_data::{TypedData, TypedDataError};

const HOOK_ACCOUNT: [u8; 20] = [1u8; 20];
const OWNER: [u8; 20] = [2u8; 20];

const DEPOSIT_TAG: u8 = 1;
const ACTION_DEPOSIT: u8 = 1;
const ACTION_WITHDRAW: u8 = 2;

const DEFAULT_MIN_AMOUNT: u64 = 1_000_000;
const DEFAULT_LOCK_LEDGERS: u32 = 10;

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account(HOOK_ACCOUNT)
        .otxn(Otxn::new(TxType::Invoke).account(OWNER))
}

/// `Instruction { action: u8, amount: u64 }`, 9 bytes.
fn instruction_bytes(action: u8, amount: u64) -> [u8; 9] {
    let mut out = [0u8; 9];
    out[0] = action;
    out[1..9].copy_from_slice(&amount.to_le_bytes());
    out
}

/// `Config { min_amount: u64, lock_ledgers: u32 }`, 12 bytes.
fn config_bytes(min_amount: u64, lock_ledgers: u32) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[0..8].copy_from_slice(&min_amount.to_le_bytes());
    out[8..12].copy_from_slice(&lock_ledgers.to_le_bytes());
    out
}

/// `AdminName { section: u8, field: u8 }`'s fixed instance (`section = 0,
/// field = 0`), 2 bytes.
const ADMIN_PAUSE_NAME: [u8; 2] = [0, 0];

/// `PauseSwitch { paused: u8 }`, 1 byte.
fn pause_bytes(paused: u8) -> [u8; 1] {
    [paused]
}

/// `DepositKey { tag: u8, owner: AccountId }`, 21 bytes — never
/// zero-padded at this layer, `rshooks-testenv` pads to the host's fixed
/// 32-byte storage width internally, matching production.
fn deposit_key(owner: [u8; 20]) -> [u8; 21] {
    let mut out = [0u8; 21];
    out[0] = DEPOSIT_TAG;
    out[1..21].copy_from_slice(&owner);
    out
}

/// `DepositValue { amount: u64, deadline: u32, flags: u8 }`, 13 bytes.
fn deposit_value_bytes(amount: u64, deadline: u32, flags: u8) -> [u8; 13] {
    let mut out = [0u8; 13];
    out[0..8].copy_from_slice(&amount.to_le_bytes());
    out[8..12].copy_from_slice(&deadline.to_le_bytes());
    out[12] = flags;
    out
}

#[derive(Debug, PartialEq)]
struct DepositValue {
    amount: u64,
    deadline: u32,
    flags: u8,
}

fn read_deposit(env: &TestEnv, owner: [u8; 20]) -> Option<DepositValue> {
    let bytes = env.state(&deposit_key(owner))?;
    assert_eq!(bytes.len(), 13);
    Some(DepositValue {
        amount: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
        deadline: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        flags: bytes[12],
    })
}

#[test]
fn deposit_with_no_cfg_uses_defaults() {
    let env = env().otxn(Otxn::new(TxType::Invoke).account(OWNER).param(
        b"INS",
        &instruction_bytes(ACTION_DEPOSIT, DEFAULT_MIN_AMOUNT),
    ));
    let exit = env.invoke::<TypedData>(0);
    assert_eq!(exit.exit, ExitType::Accept);

    let deposit = read_deposit(&env, OWNER).unwrap();
    assert_eq!(deposit.amount, DEFAULT_MIN_AMOUNT);
    assert_eq!(deposit.deadline, 1 + DEFAULT_LOCK_LEDGERS);
    assert_eq!(deposit.flags, 1);
}

#[test]
fn malformed_cfg_rolls_back_instead_of_using_defaults() {
    // A `CFG` value shorter than `Config`'s 12-byte encoding is present
    // but undecodable — this must never be treated the same as `CFG`
    // being absent.
    let env =
        env()
            .hook_param(b"CFG", &[0u8; 4])
            .otxn(Otxn::new(TxType::Invoke).account(OWNER).param(
                b"INS",
                &instruction_bytes(ACTION_DEPOSIT, DEFAULT_MIN_AMOUNT),
            ));
    let exit = env.invoke::<TypedData>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, TypedDataError::ConfigMalformed.code());
    assert_eq!(read_deposit(&env, OWNER), None);
}

#[test]
fn deposit_paused_rolls_back() {
    let env = env().hook_param(&ADMIN_PAUSE_NAME, &pause_bytes(1)).otxn(
        Otxn::new(TxType::Invoke).account(OWNER).param(
            b"INS",
            &instruction_bytes(ACTION_DEPOSIT, DEFAULT_MIN_AMOUNT),
        ),
    );
    let exit = env.invoke::<TypedData>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, TypedDataError::DepositsPaused.code());
}

#[test]
fn deposit_not_paused_when_switch_absent() {
    let env = env().otxn(Otxn::new(TxType::Invoke).account(OWNER).param(
        b"INS",
        &instruction_bytes(ACTION_DEPOSIT, DEFAULT_MIN_AMOUNT),
    ));
    let exit = env.invoke::<TypedData>(0);
    assert_eq!(exit.exit, ExitType::Accept);
}

#[test]
fn malformed_pause_switch_rolls_back_instead_of_allowing_deposit() {
    // A present-but-wrong-size pause switch is a decode failure, not "not
    // paused" — this is the same bug class as `malformed_cfg_rolls_back_
    // instead_of_using_defaults` above, for the `AdminName`-addressed
    // parameter instead of `CFG`.
    let env = env().hook_param(&ADMIN_PAUSE_NAME, &[]).otxn(
        Otxn::new(TxType::Invoke).account(OWNER).param(
            b"INS",
            &instruction_bytes(ACTION_DEPOSIT, DEFAULT_MIN_AMOUNT),
        ),
    );
    let exit = env.invoke::<TypedData>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, TypedDataError::PauseReadFailed.code());
    assert_eq!(read_deposit(&env, OWNER), None);
}

#[test]
fn deposit_amount_overflow_rolls_back() {
    let env = env()
        .state_entry(
            &deposit_key(OWNER),
            &deposit_value_bytes(u64::MAX - 10, 0, 1),
        )
        .otxn(Otxn::new(TxType::Invoke).account(OWNER).param(
            b"INS",
            &instruction_bytes(ACTION_DEPOSIT, DEFAULT_MIN_AMOUNT),
        ));
    let exit = env.invoke::<TypedData>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, TypedDataError::AmountOverflow.code());

    // The pre-existing balance must survive the rejected deposit unchanged.
    let deposit = read_deposit(&env, OWNER).unwrap();
    assert_eq!(deposit.amount, u64::MAX - 10);
}

#[test]
fn deposit_deadline_overflow_rolls_back() {
    let env = env()
        .hook_param(b"CFG", &config_bytes(0, u32::MAX))
        .ledger_seq(10)
        .otxn(
            Otxn::new(TxType::Invoke)
                .account(OWNER)
                .param(b"INS", &instruction_bytes(ACTION_DEPOSIT, 1_000)),
        );
    let exit = env.invoke::<TypedData>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, TypedDataError::DeadlineOverflow.code());
    assert_eq!(read_deposit(&env, OWNER), None);
}

#[test]
fn withdraw_after_lock_window_deletes_entry() {
    let env = env()
        .state_entry(&deposit_key(OWNER), &deposit_value_bytes(5_000_000, 5, 1))
        .ledger_seq(5)
        .otxn(
            Otxn::new(TxType::Invoke)
                .account(OWNER)
                .param(b"INS", &instruction_bytes(ACTION_WITHDRAW, 0)),
        );
    let exit = env.invoke::<TypedData>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(read_deposit(&env, OWNER), None);
}
