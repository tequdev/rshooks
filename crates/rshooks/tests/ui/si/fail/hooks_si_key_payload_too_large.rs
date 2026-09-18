//! A key's total encoded field width must be <= 31 bytes — the 32-byte key
//! minus the 1-byte State ID (`docs/STATE_INTERFACE_DESIGN.md` §1.6). A
//! single `[u8; 32]` key field is already 32 bytes, leaving no room.

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(id = 0, key(owner: [u8; 32]), value(amount: u64))]
    balance: State<Balance>,
}

fn main() {}
