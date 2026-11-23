//! Off-chain unit tests for `Rng`'s entry — driven through
//! `TestEnv::invoke` against the real chain, no wasm build, no node.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rng::Rng;
use rshooks::prelude::*;
use rshooks_testenv::prelude::*;

const SEED: [u8; 32] = [9u8; 32];

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
}

fn stored(env: &TestEnv, key: &[u8]) -> u64 {
    env.state_typed::<u64>(key).unwrap()
}

#[test]
fn seeded_roll_is_in_range_and_stored() {
    let env = env().rng_seed(SEED);
    let exit = env.invoke::<Rng>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert!((0..6).contains(&exit.code), "{exit:?}");
    assert_eq!(stored(&env, b"roll"), exit.code as u64);
    assert!(env.state(b"bits").is_some());
}

#[test]
fn consecutive_invocations_advance_the_entropy() {
    let env = env().rng_seed(SEED);
    env.invoke::<Rng>(0);
    let first = stored(&env, b"bits");
    env.invoke::<Rng>(0);
    assert_ne!(stored(&env, b"bits"), first);
}

#[test]
fn unseeded_ledger_rolls_back_with_no_entropy() {
    let env = env();
    let exit = env.invoke::<Rng>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, 1);
    assert!(env.state(b"roll").is_none());
}

#[test]
fn thin_validator_contribution_rolls_back_with_no_entropy() {
    let env = env().rng_seed(SEED).rng_contributions(1, 5);
    let exit = env.invoke::<Rng>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, 1);
}
