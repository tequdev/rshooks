//! A non-attributed helper (HOOKS_SELF_RECEIVER_DESIGN.md §3.3) taking
//! `&mut self` — chain handles are zero-sized and immutable, so this gets
//! the same mutability-specific diagnostic entries get for the same shape.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main() -> i64 {
        0
    }

    fn helper(&mut self) -> i64 {
        0
    }
}

fn main() {}
