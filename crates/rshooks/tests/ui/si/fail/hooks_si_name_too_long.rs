//! The declared `HookParameterName`'s total encoded length must fit the
//! protocol's 32-byte `HookParameterName` limit
//! (`docs/STATE_INTERFACE_DESIGN.md` §1.3): `4 + 1 + 1 + Sum(2 +
//! name_len)`. Two maximum-length (16-byte) key field names push the total
//! well past 32, even though their combined key payload (2 bytes) is far
//! under the 31-byte key-payload limit.

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(
        id = 0,
        key(abcdefghijklmnop: u8, bbcdefghijklmnop: u8),
        value(amount: u64)
    )]
    balance: State<Balance>,
}

fn main() {}
