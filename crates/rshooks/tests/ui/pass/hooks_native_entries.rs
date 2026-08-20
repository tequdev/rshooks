//! `<Struct as HookChainEntries>::ENTRIES` is populated correctly on the
//! native (non-wasm) side of a `#[hooks]` chain: two entries, one paired
//! with a `#[cbak]` and declaring `can_emit`, one bare — asserted by index,
//! name, cbak presence, and `can_emit` contents. Never invokes an entry:
//! without a backend installed (TESTENV_DESIGN.md §2.3), reaching an
//! `accept!`/`rollback!` inside a real entry body would hang the process
//! rather than return.

use rshooks::decl::HookChainEntries;
use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;
use rshooks::tx_type::TxType;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke], can_emit = [Payment])]
    fn main(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    #[cbak(0)]
    fn main_cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    #[hook(1, on = [Invoke])]
    fn second(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

fn main() {
    let entries = <Vault as HookChainEntries>::ENTRIES;
    assert_eq!(entries.len(), 2);

    let e0 = entries.iter().find(|e| e.index == 0).expect("index 0");
    assert_eq!(e0.name, "main");
    assert!(e0.cbak.is_some());
    // Declared (`can_emit = [Payment]`): `Some(&[..])`, never `None`.
    assert_eq!(e0.can_emit, Some([TxType::Payment].as_slice()));

    let e1 = entries.iter().find(|e| e.index == 1).expect("index 1");
    assert_eq!(e1.name, "second");
    assert!(e1.cbak.is_none());
    // `can_emit` never declared on this entry: `None` (unrestricted),
    // distinct from a declared-but-empty `Some(&[])`.
    assert_eq!(e1.can_emit, None);
}
