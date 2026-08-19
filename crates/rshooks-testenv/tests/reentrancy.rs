//! `invoke` inside a hook (reentrancy) panics with
//! [`rshooks_core::backend::install`]'s message — design §4 says
//! reentrancy is rejected by the occupied backend slot, with no extra
//! machinery in this crate.

#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::*;
use rshooks_testenv::prelude::*;

#[hooks]
pub struct Reentrant {
    #[state(key = b"x")]
    x: State<u64>,
}

#[hooks]
impl Reentrant {
    #[hook(0, on = [Invoke])]
    fn recurse(&self) -> i64 {
        let inner = TestEnv::new();
        let _ = inner.invoke::<Reentrant>(0);
        accept!(b"", 0)
    }
}

#[test]
#[should_panic(expected = "a backend is already installed on this thread")]
fn reentrant_invoke_panics() {
    let env = TestEnv::new();
    let _ = env.invoke::<Reentrant>(0);
}
