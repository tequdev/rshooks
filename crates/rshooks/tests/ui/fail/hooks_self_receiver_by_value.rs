//! Two more rejected receiver shapes, both getting the general "use
//! `&self`" diagnostic (HOOKS_SELF_RECEIVER_DESIGN.md §6.2): a `#[hook(..)]`
//! entry taking `self` by value, and one taking a lifetime'd `&'a self` —
//! only bare `&self` (no lifetime) is accepted.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(self) -> i64 {
        0
    }
}

#[hooks]
struct VaultRef;

#[hooks]
impl VaultRef {
    #[hook(0, on = [Invoke])]
    fn main<'a>(&'a self) -> i64 {
        0
    }
}

fn main() {}
