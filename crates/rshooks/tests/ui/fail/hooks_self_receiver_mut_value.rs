//! A `#[hook(..)]` entry taking `mut self` (by value, mutable binding) —
//! only bare `&self` is accepted; this gets the general "use `&self`"
//! diagnostic (HOOKS_SELF_RECEIVER_DESIGN.md §6.2).

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(mut self) -> i64 {
        0
    }
}

fn main() {}
