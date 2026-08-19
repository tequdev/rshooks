//! Integration tests for `TestEnv::invoke_cbak` (P2-E — design §4 "cbak
//! execution"): the callback otxn/burden/generation swap, and its
//! invocation-scoped restoration afterward. Hand-rolled `NativeEntry`
//! tables, matching `control_leftovers.rs`'s pattern in this same directory.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    missing_docs
)]

use rshooks::decl::{HookChainEntries, NativeEntry};
use rshooks_testenv::prelude::*;

/// Reserves one emission slot, `prepare`s a minimal `ttPAYMENT` template
/// (`TransactionType = 0` -> wire bytes `[0x12, 0x00, 0x00]`, mirroring
/// `crates/rshooks-testenv/src/backend.rs`'s own `minimal_template()` test
/// helper), and emits it — then, *before* accepting, records this
/// invocation's own `otxn_burden`/`otxn_generation` into state (`main_*`),
/// so a test can prove they read back to the same values both before and
/// after an intervening `invoke_cbak` call.
fn emit_minimal_payment(_r: u32) -> i64 {
    let _ = rshooks::api::etxn::etxn_reserve(1);
    let template: [u8; 3] = [0x12, 0x00, 0x00];
    let mut prepared = [0u8; 256];
    let n = rshooks::api::etxn::prepare(&mut prepared, &template).expect("prepare");
    let mut hash = [0u8; 32];
    rshooks::api::etxn::emit(&mut hash, &prepared[..n]).expect("emit");

    let burden = rshooks::api::otxn::otxn_burden();
    let _ = rshooks::api::state::state_set(&burden.to_be_bytes(), b"main_burden");
    let generation = rshooks::api::otxn::otxn_generation();
    let _ = rshooks::api::state::state_set(&generation.to_be_bytes(), b"main_generation");

    rshooks::api::control::accept(b"emitted", 0);
}

/// Records the callback's own view of `otxn_id`/`otxn_burden`/`otxn_generation`
/// into state (`cbak_*`) — the values `TestEnv::invoke_cbak` seeds per
/// design §4: the otxn is the emitted transaction itself (`otxn_id` == the
/// hash `emit` returned for it), and burden/generation come straight from
/// its own `EmitDetails` fields (not incremented).
fn record_cbak_otxn(_r: u32) -> i64 {
    let mut id = [0u8; 32];
    rshooks::api::otxn::otxn_id(&mut id, 0).expect("otxn_id");
    let _ = rshooks::api::state::state_set(&id, b"cbak_otxn_id");

    let burden = rshooks::api::otxn::otxn_burden();
    let _ = rshooks::api::state::state_set(&burden.to_be_bytes(), b"cbak_burden");
    let generation = rshooks::api::otxn::otxn_generation();
    let _ = rshooks::api::state::state_set(&generation.to_be_bytes(), b"cbak_generation");

    rshooks::api::control::accept(b"cbak", 0);
}

fn accept_no_cbak(_r: u32) -> i64 {
    rshooks::api::control::accept(b"ok", 0);
}

struct Chain;
impl HookChainEntries for Chain {
    const ENTRIES: &'static [NativeEntry] = &[
        NativeEntry {
            index: 0,
            name: "emit_minimal_payment",
            hook: emit_minimal_payment,
            cbak: Some(record_cbak_otxn),
            can_emit: None,
        },
        NativeEntry {
            index: 1,
            name: "accept_no_cbak",
            hook: accept_no_cbak,
            cbak: None,
            can_emit: None,
        },
    ];
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20])
}

#[test]
fn cbak_sees_the_emitted_transaction_as_its_own_otxn() {
    let env = env();
    let exit = env.invoke::<Chain>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<Chain>(0, CbakOutcome::Success(txn.clone()));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");

    assert_eq!(env.state(b"cbak_otxn_id"), Some(txn.hash().to_vec()));
    // A non-emitted originating otxn's default burden/generation is (1, 0)
    // (`Backend::otxn_burden`/`otxn_generation`'s own `None` fallback), so
    // `etxn_burden`/`etxn_generation` for this hook's one emission were
    // `1 * 1 = 1` / `0 + 1 = 1` — the callback's otxn_burden/otxn_generation
    // read those same `EmitDetails` values directly, unincremented.
    assert_eq!(env.state(b"cbak_burden"), Some(1u64.to_be_bytes().to_vec()));
    assert_eq!(
        env.state(b"cbak_generation"),
        Some(1u32.to_be_bytes().to_vec())
    );
}

#[test]
fn invoke_cbak_does_not_leak_its_otxn_into_a_later_plain_invoke() {
    let env = env();
    let exit1 = env.invoke::<Chain>(0);
    assert_eq!(exit1.exit, ExitType::Accept, "{exit1:?}");
    assert_eq!(env.state(b"main_burden"), Some(1u64.to_be_bytes().to_vec()));
    assert_eq!(
        env.state(b"main_generation"),
        Some(0u32.to_be_bytes().to_vec())
    );

    let txn = env.emitted()[0].clone();
    let cbak_exit = env.invoke_cbak::<Chain>(0, CbakOutcome::Success(txn));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");

    // A later plain `invoke` must see the *original* seeded otxn again
    // (default, non-emitted: burden 1 / generation 0) — not the callback's
    // own otxn/otxn_emitted values leaking through.
    let exit2 = env.invoke::<Chain>(0);
    assert_eq!(exit2.exit, ExitType::Accept, "{exit2:?}");
    assert_eq!(env.state(b"main_burden"), Some(1u64.to_be_bytes().to_vec()));
    assert_eq!(
        env.state(b"main_generation"),
        Some(0u32.to_be_bytes().to_vec())
    );
}

#[test]
fn invoke_cbak_failure_outcome_still_swaps_the_otxn() {
    let env = env();
    let _ = env.invoke::<Chain>(0);
    let txn = env.emitted()[0].clone();

    let cbak_exit = env.invoke_cbak::<Chain>(0, CbakOutcome::Failure(txn.clone()));
    assert_eq!(cbak_exit.exit, ExitType::Accept, "{cbak_exit:?}");
    assert_eq!(env.state(b"cbak_otxn_id"), Some(txn.hash().to_vec()));
}

#[test]
#[should_panic(expected = "entry 1 declares no #[cbak] body")]
fn invoke_cbak_on_an_entry_with_no_cbak_panics() {
    let env = env();
    let _ = env.invoke::<Chain>(0);
    let txn = env.emitted()[0].clone();
    let _ = env.invoke_cbak::<Chain>(1, CbakOutcome::Success(txn));
}
