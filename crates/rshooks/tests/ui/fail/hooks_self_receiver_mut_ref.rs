//! A `#[hook(..)]` entry taking `&mut self` — chain handles are zero-sized
//! and immutable, so this gets the mutability-specific diagnostic
//! (HOOKS_SELF_RECEIVER_DESIGN.md §6.2), distinct from the general
//! "use `&self`" one.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&mut self) -> i64 {
        0
    }
}

fn main() {}
