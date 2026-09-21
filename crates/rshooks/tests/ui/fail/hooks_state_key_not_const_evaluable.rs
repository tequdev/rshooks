//! A `#[state(key = &<expr>)]` field whose key expression cannot be
//! const-evaluated (it calls an ordinary, non-`const` function) — the
//! `#[hooks]` macro's `with_key_bytes` codegen const-promotes `<expr>`
//! itself inside a `const` block, so a key expression that isn't
//! const-evaluable fails to compile right here rather than silently
//! falling back to a slower runtime path.

use rshooks::decl::State;
use rshooks::hooks;
use rshooks::types::StateKey;

fn make_key() -> StateKey {
    StateKey([0u8; 32])
}

#[hooks]
struct Vault {
    #[state(key = &make_key())]
    entry: State<u64>,
}

fn main() {}
