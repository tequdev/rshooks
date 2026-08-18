//! Two `#[hook(..)]` entries declaring the same index.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn first(&self) -> i64 {
        0
    }

    #[hook(0, on = [Payment])]
    fn second(&self) -> i64 {
        0
    }
}

fn main() {}
