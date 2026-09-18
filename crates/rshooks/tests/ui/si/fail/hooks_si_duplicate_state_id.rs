//! State IDs must be unique across every `#[state_interface(..)]` field on
//! the struct (`docs/STATE_INTERFACE_DESIGN.md` §1.3).

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(id = 0, value(amount: u64))]
    balance: State<Balance>,

    #[state_interface(id = 0, value(paused: u8))]
    config: State<Config>,
}

fn main() {}
