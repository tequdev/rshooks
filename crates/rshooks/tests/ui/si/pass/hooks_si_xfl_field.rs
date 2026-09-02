//! A `#[hooks]` struct declaring state interface fields that use `XFL`
//! (`docs/STATE_INTERFACE_DESIGN.md` §1.5, XAS-010d's `0x80` type code) as
//! both a value field and a key field — `XFL` is fixed-width (8 bytes), so
//! it is legal in either position. Never invokes the entry (mirrors
//! `tests/ui/sig/pass/hooks_sig_params.rs`'s reasoning): this only proves
//! the generated value structs/marker/`StateSpec` impls compile and the
//! typed accessors resolve.

use rshooks::decl::HookChainEntries;
use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;
use rshooks::types::AccountId;
use rshooks::xfl::XFL;

#[hooks]
struct Treasury {
    /// `id(0): key(account: AccountId), value(rate: XFL, updated: u32)`.
    #[state_interface(id = 0, key(account: AccountId), value(rate: XFL, updated: u32))]
    rates: rshooks::decl::State<Rate>,

    /// `id(1): key(rate: XFL), value(count: u64)` — keyed by `XFL`.
    #[state_interface(id = 1, key(rate: XFL), value(count: u64))]
    by_rate: rshooks::decl::State<Count>,
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

    assert_eq!(Rate::LEN, 8 + 4);
    assert_eq!(Count::LEN, 8);

    let one = XFL::from_raw_bits(0x54838D7EA4C68000u64 as i64);

    // Keyed accessor: `KeyArgs` is a tuple of the declared key fields.
    let entry = Treasury.state.rates.at(AccountId::default());
    assert!(entry.get().is_err());
    assert!(entry
        .set(&Rate {
            rate: one,
            updated: 12345,
        })
        .is_err());

    // Keyed by `XFL`.
    let by_rate = Treasury.state.by_rate.at(one);
    assert!(by_rate.get().is_err());
    assert!(by_rate.set(&Count { count: 1 }).is_err());
}
