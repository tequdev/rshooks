//! `#[cfg]`/`#[cfg_attr]` are not allowed on a hook entry function.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[cfg(target_arch = "wasm32")]
    #[hook(0, on = [Invoke])]
    fn main() -> i64 {
        0
    }
}

fn main() {}
