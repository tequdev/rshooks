//! A non-attributed helper (HOOKS_SELF_RECEIVER_DESIGN.md §3.3) taking
//! `self` by value — helpers accept the same receiver shapes as entries
//! (no receiver, or bare `&self`); this gets the same general "use
//! `&self`" diagnostic entries get for the same shape.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main() -> i64 {
        0
    }

    fn helper(self) -> i64 {
        0
    }
}

fn main() {}
