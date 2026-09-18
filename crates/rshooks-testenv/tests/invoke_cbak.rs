//! Integration tests for `TestEnv::invoke_cbak` (design §4 "cbak
//! execution"): the callback otxn/burden/generation swap, and its
//! invocation-scoped restoration afterward. Hand-rolled `NativeEntry`
//! tables.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    missing_docs
)]

use rshooks::decl::{HookChainEntries, NativeEntry};
use rshooks_testenv::prelude::*;

/// Reserves one emission slot, `prepare`s a minimal `ttPAYMENT` template
/// (`TransactionType = 0` plus the two fields `prepare` never fills in —
/// `sfAmount`/`sfDestination`, both `presence: "required"` for Payment in
/// `protocol_formats.json`), and emits it — then, *before* accepting,
/// records this invocation's own `otxn_burden`/`otxn_generation` into state
/// (`main_*`), so a test can prove they read back to the same values both
/// before and after an intervening `invoke_cbak` call.
fn emit_minimal_payment(_r: u32) -> i64 {
    let _ = rshooks::api::etxn::etxn_reserve(1);
    let mut template = vec![0x12, 0x00, 0x00]; // TransactionType = 0
    template.push(0x61); // Amount (6, 1): native 1 drop
    template.extend_from_slice(&0x4000_0000_0000_0001u64.to_be_bytes());
    template.push(0x83); // Destination (8, 3)
    template.push(20);
    template.extend_from_slice(&[0u8; 20]);
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
        // Same emitting body as index 0, but declares no `#[cbak]` — its
        // emitted transactions must never carry `EmitCallback`.
        NativeEntry {
            index: 2,
            name: "emit_minimal_payment_no_cbak",
            hook: emit_minimal_payment,
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
    // A non-emitted originating otxn's default burden/generation is (1, 0),
    // so this hook's one emission has burden `1 * 1 = 1` and generation
    // `0 + 1 = 1` — the callback reads those same `EmitDetails` values
    // directly, unincremented.
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

/// The `EmitDetails.EmitCallback` field (AccountID, VL-encoded: a 1-byte
/// length prefix of `20` then the account) is present, holding this
/// `TestEnv`'s own `hook_account`, exactly when the emitting entry declares
/// a `#[cbak]` body — and absent otherwise.
#[test]
fn emit_callback_presence_tracks_whether_the_entry_declares_a_cbak() {
    let with_cbak = env();
    let exit = with_cbak.invoke::<Chain>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let blob_with = with_cbak.emitted()[0].blob().to_vec();

    let without_cbak = env();
    let exit = without_cbak.invoke::<Chain>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let blob_without = without_cbak.emitted()[0].blob().to_vec();

    let (hdr, hdr_len) = rshooks::txn::codec::field_header(rshooks::sfield::sfEmitCallback);
    let mut expected = hdr[..hdr_len].to_vec();
    expected.push(20); // AccountID VL length prefix
    expected.extend_from_slice(&[1u8; 20]); // env()'s hook_account

    assert!(
        blob_with
            .windows(expected.len())
            .any(|w| w == expected.as_slice()),
        "EmitCallback region not found in the emitted blob: {blob_with:02x?}"
    );
    assert!(
        !blob_without.windows(hdr_len).any(|w| w == &hdr[..hdr_len]),
        "EmitCallback header unexpectedly present: {blob_without:02x?}"
    );
}

#[test]
#[should_panic(expected = "carries no EmitCallback")]
fn invoke_cbak_panics_for_a_transaction_emitted_by_an_entry_with_no_cbak() {
    let env = env();
    let exit = env.invoke::<Chain>(2); // declares cbak: None
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let txn = env.emitted()[0].clone();

    // Entry 0 does declare a `#[cbak]`, but this transaction was never
    // emitted by it — on-chain, `doHookCallback` would never invoke
    // anything for a transaction whose `EmitDetails` has no `EmitCallback`
    // at all.
    let _ = env.invoke_cbak::<Chain>(0, CbakOutcome::Success(txn));
}

#[test]
#[should_panic(expected = "do not match")]
fn invoke_cbak_panics_when_the_hook_account_changed_since_the_emit() {
    let env = env();
    let exit = env.invoke::<Chain>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let txn = env.emitted()[0].clone();

    let env = env.hook_account([9u8; 20]);
    let _ = env.invoke_cbak::<Chain>(0, CbakOutcome::Success(txn));
}
