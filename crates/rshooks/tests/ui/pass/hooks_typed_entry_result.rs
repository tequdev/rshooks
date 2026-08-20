//! `#[hook]`/`#[cbak]` entries returning `HookResult` (the typed exit form,
//! `.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md`) — including a
//! `?`-propagated `hook_errors!` enum with a message clause on one entry —
//! and the generated `HookChainEntries::ENTRIES` table is unaffected by
//! which entry in the chain does the propagating.
//! Never invokes an entry: see `hooks_native_entries.rs`'s doc comment for
//! why (reaching `accept!`/`rollback!` — or, for `HookResult`, the `Ok`/
//! `Err` arms `EntryReturn::finish` calls them from — without an installed
//! backend hangs the process rather than returning).

use rshooks::decl::HookChainEntries;
use rshooks::exit::{Accept, HookResult};
use rshooks::{hook_errors, hooks};

hook_errors! {
    /// Demo error for the `?`-propagation path below.
    enum DemoError {
        /// The only failure this fixture models.
        Bad = 1 => b"demo: bad",
    }
}

fn helper(ok: bool) -> Result<u32, DemoError> {
    if ok { Ok(1) } else { Err(DemoError::Bad) }
}

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    /// Typed entry: `?` propagates `DemoError` into `HookResult` via the
    /// `hook_errors!`-generated `From<DemoError> for Rollback`.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let v = helper(true)?;
        Ok(Accept::new(b"ok", v as i64))
    }

    #[cbak(0)]
    fn main_cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    /// A second entry in the same chain, without a `#[cbak]`.
    #[hook(1, on = [Invoke])]
    fn second(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

fn main() {
    let entries = <Vault as HookChainEntries>::ENTRIES;
    assert_eq!(entries.len(), 2);

    let e0 = entries.iter().find(|e| e.index == 0).expect("index 0");
    assert!(e0.cbak.is_some());

    let e1 = entries.iter().find(|e| e.index == 1).expect("index 1");
    assert!(e1.cbak.is_none());
}
