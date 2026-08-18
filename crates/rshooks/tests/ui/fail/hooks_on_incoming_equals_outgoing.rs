//! `on_incoming`/`on_outgoing` must not describe the same transaction-type
//! set — use `on` instead.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on_incoming = [Payment, Invoke], on_outgoing = [Invoke, Payment])]
    fn main() -> i64 {
        0
    }
}

fn main() {}
