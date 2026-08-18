//! `#[hooks]` must be applied to a struct or an inherent impl — an enum is
//! rejected.

use rshooks::hooks;

#[hooks]
enum Vault {
    A,
    B,
}

fn main() {}
