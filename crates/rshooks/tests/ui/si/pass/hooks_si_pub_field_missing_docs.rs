//! With `#![deny(missing_docs)]`, a `pub` `#[hooks]` struct with a `pub`
//! `#[state_interface(..)]` field must still compile: the generated value
//! struct and its fields are `pub` too (following the field's own
//! visibility, same rule the marker follows), so they need real doc
//! comments, not just the marker's `#[doc(hidden)]` escape.

#![deny(missing_docs)]

use rshooks::decl::HookChainEntries;
use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;
use rshooks::types::AccountId;

/// A documented chain struct with a documented, `pub` state interface field.
#[hooks]
pub struct Treasury {
    /// Per-`(account, token)` balance.
    #[state_interface(
        id = 0,
        key(account: AccountId, token: u32),
        value(amount: u64, updated: u32)
    )]
    pub balances: rshooks::decl::State<Balance>,
}

#[hooks]
impl Treasury {
    /// Never invoked — see `hooks_si_worked_example.rs`'s doc comment for
    /// why.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

fn main() {
    let entries = <Treasury as HookChainEntries>::ENTRIES;
    assert_eq!(entries.len(), 1);
    assert_eq!(Balance::LEN, 8 + 4);

    let entry = Treasury.state.balances.at((AccountId::default(), 42u32));
    assert!(entry.get().is_err());
}
