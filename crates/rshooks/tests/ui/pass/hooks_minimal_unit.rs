//! Minimal `#[hooks]` chain: a unit struct with a single hook entry taking
//! `&self` and no declared state or parameters.

use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

fn main() {}
