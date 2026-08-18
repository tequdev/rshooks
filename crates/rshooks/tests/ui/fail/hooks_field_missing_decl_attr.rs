//! A chain-struct field with no `#[state]`/`#[hook_param]`/`#[otxn_param]`
//! declaration attribute.

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    counter: State<u64>,
}

fn main() {}
