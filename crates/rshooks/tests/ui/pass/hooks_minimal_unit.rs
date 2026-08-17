//! Minimal `#[hooks]` chain: a unit struct with a single, argument-free
//! hook entry and no declared state or parameters.

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
