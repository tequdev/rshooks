//! A chain mixing the accepted entry/cbak/helper receiver forms
//! (HOOKS_SELF_RECEIVER_DESIGN.md §3): a `&self` hook entry that reads a
//! declared field through `self.<field>` and calls a `&self` helper via
//! `self.helper()`, a no-receiver hook entry (the pre-existing form, still
//! legal alongside `&self`), and a `&self` cbak.

use rshooks::hooks;

#[hooks]
struct Vault {
    #[state(key = b"CT")]
    counter: State<u64>,
}

#[hooks]
impl Vault {
    /// `&self` entry: reads a declared field via `self.` and delegates to a
    /// `&self` helper.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> i64 {
        let _ = self.counter.get();
        self.helper()
    }

    /// `&self` cbak, also using `self.`.
    #[cbak(0)]
    fn main_cbak(&self) -> i64 {
        i64::from(self.counter.get().is_err())
    }

    /// No-receiver entry: the pre-existing form remains legal alongside
    /// `&self` entries in the same chain.
    #[hook(1, on = [Invoke])]
    fn second() -> i64 {
        0
    }

    /// `&self` helper (non-attributed), called via `self.helper()`.
    fn helper(&self) -> i64 {
        i64::from(self.counter.get().is_err())
    }
}

fn main() {
    assert!(Vault.counter.get().is_err());
}
