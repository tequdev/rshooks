//! `#[cfg]`/`#[cfg_attr]` are not allowed on a `self` receiver
//! (HOOKS_SELF_RECEIVER_DESIGN.md §3.3): the receiver shape — and, for an
//! entry, the has_self-driven discovery/selected wrapper it feeds — is
//! decided once at macro-expansion time, before `cfg` resolves, so a
//! conditional receiver would let that decision diverge from what
//! actually compiles.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(#[cfg(target_arch = "wasm32")] &self) -> i64 {
        0
    }
}

fn main() {}
