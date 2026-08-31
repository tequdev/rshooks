//! A `#[hooks]` struct declaring state interface fields
//! (`docs/STATE_INTERFACE_DESIGN.md` §2) — the design doc's own worked
//! example: a keyed `balances` declaration and a singleton `paused`
//! declaration. Never invokes the entry (mirrors
//! `tests/ui/sig/pass/hooks_sig_params.rs`'s reasoning): this only proves
//! the generated value structs/marker/`StateSpec` impls compile and the
//! typed accessors resolve.

use rshooks::decl::HookChainEntries;
use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;
use rshooks::types::AccountId;

#[hooks]
struct Treasury {
    /// `id(0): key(account: AccountId, token: u32), value(amount: u64, updated: u32)`.
    #[state_interface(
        id = 0,
        key(account: AccountId, token: u32),
        value(amount: u64, updated: u32)
    )]
    balances: rshooks::decl::State<Balance>,

    /// `id(1): value(paused: u8)` — no key fields, a singleton.
    #[state_interface(id = 1, value(paused: u8))]
    paused: rshooks::decl::State<Config>,
}

#[hooks]
impl Treasury {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

fn main() {
    let entries = <Treasury as HookChainEntries>::ENTRIES;
    assert_eq!(entries.len(), 1);

    assert_eq!(Balance::LEN, 8 + 4);
    assert_eq!(Config::LEN, 1);

    // Keyed accessor: `KeyArgs` is a tuple of the declared key fields, in
    // order.
    let entry = Treasury.state.balances.at((AccountId::default(), 42u32));
    assert!(entry.get().is_err());
    assert!(entry.set(&Balance {
        amount: 1000,
        updated: 12345,
    })
    .is_err());

    // Singleton accessor: no `.at(..)` needed, `KeyArgs = ()`.
    assert!(Treasury.state.paused.get().is_err());
    assert!(Treasury.state.paused.set(&Config { paused: 1 }).is_err());
}
