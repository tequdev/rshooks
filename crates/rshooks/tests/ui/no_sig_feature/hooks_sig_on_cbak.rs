//! With `unstable-param-sig-interface` off, a `#[cbak(..)]` fn with an
//! extra argument still gets the cbak-specific rejection, not the
//! feature-hint diagnostic `hooks_sig_feature_gate.rs` pins for
//! `#[hook(..)]` — a callback's originating transaction is the emitted
//! transaction, not the invocation, so the signature parameter interface
//! (draft or not) never applies to it.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> i64 {
        0
    }

    #[cbak(0)]
    fn cbak(&self, count: u16) -> i64 {
        i64::from(count)
    }
}

fn main() {}
