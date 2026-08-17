//! `#[cbak(..)]` takes only the index — no other arguments.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main() -> i64 {
        0
    }

    #[cbak(0, on = [Invoke])]
    fn cbak() -> i64 {
        0
    }
}

fn main() {}
