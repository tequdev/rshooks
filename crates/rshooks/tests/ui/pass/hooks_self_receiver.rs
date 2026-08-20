//! A chain mixing the accepted entry/cbak/helper receiver forms
//! (HOOKS_SELF_RECEIVER_DESIGN.md §3): a `&self` hook entry that reads a
//! declared field through `self.<field>` and calls a `&self` helper via
//! `self.helper()`, a second plain `&self` entry, a `&self` cbak, and a
//! `&self` entry/helper pair each carrying a leading attribute on the
//! receiver itself — a legal receiver attribute must not make the receiver
//! look absent (R1 of the self-receiver review).

use rshooks::exit::{Accept, HookResult};
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
    fn main(&self) -> HookResult {
        let _ = self.counter.get();
        Ok(Accept::from_code(self.helper()))
    }

    /// `&self` cbak, also using `self.`.
    #[cbak(0)]
    fn main_cbak(&self) -> HookResult {
        Ok(Accept::from_code(i64::from(self.counter.get().is_err())))
    }

    /// A second plain `&self` entry.
    #[hook(1, on = [Invoke])]
    fn second(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    /// `&self` entry with a leading attribute on the receiver itself: must
    /// classify exactly like bare `&self`, not as an extra/no-receiver
    /// argument.
    #[hook(2, on = [Invoke])]
    fn attributed(#[allow(unused)] &self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    /// `&self` helper (non-attributed), called via `self.helper()`.
    fn helper(&self) -> i64 {
        i64::from(self.counter.get().is_err())
    }

    /// `&self` helper with a leading attribute on the receiver itself.
    fn attributed_helper(#[allow(unused)] &self) -> i64 {
        0
    }
}

fn main() {
    assert!(Vault.counter.get().is_err());
    assert_eq!(Vault.attributed(), Ok(Accept::from_code(0)));
    assert_eq!(Vault.attributed_helper(), 0);
}
