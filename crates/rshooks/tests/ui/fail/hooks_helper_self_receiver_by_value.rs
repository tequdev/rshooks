//! A non-attributed helper (HOOKS_SELF_RECEIVER_DESIGN.md §3.3) taking
//! `self` by value — unlike an entry (which requires exactly `&self`), a
//! helper still accepts either no receiver or bare `&self`; this gets the
//! same general "use `&self`" diagnostic entries get for the same rejected
//! shape.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> i64 {
        0
    }

    fn helper(self) -> i64 {
        0
    }
}

fn main() {}
