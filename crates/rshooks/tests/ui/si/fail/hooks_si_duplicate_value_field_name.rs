//! Field names must be unique within one `value(..)` record
//! (`docs/STATE_INTERFACE_DESIGN.md` §1.8).

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(id = 0, value(amount: u64, amount: u32))]
    balance: State<Balance>,
}

fn main() {}
