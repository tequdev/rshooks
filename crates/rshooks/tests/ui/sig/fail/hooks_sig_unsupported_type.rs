//! A `#[hook(..)]` entry's extra argument declares a signature parameter
//! (`docs/PARAM_SIGNATURE_DESIGN.md` §1), but only for a type implementing
//! `SigParamType` — `i64` isn't one of them (`i64` is signed; the
//! interface has no signed integer type).

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
