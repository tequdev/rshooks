//! End-to-end: a `#[hooks]` struct declaring `#[state_interface(..)]` fields
//! (`docs/STATE_INTERFACE_DESIGN.md`) — the design doc's own worked example,
//! a keyed `balances` state and a singleton `paused` state — driven through
//! `TestEnv::invoke` and read back via `TestEnv::state` (raw bytes), so the
//! exact on-ledger `HookStateKey`/`HookStateData` bytes can be pinned
//! against the design doc §7 spec vectors.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use rshooks::exit::HookResult;
use rshooks::types::AccountId;
use rshooks::{accept, hooks};
use rshooks_testenv::prelude::*;

#[hooks]
pub struct Treasury {
    /// Keyed declaration — the design doc's own worked example.
    #[state_interface(
        id = 0,
        key(account: AccountId, token: u32),
        value(amount: u64, updated: u32)
    )]
    balances: State<Balance>,

    /// Singleton declaration — no key fields.
    #[state_interface(id = 1, value(paused: u8))]
    paused: State<Config>,
}

#[hooks]
impl Treasury {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let entry = self.state.balances.at((WORKED_ACCOUNT, 42u32));
        let _ = entry.set(&Balance {
            amount: 1000,
            updated: 12345,
        });
        let _ = self.state.paused.set(&Config { paused: 1 });
        accept!(b"treasury", 0)
    }
}

/// The design doc §7 spec vector's account: `4B4E9C06F24296074F7BC48F92A97916C6DC5EA9`.
const WORKED_ACCOUNT: AccountId = AccountId([
    0x4B, 0x4E, 0x9C, 0x06, 0xF2, 0x42, 0x96, 0x07, 0x4F, 0x7B, 0xC4, 0x8F, 0x92, 0xA9, 0x79, 0x16,
    0xC6, 0xDC, 0x5E, 0xA9,
]);

/// `docs/STATE_INTERFACE_DESIGN.md` §7's key vector: `004B4E9C06F24296074F7BC48F92A97916C6DC5EA90000002A00000000000000`.
const WORKED_KEY: [u8; 32] = [
    0x00, // State ID
    0x4B, 0x4E, 0x9C, 0x06, 0xF2, 0x42, 0x96, 0x07, 0x4F, 0x7B, 0xC4, 0x8F, 0x92, 0xA9, 0x79, 0x16,
    0xC6, 0xDC, 0x5E, 0xA9, // account
    0x00, 0x00, 0x00, 0x2A, // token = 42, BE
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 7 zero-padding bytes
];

/// `docs/STATE_INTERFACE_DESIGN.md` §7's data vector: `00000000000003E800003039`.
const WORKED_VALUE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xE8, // amount = 1000, BE
    0x00, 0x00, 0x30, 0x39, // updated = 12345, BE
];

/// The singleton declaration's key: State ID `0x01` followed by 31 zero
/// bytes (`docs/STATE_INTERFACE_DESIGN.md` §1.6).
const SINGLETON_KEY: [u8; 32] = {
    let mut key = [0u8; 32];
    key[0] = 0x01;
    key
};

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
}

#[test]
fn keyed_entry_matches_the_design_docs_spec_vector_key_and_data() {
    let e = env();
    let exit = e.invoke::<Treasury>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");

    assert_eq!(e.state(&WORKED_KEY), Some(WORKED_VALUE.to_vec()));
}

#[test]
fn singleton_entry_matches_state_id_plus_thirty_one_zero_bytes() {
    let e = env();
    let exit = e.invoke::<Treasury>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");

    assert_eq!(e.state(&SINGLETON_KEY), Some(vec![1u8]));
}

#[test]
fn typed_accessors_round_trip_through_the_generated_value_structs() {
    // `Treasury.state.balances.at(..).get()` reads through the real Hook
    // API host call, which has no backend installed outside an active
    // `TestEnv::invoke` — read back via `TestEnv::state_typed` instead,
    // which decodes straight from the environment's own state store (the
    // generated `Balance`/`Config` structs already implement `FromBytes`).
    let e = env();
    let exit = e.invoke::<Treasury>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");

    let balance = e
        .state_typed::<Balance>(&WORKED_KEY)
        .expect("entry present");
    assert_eq!(balance.amount, 1000);
    assert_eq!(balance.updated, 12345);

    let config = e
        .state_typed::<Config>(&SINGLETON_KEY)
        .expect("entry present");
    assert_eq!(config.paused, 1);
}

#[test]
fn a_different_key_arguments_pair_reads_back_absent() {
    let e = env();
    let exit = e.invoke::<Treasury>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");

    // State ID 0 || 20 zero bytes (account) || token = 7 (BE) || 7 zero
    // padding bytes — a key nothing in `main` ever wrote to.
    let mut other_key = [0u8; 32];
    other_key[24] = 7;
    assert_eq!(e.state(&other_key), None);
}
