//! Builder order-independence for `state_entry` vs. `hook_account`/
//! `own_namespace` (design review FIX 3): a `state_entry` seed must stay
//! reachable at whatever account/namespace `hook_account`/`own_namespace`
//! ultimately name, regardless of which builder ran first — see
//! `World::rekey_own_seeds`'s doc comment.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::*;
use rshooks_testenv::prelude::*;

const FOREIGN_ACC: [u8; 20] = [5u8; 20];
const FOREIGN_NS: [u8; 32] = [6u8; 32];

#[hooks]
pub struct Seeded {
    #[state(key = b"K")]
    seeded: State<u64>,
    #[state(key = b"seen")]
    seen: State<u64>,
}

#[hooks]
impl Seeded {
    /// Reads this hook's own seeded entry and copies it into `seen`, so a
    /// test can confirm the seed is visible from *inside* an invocation
    /// (not just via `TestEnv::state`).
    #[hook(0, on = [Invoke])]
    fn read_own_seed(&self) -> i64 {
        match self.seeded.get() {
            Ok(Some(v)) => {
                if self.seen.set(&v).is_err() {
                    rollback!(b"store failed", 1);
                }
                accept!(b"", 0)
            }
            Ok(None) => accept!(b"", 1),
            Err(e) => accept!(b"", e.code()),
        }
    }

    /// Reads a foreign seed at a fixed (account, namespace) unrelated to
    /// this hook's own — proves `hook_account` changes never touch it.
    #[hook(1, on = [Invoke])]
    fn read_foreign_seed(&self) -> i64 {
        let mut out = [0u8; 8];
        let code = match rshooks::api::state::state_foreign(
            &mut out,
            b"F",
            &FOREIGN_NS,
            &FOREIGN_ACC,
        ) {
            Ok(_) => {
                let v = u64::from_le_bytes(out);
                if self.seen.set(&v).is_err() {
                    rollback!(b"store failed", 1);
                }
                0
            }
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }
}

#[test]
fn seed_then_set_account_is_found_by_env_and_by_hook() {
    let env = TestEnv::new()
        .state_entry(b"K", &42u64.to_le_bytes())
        .hook_account([7u8; 20]);
    assert_eq!(env.state(b"K"), Some(42u64.to_le_bytes().to_vec()));

    let exit = env.invoke::<Seeded>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(env.state_typed::<u64>(b"seen"), Some(42));
}

#[test]
fn seed_then_set_own_namespace_is_found_by_env_and_by_hook() {
    let env = TestEnv::new()
        .state_entry(b"K", &55u64.to_le_bytes())
        .own_namespace([3u8; 32]);
    assert_eq!(env.state(b"K"), Some(55u64.to_le_bytes().to_vec()));

    let exit = env.invoke::<Seeded>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(env.state_typed::<u64>(b"seen"), Some(55));
}

#[test]
fn setting_account_twice_rekeys_through_both_changes() {
    let env = TestEnv::new()
        .state_entry(b"K", &7u64.to_le_bytes())
        .hook_account([1u8; 20])
        .hook_account([2u8; 20]);
    assert_eq!(env.state(b"K"), Some(7u64.to_le_bytes().to_vec()));

    let exit = env.invoke::<Seeded>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(env.state_typed::<u64>(b"seen"), Some(7));
}

#[test]
fn foreign_seed_is_unaffected_by_hook_account_changes() {
    let env = TestEnv::new()
        .foreign_state_entry(FOREIGN_NS, FOREIGN_ACC, b"F", &99u64.to_le_bytes())
        .hook_account([7u8; 20])
        .hook_account([8u8; 20]);

    let exit = env.invoke::<Seeded>(1);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(env.state_typed::<u64>(b"seen"), Some(99));
}
