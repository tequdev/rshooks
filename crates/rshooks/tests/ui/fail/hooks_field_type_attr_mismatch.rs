//! A `#[state(..)]` attribute on a field typed `HookParam<V>` instead of
//! `State<V>`.

use rshooks::decl::HookParam;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state(key = b"counter")]
    counter: HookParam<u64>,
}

fn main() {}
