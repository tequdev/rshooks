//! `default = <expr>` may only be specified once on a parameter field —
//! the earlier value must not be silently discarded.

use rshooks::hooks;

#[hooks]
struct Vault {
    #[hook_param(name = b"CFG", default = [0u8; 4], default = [1u8; 4])]
    cfg: rshooks::decl::HookParam<[u8; 4]>,
}

fn main() {}
