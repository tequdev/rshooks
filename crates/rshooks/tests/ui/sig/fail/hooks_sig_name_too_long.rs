//! A signature parameter's display name must be 1..=16 bytes
//! (`docs/PARAM_SIGNATURE_DESIGN.md` §1) — 17 ASCII letters is one byte too
//! many.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self, abcdefghijklmnopq: u16) -> i64 {
        i64::from(abcdefghijklmnopq)
    }
}

fn main() {}
