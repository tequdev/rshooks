//! A value field name containing `_` does not match the interface draft's
//! `[A-Za-z][A-Za-z0-9]*` charset (`docs/STATE_INTERFACE_DESIGN.md` §1.2).

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(id = 0, value(min_amount: u64))]
    balance: State<Balance>,
}

fn main() {}
