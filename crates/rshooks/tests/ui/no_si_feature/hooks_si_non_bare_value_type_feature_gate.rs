//! With `unstable-state-interface` off, `#[state_interface(..)]` is
//! rejected with the feature-gate diagnostic even when the field's value
//! type is also shaped wrong (a non-bare, path-qualified type) — the
//! feature gate wins over the shape check, the same gate-ordering rule
//! `#[cbak(..)]` extra arguments follow against the signature interface's
//! own unconditional rejection.

use rshooks::decl::State;
use rshooks::hooks;

mod inner {
    pub struct Balance;
}

#[hooks]
struct Vault {
    #[state_interface(id = 0, value(amount: u64))]
    balance: State<inner::Balance>,
}

fn main() {}
