//! With `unstable-param-sig-interface` off, an extra `ident: Type`
//! argument on a `#[hook(..)]` entry fn — otherwise a Hook Parameter
//! Signature Interface declaration (`docs/PARAM_SIGNATURE_DESIGN.md` §1) —
//! is rejected with a diagnostic naming the feature, rather than being
//! parsed as a signature parameter.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self, count: u16) -> i64 {
        i64::from(count)
    }
}

fn main() {}
