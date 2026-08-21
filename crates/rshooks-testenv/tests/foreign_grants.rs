//! Foreign-state write authorization (design §5.5): `NOT_AUTHORIZED` vs a
//! granted success vs the retry-block flag — and that a `TOO_BIG` failure
//! (a size/limit error, not an authorization failure) never sets the
//! retry-block flag.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::exit::HookResult;
use rshooks::*;
use rshooks_testenv::prelude::*;

const TARGET: [u8; 20] = [9u8; 20];
const NS: [u8; 32] = [1u8; 32];

#[hooks]
pub struct ForeignWriter {
    #[state(key = b"scratch")]
    scratch: State<u64>,
}

#[hooks]
impl ForeignWriter {
    /// Writes once into the seeded target/ns, reports the raw error code.
    #[hook(0, on = [Invoke])]
    fn write_once(&self) -> HookResult {
        let code = match rshooks::api::state::state_foreign_set(b"v", b"K", &NS, &TARGET) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    /// Writes twice: an oversized (`TOO_BIG`) attempt, then a normal one —
    /// proves a size/limit failure never sets the retry-block flag.
    #[hook(1, on = [Invoke])]
    fn oversized_then_normal(&self) -> HookResult {
        let big = [0u8; 257];
        let first_code = match rshooks::api::state::state_foreign_set(&big, b"K", &NS, &TARGET) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        let second_code = match rshooks::api::state::state_foreign_set(b"v", b"K", &NS, &TARGET) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        if self.scratch.set(&(first_code as u64)).is_err() {
            rollback!(b"store failed", 1);
        }
        accept!(b"", second_code)
    }

    /// Writes twice, both unauthorized — proves the second attempt is
    /// blocked by `PREVIOUS_FAILURE_PREVENTS_RETRY`, not a second
    /// `NOT_AUTHORIZED`.
    #[hook(2, on = [Invoke])]
    fn unauthorized_twice(&self) -> HookResult {
        let first_code = match rshooks::api::state::state_foreign_set(b"v", b"K", &NS, &TARGET) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        let second_code = match rshooks::api::state::state_foreign_set(b"v", b"K", &NS, &TARGET) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        if self.scratch.set(&(first_code as u64)).is_err() {
            rollback!(b"store failed", 1);
        }
        accept!(b"", second_code)
    }
}

#[test]
fn ungranted_foreign_write_is_not_authorized() {
    let env = TestEnv::new().hook_account([2u8; 20]);
    let exit = env.invoke::<ForeignWriter>(0);
    assert_eq!(exit.code, rshooks_core::NOT_AUTHORIZED);
}

#[test]
fn granted_foreign_write_succeeds() {
    let env = TestEnv::new()
        .hook_account([2u8; 20])
        .grant(TARGET, NS, Grant::account([2u8; 20]));
    let exit = env.invoke::<ForeignWriter>(0);
    assert_eq!(exit.code, 0);
    assert_eq!(exit.exit, ExitType::Accept);
}

#[test]
fn grant_by_hook_hash_authorizes_regardless_of_account() {
    let env = TestEnv::new()
        .hook_account([2u8; 20])
        .hook_hash(0, [5u8; 32])
        .grant(TARGET, NS, Grant::hook_hash([5u8; 32]));
    let exit = env.invoke::<ForeignWriter>(0);
    assert_eq!(exit.code, 0);
}

#[test]
fn a_size_failure_never_blocks_the_next_attempt() {
    let env = TestEnv::new()
        .hook_account([2u8; 20])
        .grant(TARGET, NS, Grant::account([2u8; 20]));
    let exit = env.invoke::<ForeignWriter>(1);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(
        env.state_typed::<u64>(b"scratch"),
        Some(rshooks_core::TOO_BIG as u64)
    );
    assert_eq!(
        exit.code, 0,
        "the second, normal-sized write must still succeed"
    );
}

#[test]
fn an_authorization_failure_blocks_the_next_attempt() {
    let env = TestEnv::new().hook_account([2u8; 20]);
    let exit = env.invoke::<ForeignWriter>(2);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(
        env.state_typed::<u64>(b"scratch"),
        Some(rshooks_core::NOT_AUTHORIZED as u64)
    );
    assert_eq!(exit.code, rshooks_core::PREVIOUS_FAILURE_PREVENTS_RETRY);
}
