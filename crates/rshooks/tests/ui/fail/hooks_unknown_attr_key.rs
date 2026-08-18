//! An unknown key inside a `#[state(..)]` field attribute.

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state(bogus = b"counter")]
    counter: State<u64>,
}

fn main() {}
