//! A `#[hooks]` chain-declaration struct cannot be a tuple struct.

use rshooks::hooks;

#[hooks]
struct Vault(u8);

fn main() {}
