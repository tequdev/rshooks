//! A literal `#[hook_param(name = b"...")]` name must decode to at least 1
//! byte — an empty byte string is rejected.

use rshooks::hooks;

#[hooks]
struct Vault {
    #[hook_param(name = b"")]
    cfg: rshooks::decl::HookParam<[u8; 4]>,
}

fn main() {}
