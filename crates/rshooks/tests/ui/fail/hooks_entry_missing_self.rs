//! A `#[hook(..)]` entry with no receiver at all — entries require exactly
//! `&self` (HOOKS_SELF_RECEIVER_DESIGN.md §3.2): the chain declaration is
//! passed by shared reference, so a bare `fn main() -> i64` is rejected.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main() -> i64 {
        0
    }
}

fn main() {}
