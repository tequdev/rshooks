//! A `#[hooks]` impl block declaring no `#[hook(..)]` entries at all.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    fn helper() -> i64 {
        0
    }
}

fn main() {}
