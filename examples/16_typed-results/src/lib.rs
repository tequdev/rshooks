#![no_std]

use rshooks::exit::{Accept, HookResult};
use rshooks::*;

hook_errors! {
    /// Errors returned by the typed `deposit` entry.
    pub enum DepositError {
        /// The updated counter could not be persisted.
        StateSetFailed = 2 => b"typed-results: state_set failed",
    }
}

#[hooks(
    description = "Two typed (HookResult) entries, one chain: idiomatic `?` vs. raw accept!/rollback!."
)]
pub struct TypedResults {
    /// Persistent running total, shared by both entries below.
    #[state(key = b"counter")]
    counter: State<u64>,
}

// `#[inline(always)]` (the D4 convention from
// `.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` §5 — probe p2fix measured
// that *without* forcing the inline, the extra call boundary a plain
// `Result`-returning helper introduces costs a small but real WCE delta at
// this call density; force-inlined, the typed form measured *below* the
// hand-written `accept!`/`rollback!` baseline). Converts its failure with
// `.map_err(..)`, never `?` on the raw `HookError` a Hook API call returns
// directly — see [`rshooks::exit::Rollback`]'s doc comment (D3):
// `HookError::code()` is a 46-arm re-encode match that measurably does not
// optimize away through a two-hop `?`.
#[inline(always)]
fn bump_counter(t: &TypedResults, amount: u64) -> Result<u64, DepositError> {
    let count = t.state.counter.get().unwrap_or(Some(0)).unwrap_or(0);
    let next = count.wrapping_add(amount);
    t.state
        .counter
        .set(&next)
        .map_err(|_| DepositError::StateSetFailed)?;
    Ok(next)
}

#[hooks]
impl TypedResults {
    /// Typed entry: `amount` is a declared signature parameter
    /// (`docs/PARAM_SIGNATURE_DESIGN.md` §1) — already decoded by the time
    /// this body runs (a missing or wrong-length `amount` rolls back from
    /// the generated prologue, before `bump_counter` is ever called; see
    /// this crate's README for the exact message). `bump_counter`'s
    /// `?`-propagates its own `StateSetFailed` through `DepositError` — the
    /// compiled shape this example exists to demonstrate, in place of a
    /// hand-written `accept!`/`rollback!` call at that failure point.
    #[hook(0, name = "deposit", on = [Invoke])]
    fn deposit(&self, amount: u64) -> HookResult {
        let next = bump_counter(self, amount)?;
        Ok(Accept::new(b"typed-results: deposited", next as i64))
    }

    /// Raw-style typed entry, in the same chain as `deposit` above —
    /// demonstrates that the `accept!`/`rollback!` escape hatch stays
    /// first-class inside a `HookResult`-returning entry. Resets the
    /// counter to zero with a raw accept!/rollback! body, and is
    /// declared `-> HookResult` like every other entry.
    #[hook(1, name = "reset", on = [Invoke])]
    fn reset(&self) -> HookResult {
        if self.state.counter.set(&0u64).is_err() {
            rollback!(b"typed-results: reset failed", DepositError::StateSetFailed);
        }
        accept!(b"typed-results: reset", 0)
    }
}
