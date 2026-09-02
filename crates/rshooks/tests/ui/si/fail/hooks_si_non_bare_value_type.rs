//! The field type must be `State<VName>` where `VName` is a bare
//! identifier — the macro generates `struct VName` from the `value(..)`
//! schema, so a path-qualified or otherwise non-bare spelling is rejected.

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
