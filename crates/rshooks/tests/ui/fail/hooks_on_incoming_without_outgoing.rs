//! `on_incoming` and `on_outgoing` must be specified together.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on_incoming = [Payment])]
    fn main() -> i64 {
        0
    }
}

fn main() {}
