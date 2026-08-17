//! `required` and `default = <expr>` are mutually exclusive on a parameter
//! field.

use rshooks::hooks;

#[hooks]
struct Vault {
    #[hook_param(name = b"CFG", required, default = [0u8; 4])]
    cfg: rshooks::decl::HookParam<[u8; 4]>,
}

fn main() {}
