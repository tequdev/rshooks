//! `required` may only be specified once on a parameter field.

use rshooks::hooks;

#[hooks]
struct Vault {
    #[hook_param(name = b"CFG", required, required)]
    cfg: rshooks::decl::HookParam<[u8; 4]>,
}

fn main() {}
