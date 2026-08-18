//! A literal `#[hook_param(name = b"...")]` name must decode to 1..=32
//! bytes (the Hook API's parameter-name length limit).

use rshooks::hooks;

#[hooks]
struct Vault {
    #[hook_param(name = b"THIS_NAME_IS_DEFINITELY_MORE_THAN_THIRTY_TWO_BYTES_LONG")]
    cfg: rshooks::decl::HookParam<[u8; 4]>,
}

fn main() {}
