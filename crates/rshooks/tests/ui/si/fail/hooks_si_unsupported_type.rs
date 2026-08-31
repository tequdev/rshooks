//! A `value(..)` field's type must be one of the version-0 fixed-width
//! types (`docs/STATE_INTERFACE_DESIGN.md` §1.5) — `i64` isn't one of them
//! (`i64` is signed; the interface has no signed integer type).

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(id = 0, value(amount: i64))]
    balance: State<Balance>,
}

fn main() {}
