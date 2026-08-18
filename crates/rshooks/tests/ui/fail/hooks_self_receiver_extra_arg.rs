//! A `#[hook(..)]` entry taking `&self` plus a further parameter — entries
//! accept no arguments other than `&self` (HOOKS_SELF_RECEIVER_DESIGN.md
//! §3.2/§3.3): `&self` opts an entry into reading declared fields, it does
//! not open the door to ordinary parameters too.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self, x: i64) -> i64 {
        x
    }
}

fn main() {}
