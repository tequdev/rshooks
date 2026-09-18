//! The declared `HookParameterValue`'s total encoded length must fit the
//! protocol's `maxHookParameterValueSize()` limit (256 bytes — xahaud
//! `include/xrpl/hook/Enum.h`). Fifteen maximum-length (16-byte) value
//! field names push the total to 271 bytes, even though every field is
//! individually a 1-byte `u8`.

use rshooks::decl::State;
use rshooks::hooks;

#[hooks]
struct Vault {
    #[state_interface(
        id = 0,
        value(
            abcdefghijklmnop: u8,
            bbcdefghijklmnop: u8,
            cbcdefghijklmnop: u8,
            dbcdefghijklmnop: u8,
            ebcdefghijklmnop: u8,
            fbcdefghijklmnop: u8,
            gbcdefghijklmnop: u8,
            hbcdefghijklmnop: u8,
            ibcdefghijklmnop: u8,
            jbcdefghijklmnop: u8,
            kbcdefghijklmnop: u8,
            lbcdefghijklmnop: u8,
            mbcdefghijklmnop: u8,
            nbcdefghijklmnop: u8,
            obcdefghijklmnop: u8
        )
    )]
    balance: State<Balance>,
}

fn main() {}
