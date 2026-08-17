//! A `#[cbak(..)]` index with no matching `#[hook(..)]` declared.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main() -> i64 {
        0
    }

    #[cbak(1)]
    fn cbak() -> i64 {
        0
    }
}

fn main() {}
