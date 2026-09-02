//! `value(..)` is required and must declare at least one field
//! (`docs/STATE_INTERFACE_DESIGN.md` §1.4).

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(id = 0)]
    balance: State<Balance>,
}

fn main() {}
