//! Two `#[cbak(..)]` entries declaring the same index.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn first(&self) -> i64 {
        0
    }

    #[cbak(0)]
    fn first_cbak(&self) -> i64 {
        0
    }

    #[hook(1, on = [Payment])]
    fn second(&self) -> i64 {
        0
    }

    #[cbak(0)]
    fn second_cbak(&self) -> i64 {
        0
    }
}

fn main() {}
