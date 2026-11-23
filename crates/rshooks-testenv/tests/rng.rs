//! `dice`/`util_random` through `TestEnv::invoke`: seeding, the per-execution
//! draw budget, and that a rollback keeps the draws it consumed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::api::etxn;
use rshooks::exit::HookResult;
use rshooks::prelude::*;
use rshooks::*;
use rshooks_testenv::prelude::*;

const SEED: [u8; 32] = [7u8; 32];

#[hooks]
pub struct Rng;

#[hooks]
impl Rng {
    /// Rolls `dice(6)`, accepting with the roll or the error code.
    #[hook(0, on = [Invoke])]
    fn roll(&self) -> HookResult {
        match dice(6) {
            Ok(v) => accept!(b"", v as i64),
            Err(e) => accept!(b"", e.code()),
        }
    }

    /// Fills 8 random bytes, accepting with the first byte or the error code.
    #[hook(1, on = [Invoke])]
    fn fill(&self) -> HookResult {
        let mut buf = [0u8; 8];
        match util_random(&mut buf) {
            Ok(n) => accept!(&buf[..n], 0),
            Err(e) => accept!(b"", e.code()),
        }
    }

    /// Draws, then rolls back.
    #[hook(2, on = [Invoke])]
    fn draw_then_rollback(&self) -> HookResult {
        let _ = dice(6);
        rollback!(b"no", 1)
    }

    /// Emits a payment so a callback can follow.
    #[hook(4, on = [Invoke], can_emit = [Payment])]
    fn emit_for_cbak(&self) -> HookResult {
        let _ = etxn::etxn_reserve(1);
        let mut template = vec![0x12, 0x00, 0x00];
        template.push(0x61);
        template.extend_from_slice(&0x4000_0000_0000_0001u64.to_be_bytes());
        template.push(0x83);
        template.push(20);
        template.extend_from_slice(&[0u8; 20]);
        let mut prepared = [0u8; 256];
        let n = etxn::prepare(&mut prepared, &template).expect("prepare");
        let mut hash = [0u8; 32];
        etxn::emit(&mut hash, &prepared[..n]).expect("emit");
        accept!(b"", 0)
    }

    /// Accepts with 32 random bytes followed by the callback's otxn id.
    #[cbak(4)]
    fn draw_in_cbak(&self) -> HookResult {
        let mut out = [0u8; 64];
        let _ = util_random(&mut out[..32]);
        let _ = otxn_id(&mut out[32..], 0);
        accept!(&out, 0)
    }

    /// Draws until the budget is exhausted, accepting with the number of
    /// successful draws.
    #[hook(3, on = [Invoke])]
    fn drain(&self) -> HookResult {
        let mut ok = 0i64;
        let mut last = 0i64;
        for _ in 0..300 {
            guard!(300);
            match dice(2) {
                Ok(_) => ok += 1,
                Err(e) => {
                    last = e.code();
                    break;
                }
            }
        }
        accept!(b"", ok * 1000 + (-last))
    }
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20])
}

#[test]
fn unseeded_env_has_too_little_entropy() {
    let env = env();
    assert_eq!(env.rng_state(), None);
    let code = rshooks_core::TOO_LITTLE_ENTROPY;
    assert_eq!(env.invoke::<Rng>(0).code, code);
    assert_eq!(env.invoke::<Rng>(1).code, code);
}

#[test]
fn seeded_env_draws_and_advances_state() {
    let env = env().rng_seed(SEED);
    assert_eq!(env.rng_state(), Some(SEED));
    let first = env.invoke::<Rng>(0);
    assert!((0..6).contains(&first.code), "{first:?}");
    assert_ne!(env.rng_state(), Some(SEED));
    let fill = env.invoke::<Rng>(1);
    assert_eq!(fill.code, 0);
    assert_eq!(fill.msg.len(), 8);
}

#[test]
fn identical_envs_draw_identically() {
    let a = env().rng_seed(SEED).invoke::<Rng>(1);
    let b = env().rng_seed(SEED).invoke::<Rng>(1);
    assert_eq!(a.msg, b.msg);
}

#[test]
fn each_invocation_gets_a_fresh_budget() {
    let env = env().rng_seed(SEED);
    // 256 draws succeed, the 257th is TooManyNonces; the next invoke starts over.
    let expected = 256 * 1000 + -rshooks_core::TOO_MANY_NONCES;
    assert_eq!(env.invoke::<Rng>(3).code, expected);
    assert_eq!(env.invoke::<Rng>(3).code, expected);
}

#[test]
fn rollback_does_not_rewind_the_rng() {
    let env = env().rng_seed(SEED);
    let exit = env.invoke::<Rng>(2);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_ne!(env.rng_state(), Some(SEED));
}

#[test]
fn stale_seed_sequence_and_thin_contribution_are_too_little_entropy() {
    let code = rshooks_core::TOO_LITTLE_ENTROPY;
    let stale = env().ledger_seq(5).rng_seed(SEED).rng_seeded_for(4);
    assert_eq!(stale.invoke::<Rng>(0).code, code);
    let current = env().ledger_seq(5).rng_seed(SEED).rng_seeded_for(5);
    assert!(current.invoke::<Rng>(0).code >= 0);
    let thin = env().rng_seed(SEED).rng_contributions(3, 5);
    assert_eq!(thin.invoke::<Rng>(0).code, code);
    let enough = env().rng_seed(SEED).rng_contributions(4, 5);
    assert!(enough.invoke::<Rng>(0).code >= 0);
}

/// Output block as the host defines it: the preimage layout is spelled out
/// here independently of the mock.
fn expected_block(txid: &[u8], callback_param: u32) -> [u8; 32] {
    use sha2::{Digest, Sha512};
    let mut pre = Vec::new();
    pre.extend_from_slice(b"RNGO");
    pre.extend_from_slice(&SEED);
    pre.extend_from_slice(txid);
    pre.extend_from_slice(&[1u8; 20]);
    pre.extend_from_slice(&[0u8; 32]);
    pre.extend_from_slice(&[0, 0, 1]);
    pre.extend_from_slice(&callback_param.to_be_bytes());
    Sha512::digest(&pre)[..32].try_into().unwrap()
}

fn cbak_draw(outcome: fn(EmittedTxn) -> CbakOutcome) -> Vec<u8> {
    let env = env().rng_seed(SEED);
    assert_eq!(env.invoke::<Rng>(4).exit, ExitType::Accept);
    let txn = env.emitted()[0].clone();
    let exit = env.invoke_cbak::<Rng>(4, outcome(txn));
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    exit.msg
}

#[test]
fn callback_draws_bind_the_callback_flag_and_wasm_param() {
    let success = cbak_draw(CbakOutcome::Success);
    let failure = cbak_draw(CbakOutcome::Failure);
    assert_eq!(success[..32], expected_block(&success[32..], 0));
    assert_eq!(failure[..32], expected_block(&failure[32..], 1));
    assert_ne!(success[..32], failure[..32]);
}

#[test]
fn callback_draw_differs_from_a_hook_draw_over_the_same_state() {
    let hook = env().rng_seed(SEED).invoke::<Rng>(1).msg;
    let success = cbak_draw(CbakOutcome::Success);
    assert_ne!(hook[..], success[..8]);
}
