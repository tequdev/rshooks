//! A field must carry exactly one of `#[state]`, `#[hook_param]`,
//! `#[otxn_param]`, `#[state_interface]` — never two.

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state(key = b"BAL")]
    #[state_interface(id = 0, value(amount: u64))]
    balance: State<Balance>,
}

fn main() {}
