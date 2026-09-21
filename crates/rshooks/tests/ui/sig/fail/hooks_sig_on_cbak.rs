//! A `#[cbak(..)]` fn takes at most one argument after `&self` — the
//! callback outcome (`u32` or `rshooks::exit::EmitOutcome`). Signature
//! parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §1) apply to `#[hook(..)]`
//! only.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self, count: u16) -> i64 {
        i64::from(count)
    }

    #[cbak(0)]
    fn cbak(&self, outcome: u32, count: u16) -> i64 {
        i64::from(count) + i64::from(outcome)
    }
}

fn main() {}
