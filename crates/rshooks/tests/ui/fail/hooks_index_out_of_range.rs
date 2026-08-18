//! A `#[hook(..)]` index outside the `0..=9` chain-position range.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(10, on = [Invoke])]
    fn main() -> i64 {
        0
    }
}

fn main() {}
