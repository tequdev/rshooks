//! A `#[hook(..)]` entry declaring signature parameters
//! (`docs/PARAM_SIGNATURE_DESIGN.md` §1) — the interface draft's own worked
//! example, `increment(account: AccountID, count: UInt16)`.
//! Never invokes the entry: see `hooks_typed_entry_result.rs`'s doc comment
//! for why (the generated prologue's `Err` arm reaches `rollback!`, which —
//! like `accept!` — hangs the process without an installed backend rather
//! than returning).

use rshooks::decl::HookChainEntries;
use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;
use rshooks::types::AccountId;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    /// `account`(0): `AccountId` (`STI_ACCOUNT`, 0x08). `count`(1): `u16`
    /// (`STI_UINT16`, 0x01). Both are already decoded by the time the body
    /// runs.
    #[hook(0, on = [Invoke])]
    fn increment(&self, account: AccountId, count: u16) -> HookResult {
        let _ = account;
        Ok(Accept::new(b"ok", i64::from(count)))
    }
}

fn main() {
    let entries = <Vault as HookChainEntries>::ENTRIES;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].index, 0);
    assert_eq!(entries[0].name, "increment");
}
