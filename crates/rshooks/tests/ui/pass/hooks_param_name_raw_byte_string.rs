//! A raw byte-string literal (`br"..."` / `br#"..."#`) is accepted
//! anywhere a plain `b"..."` literal is for `#[hook_param(name = ..)]` /
//! `#[otxn_param(name = ..)]`.

use rshooks::hooks;

#[hooks]
struct Vault {
    #[hook_param(name = br"CFG")]
    cfg: rshooks::decl::HookParam<[u8; 4]>,

    #[otxn_param(name = br#"DEF"#)]
    def: rshooks::decl::OtxnParam<[u8; 4]>,
}

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main() -> i64 {
        0
    }
}

fn main() {
    assert!(Vault.cfg.get().is_err());
    assert!(Vault.def.get().is_err());
}
