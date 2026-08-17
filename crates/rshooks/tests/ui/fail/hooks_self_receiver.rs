//! A `#[hook(..)]` entry taking `&self` — entry functions must be stateless
//! associated functions, not methods.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> i64 {
        0
    }
}

fn main() {}
