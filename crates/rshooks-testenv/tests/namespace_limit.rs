//! `TOO_MANY_NAMESPACES` boundary tests (design §4 / `maxNamespaces`,
//! `Xahau/xahaud` `dev`, `include/xrpl/hook/Enum.h`): the cap is per
//! *account*, evaluated against the account's own existing hook namespaces
//! (seeded state entries, or ones a previously accepted `invoke` created),
//! not just the namespaces touched by the invocation in progress.

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

/// The account every hook entry in this file targets — kept equal to the
/// harness's default `hook_account` ([`Default`] all-zero) so
/// `state_foreign_set` never takes the foreign/grant-checked path (design
/// §5.5: "a write to this hook's own account never consults grants at
/// all").
const ACC: [u8; 20] = [0u8; 20];

/// A distinct namespace, keyed by `i` — never `[0u8; 32]` (the harness's
/// default `own_namespace`), so seeded namespaces never collide with an
/// unrelated default.
const fn ns(i: u16) -> [u8; 32] {
    let mut n = [1u8; 32];
    n[30] = (i >> 8) as u8;
    n[31] = (i & 0xFF) as u8;
    n
}

#[hooks]
pub struct NsLimit {
    #[state(key = b"scratch")]
    scratch: State<u64>,
}

#[hooks]
impl NsLimit {
    /// Writes a new key into `ns(300)` — a namespace never seeded in this
    /// file's tests.
    #[hook(0, on = [Invoke])]
    fn write_new_namespace_300(&self) -> HookResult {
        let code = match rshooks::api::state::state_foreign_set(b"v", b"K", &ns(300), &ACC) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    /// Writes a new key into `ns(0)` — always seeded by this file's
    /// "account already full" test.
    #[hook(1, on = [Invoke])]
    fn write_existing_namespace_0(&self) -> HookResult {
        let code = match rshooks::api::state::state_foreign_set(b"v", b"K", &ns(0), &ACC) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    /// Writes a new key into `ns(500)`.
    #[hook(2, on = [Invoke])]
    fn write_new_namespace_500(&self) -> HookResult {
        let code = match rshooks::api::state::state_foreign_set(b"v", b"K", &ns(500), &ACC) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    /// Writes a new key into `ns(501)`, distinct from `ns(500)`.
    #[hook(3, on = [Invoke])]
    fn write_new_namespace_501(&self) -> HookResult {
        let code = match rshooks::api::state::state_foreign_set(b"v", b"K", &ns(501), &ACC) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    /// Writes a new key into `ns(600)`, then always rolls back.
    #[hook(4, on = [Invoke])]
    fn write_namespace_600_then_rollback(&self) -> HookResult {
        let code = match rshooks::api::state::state_foreign_set(b"v", b"K", &ns(600), &ACC) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        rollback!(b"", code)
    }

    /// Writes a new key into `ns(601)`, distinct from `ns(600)`.
    #[hook(5, on = [Invoke])]
    fn write_new_namespace_601(&self) -> HookResult {
        let code = match rshooks::api::state::state_foreign_set(b"v", b"K", &ns(601), &ACC) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }
}

/// Seeds `ACC` with `count` distinct existing namespaces (`ns(0)..ns(count)`),
/// each holding one live state entry.
fn seed_full_env(count: u16) -> TestEnv {
    let mut env = TestEnv::new();
    for i in 0..count {
        env = env.foreign_state_entry(ns(i), ACC, b"seed", &[1]);
    }
    env
}

#[test]
fn account_already_at_the_namespace_cap_rejects_a_new_namespace() {
    let env = seed_full_env(256);
    let exit = env.invoke::<NsLimit>(0);
    assert_eq!(exit.code, rshooks_core::TOO_MANY_NAMESPACES);
}

#[test]
fn account_already_at_the_namespace_cap_still_accepts_writes_into_an_existing_namespace() {
    let env = seed_full_env(256);
    let exit = env.invoke::<NsLimit>(1);
    assert_eq!(exit.code, 0);
}

#[test]
fn a_namespace_created_by_an_accepted_invoke_persists_and_counts_against_a_later_invoke() {
    let env = seed_full_env(255);

    let first = env.invoke::<NsLimit>(2);
    assert_eq!(first.exit, ExitType::Accept);
    assert_eq!(first.code, 0);

    // `ns(500)` pushed the account to exactly 256 namespaces; a second,
    // distinct new namespace must now be rejected.
    let second = env.invoke::<NsLimit>(3);
    assert_eq!(second.code, rshooks_core::TOO_MANY_NAMESPACES);
}

#[test]
fn a_namespace_created_by_a_rolled_back_invoke_never_persists() {
    let env = seed_full_env(255);

    let first = env.invoke::<NsLimit>(4);
    assert_eq!(first.exit, ExitType::Rollback);

    // `ns(600)`'s claim was discarded with the rest of the rollback, so the
    // account is still at 255: one more new namespace must succeed.
    let second = env.invoke::<NsLimit>(5);
    assert_eq!(second.exit, ExitType::Accept);
    assert_eq!(second.code, 0);

    // ...and that acceptance really did commit `ns(601)`, bringing the
    // account to exactly 256: a further new namespace is rejected again.
    let third = env.invoke::<NsLimit>(0);
    assert_eq!(third.code, rshooks_core::TOO_MANY_NAMESPACES);
}
