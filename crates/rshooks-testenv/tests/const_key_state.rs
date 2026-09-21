//! End-to-end: `#[state(key = &<expr>)]` fields whose key expression is a
//! non-literal constant, a `static`, or a `#[derive(HookKey)]` composite
//! key — the `#[hooks]` macro's const-promoted `with_key_bytes` codegen
//! const-promotes `<expr>` itself (rather than trying to name its type) and
//! hands the promoted reference to that concrete type's own
//! `StateKeyEncode::with_key_bytes` override — driven through
//! `TestEnv::invoke` and read back via `TestEnv::state`, pinning the exact
//! on-ledger key bytes for each supported key-expression shape.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use rshooks::exit::HookResult;
use rshooks::types::{AccountId, StateKey};
use rshooks::{HookKey, accept, hooks, pad};
use rshooks_testenv::prelude::*;

/// A plain `const`, referenced by `&` — mirrors
/// `examples/09_state-foreign`'s `ENABLED_KEY`.
const FLAG_KEY: StateKey = StateKey(pad!(b"flag"));

/// A `static`, referenced by `&`: const-promoting a reference to a
/// `static` inside a `const` block is exactly the shape this mechanism
/// must not regress on.
static COUNT_KEY: StateKey = StateKey(pad!(b"count"));

/// A composite, `#[derive(HookKey)]` key behind a `const` — a type
/// `rshooks::state::StateKeyEncode` covers but no single built-in type
/// (`[u8; N]`/`StateKey`/`EncodedStateKey`) can stand in for.
#[derive(HookKey, Clone, Copy)]
struct OwnerKey {
    owner: AccountId,
}

const OWNER_KEY: OwnerKey = OwnerKey {
    owner: AccountId([9u8; 20]),
};

#[hooks]
pub struct ConstKeyed {
    /// Keyed by a non-literal `const`, not a byte-string literal.
    #[state(key = &FLAG_KEY)]
    flag: State<u8>,
    /// Keyed by a `static`.
    #[state(key = &COUNT_KEY)]
    count: State<u8>,
    /// Keyed by a `#[derive(HookKey)]` struct behind a `const`.
    #[state(key = &OWNER_KEY)]
    owner: State<u8>,
}

#[hooks]
impl ConstKeyed {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let _ = self.state.flag.set(&1u8);
        let _ = self.state.count.set(&2u8);
        let _ = self.state.owner.set(&3u8);
        accept!(b"const-keyed", 0)
    }
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20])
}

#[test]
fn const_key_writes_to_its_own_full_32_byte_key() {
    let e = env();
    let exit = e.invoke::<ConstKeyed>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");

    assert_eq!(e.state(&FLAG_KEY.0), Some(vec![1u8]));
}

#[test]
fn static_key_writes_to_its_own_full_32_byte_key() {
    let e = env();
    let exit = e.invoke::<ConstKeyed>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");

    assert_eq!(e.state(&COUNT_KEY.0), Some(vec![2u8]));
}

#[test]
fn hook_key_derived_const_key_writes_to_its_own_real_length_key() {
    let e = env();
    let exit = e.invoke::<ConstKeyed>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");

    // `OwnerKey`'s real length is 20 bytes (a bare `AccountId` field), sent
    // unpadded — `TestEnv::state` left-pads it to the full 32-byte slot the
    // same way the host does.
    assert_eq!(e.state(&[9u8; 20]), Some(vec![3u8]));
}
