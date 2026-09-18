//! A `#[hook(..)]` entry declaring an `XFL` signature parameter
//! (`docs/PARAM_SIGNATURE_DESIGN.md` §2, XAS-010d's `0x80` type code).
//! Never invokes the entry: see `hooks_sig_params.rs`'s doc comment for why
//! (the generated prologue's `Err` arm reaches `rollback!`, which — like
//! `accept!` — hangs the process without an installed backend rather than
//! returning).

use rshooks::decl::HookChainEntries;
use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;
use rshooks::xfl::XFL;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    /// `rate`(0): `XFL` (XAS-010d `XFL`, 0x80).
    #[hook(0, on = [Invoke])]
    fn set_rate(&self, rate: XFL) -> HookResult {
        Ok(Accept::new(b"ok", rate.raw_bits()))
    }
}

fn main() {
    let entries = <Vault as HookChainEntries>::ENTRIES;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].index, 0);
    assert_eq!(entries[0].name, "set_rate");
}
