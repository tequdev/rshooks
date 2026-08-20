#![no_std]

use rshooks::exit::{Accept, HookResult};
use rshooks::*;

hook_errors! {
    /// Errors returned by the typed `deposit` entry.
    pub enum DepositError {
        /// The `AMT` parameter was missing, or not exactly 8 bytes.
        BadAmount = 1 => b"typed-results: bad AMT parameter",
        /// The updated counter could not be persisted.
        StateSetFailed = 2 => b"typed-results: state_set failed",
    }
}

#[hooks(description = "Typed deposit (HookResult) plus a legacy-style reset, one chain.")]
pub struct TypedResults {
    /// Persistent running total, shared by both entries below.
    #[state(key = b"counter")]
    counter: State<u64>,
    /// Deposit amount, read from the originating transaction. Only the
    /// typed `deposit` entry reads this field.
    #[otxn_param(name = b"AMT", required)]
    amount: OtxnParam<[u8; 8]>,
}

// Two `?`-called helpers, each `#[inline(always)]` (the D4 convention from
// `.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` §5 — probe p2fix measured
// that *without* forcing the inline, the extra call boundary a plain
// `Result`-returning helper introduces costs a small but real WCE delta at
// this call density; force-inlined, the typed form measured *below* the
// hand-written `accept!`/`rollback!` baseline). Both convert their failure
// with `.map_err(..)`, never `?` on the raw `HookError` a Hook API call
// returns directly — see [`rshooks::exit::Rollback`]'s doc comment (D3):
// `HookError::code()` is a 46-arm re-encode match that measurably does not
// optimize away through a two-hop `?`.
#[inline(always)]
fn read_amount(t: &TypedResults) -> Result<u64, DepositError> {
    let bytes = t
        .amount
        .get_required()
        .map_err(|_| DepositError::BadAmount)?;
    Ok(u64::from_be_bytes(bytes))
}

#[inline(always)]
fn bump_counter(t: &TypedResults, amount: u64) -> Result<u64, DepositError> {
    let count = t.counter.get().unwrap_or(Some(0)).unwrap_or(0);
    let next = count.wrapping_add(amount);
    t.counter
        .set(&next)
        .map_err(|_| DepositError::StateSetFailed)?;
    Ok(next)
}

#[hooks]
impl TypedResults {
    /// Typed entry: reads `AMT`, adds it to the persistent counter, and
    /// accepts with the new total. Both fallible steps `?`-propagate
    /// through `DepositError` — the compiled shape this example exists to
    /// demonstrate, in place of hand-written `accept!`/`rollback!` calls at
    /// every failure point.
    #[hook(0, name = "deposit", on = [Invoke])]
    fn deposit(&self) -> HookResult {
        let amount = read_amount(self)?;
        let next = bump_counter(self, amount)?;
        Ok(Accept::new(b"typed-results: deposited", next as i64))
    }

    /// Legacy entry, in the same chain as the typed one above — proves the
    /// two forms coexist in one `#[hooks]` struct/impl pair. Resets the
    /// counter to zero via `accept!`/`rollback!` directly, exactly like
    /// `examples/02_state-counter`.
    #[hook(1, name = "reset", on = [Invoke])]
    fn reset(&self) -> i64 {
        if self.counter.set(&0u64).is_err() {
            rollback!(b"typed-results: reset failed", DepositError::StateSetFailed);
        }
        accept!(b"typed-results: reset", 0)
    }
}
