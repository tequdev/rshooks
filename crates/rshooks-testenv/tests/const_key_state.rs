//! End-to-end: a `#[state(key = &CONST)]` field whose key expression is a
//! non-literal constant (here a `&'static StateKey`) — the `#[hooks]`
//! macro's `rshooks::state::ConstKey`-promoted `with_key_bytes` codegen —
//! driven through `TestEnv::invoke` and read back via `TestEnv::state`, so
//! the exact on-ledger key bytes can be pinned against the un-promoted
//! `StateKeyEncode::encode` path this mechanism replaces.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use rshooks::exit::HookResult;
use rshooks::types::StateKey;
use rshooks::{accept, hooks, pad};
use rshooks_testenv::prelude::*;

/// The right-padded, non-literal constant key — mirrors
/// `examples/09_state-foreign`'s `ENABLED_KEY`.
const FLAG_KEY: StateKey = StateKey(pad!(b"flag"));

#[hooks]
pub struct ConstKeyed {
    /// Keyed by a non-literal `const`, not a byte-string literal.
    #[state(key = &FLAG_KEY)]
    flag: State<u8>,
}

#[hooks]
impl ConstKeyed {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let _ = self.state.flag.set(&1u8);
        accept!(b"const-keyed", 0)
    }
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20])
}

#[test]
fn const_promoted_non_literal_key_writes_to_the_full_32_byte_key() {
    let e = env();
    let exit = e.invoke::<ConstKeyed>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");

    assert_eq!(e.state(&FLAG_KEY.0), Some(vec![1u8]));
}
