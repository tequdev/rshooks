//! With `unstable-state-interface` off, `#[state_interface(..)]` — the Hook
//! State Interface draft (`docs/STATE_INTERFACE_DESIGN.md`) — is rejected
//! with a diagnostic naming the feature, rather than being parsed as a
//! declaration.

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(id = 0, value(amount: u64))]
    balance: State<Balance>,
}

fn main() {}
