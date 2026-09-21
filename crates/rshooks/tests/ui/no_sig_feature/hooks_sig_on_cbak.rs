//! With `unstable-param-sig-interface` off, a `#[cbak(..)]` fn with two
//! extra arguments still gets the cbak-specific "at most one argument"
//! rejection, not the feature-hint diagnostic `hooks_sig_feature_gate.rs`
//! pins for `#[hook(..)]` — a `#[cbak(..)]` fn's single allowed argument is
//! the callback outcome, not a signature parameter, so the interface (draft
//! or not) never applies to it.

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
    fn cbak(&self, outcome: u32, count: u16) -> i64 {
        i64::from(count) + i64::from(outcome)
    }
}

fn main() {}
