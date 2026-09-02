//! End-to-end: a `#[hooks]` entry declaring signature parameters
//! (`docs/PARAM_SIGNATURE_DESIGN.md` §1) — `increment(account: AccountId,
//! count: u16)` — driven through `TestEnv::invoke` with the originating
//! transaction seeded via `Otxn::param`, using [`rshooks::sig_name!`] to
//! build each declared `HookParameterName`.
//!
//! Covers the generated prologue's outcomes: a correctly-seeded invocation
//! reaches the body with both arguments already decoded; a missing or
//! wrong-length parameter rolls back with the argument's own index as the
//! code.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use rshooks::exit::HookResult;
use rshooks::sig_name;
use rshooks::types::AccountId;
use rshooks::xfl::XFL;
use rshooks::{accept, hooks};
use rshooks_testenv::prelude::*;

#[hooks]
pub struct Increment;

#[hooks]
impl Increment {
    /// `account`(0): `AccountId`. `count`(1): `u16`. The body only runs once
    /// both are decoded — `accept!`s with `count` as the code, so a
    /// successful invocation proves the decoded value actually reached the
    /// body (not just that the prologue compiled).
    #[hook(0, on = [Invoke])]
    fn main(&self, account: AccountId, count: u16) -> HookResult {
        let _ = account;
        accept!(b"incremented", i64::from(count))
    }

    /// `rate`(0): `XFL` (XAS-010d `0x80`) — a second entry on the same
    /// chain struct (only one `#[hooks]` struct per crate is allowed, per
    /// its `#[no_mangle]` link marker), rather than a second struct.
    #[hook(1, on = [Invoke])]
    fn set_rate(&self, rate: XFL) -> HookResult {
        accept!(b"rate", rate.raw_bits())
    }
}

const ACCOUNT_NAME: [u8; 14] = sig_name!(0, AccountId, b"account");
const COUNT_NAME: [u8; 12] = sig_name!(1, u16, b"count");
const RATE_NAME: [u8; 11] = sig_name!(0, XFL, b"rate");

const SENDER: [u8; 20] = [9u8; 20];
const TARGET_ACCOUNT: [u8; 20] = [7u8; 20];

fn env_with_params(params: &[(&[u8], &[u8])]) -> TestEnv {
    let mut otxn = Otxn::new(TxType::Invoke).account(SENDER);
    for (name, value) in params {
        otxn = otxn.param(name, value);
    }
    TestEnv::new().hook_account(SENDER).otxn(otxn)
}

#[test]
fn correctly_seeded_invocation_decodes_both_args_and_reaches_the_body() {
    let env = env_with_params(&[
        (&ACCOUNT_NAME, &TARGET_ACCOUNT),
        (&COUNT_NAME, &7u16.to_be_bytes()),
    ]);
    let exit = env.invoke::<Increment>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    // The decoded `count` value reached the body: it is the accept code.
    assert_eq!(exit.code, 7);
    assert_eq!(exit.msg, b"incremented");
}

#[test]
fn missing_first_arg_rolls_back_with_its_own_index_as_the_code() {
    // `account` (index 0) is never set; `count` is present and valid.
    let env = env_with_params(&[(&COUNT_NAME, &7u16.to_be_bytes())]);
    let exit = env.invoke::<Increment>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, 0);
    assert_eq!(exit.msg, b"rshooks: bad sig param 'account'");
}

#[test]
fn missing_second_arg_rolls_back_with_its_own_index_as_the_code() {
    // `count` (index 1) is never set; `account` is present and valid.
    let env = env_with_params(&[(&ACCOUNT_NAME, &TARGET_ACCOUNT)]);
    let exit = env.invoke::<Increment>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, 1);
    assert_eq!(exit.msg, b"rshooks: bad sig param 'count'");
}

#[test]
fn wrong_length_value_rolls_back_the_same_way_as_absence() {
    // `count` is present but only 1 byte (it decodes as `u16`, 2 bytes BE).
    let env = env_with_params(&[(&ACCOUNT_NAME, &TARGET_ACCOUNT), (&COUNT_NAME, &[0x07])]);
    let exit = env.invoke::<Increment>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, 1);
    assert_eq!(exit.msg, b"rshooks: bad sig param 'count'");
}

#[test]
fn wrong_length_account_value_rolls_back_with_index_zero() {
    // `account` is present but only 4 bytes (it decodes as `AccountId`, 20 bytes).
    let env = env_with_params(&[
        (&ACCOUNT_NAME, &[1, 2, 3, 4]),
        (&COUNT_NAME, &7u16.to_be_bytes()),
    ]);
    let exit = env.invoke::<Increment>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, 0);
    assert_eq!(exit.msg, b"rshooks: bad sig param 'account'");
}

#[test]
fn xfl_param_decodes_and_reaches_the_body_as_the_accept_code() {
    let env = env_with_params(&[(&RATE_NAME, &0x54838D7EA4C68000u64.to_be_bytes())]);
    let exit = env.invoke::<Increment>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert_eq!(exit.code, 0x54838D7EA4C68000u64 as i64);
    assert_eq!(exit.msg, b"rate");
}

#[test]
fn xfl_param_wrong_length_rolls_back_with_its_own_index_as_the_code() {
    // `rate` is present but only 7 bytes (it decodes as `XFL`, 8 bytes BE).
    let env = env_with_params(&[(&RATE_NAME, &0x54838D7EA4C68000u64.to_be_bytes()[..7])]);
    let exit = env.invoke::<Increment>(1);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, 0);
    assert_eq!(exit.msg, b"rshooks: bad sig param 'rate'");
}
