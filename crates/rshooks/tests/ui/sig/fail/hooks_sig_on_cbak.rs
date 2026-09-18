//! A `#[cbak(..)]` fn must take no arguments other than `&self` — a
//! callback's originating transaction is the emitted transaction, not the
//! invocation, so the signature parameter interface
//! (`docs/PARAM_SIGNATURE_DESIGN.md` §1) does not apply.

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
    fn cbak(&self, count: u16) -> i64 {
        i64::from(count)
    }
}

fn main() {}
