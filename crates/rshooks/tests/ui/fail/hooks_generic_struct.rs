//! A `#[hooks]` chain-declaration struct cannot be generic.

use rshooks::hooks;

#[hooks]
struct Vault<T>;

fn main() {}
