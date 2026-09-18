//! `increment(account: AccountID, count: UInt16)` — the Hook Parameter
//! Signature Interface draft's own worked example: adds `count` to a
//! per-`account` counter in hook state. `account`/`count` are declared
//! directly on the entry fn's own signature (extra arguments after
//! `&self`) rather than a hand-rolled `#[otxn_param(..)]` struct — see
//! `examples/12_typed-data` for that surface.

#![no_std]

use rshooks::prelude::*;
use rshooks::*;

hook_errors! {
    /// `param-signature` rollback codes.
    ///
    /// Numbered from 16, not 1: this hook declares two signature parameters
    /// (`account`/`count`), and the `#[hooks]`-generated prologue rolls
    /// back with the argument's own 0-based index as its code (here, `0`
    /// or `1`). A hook-authored code has to stay clear of every possible
    /// argument index (`0x00..=0x0F`, i.e. `0..=15`) or the two rollback
    /// sources become ambiguous by code alone.
    pub enum ParamSignatureError {
        /// The updated counter could not be persisted.
        StateSetFailed = 16,
    }
}

/// Per-account counter key: the invoking account's own 20 bytes. This
/// chain has only one state field, so no discriminant tag is needed. A
/// single-field `HookKey` struct is still preferred over
/// `#[state(key_by = [u8; 20])]` directly, since `AccountId` itself has no
/// `StateKeyEncode` impl of its own.
#[derive(HookKey, Clone, Copy)]
struct CounterKey {
    account: AccountId,
}

#[hooks]
pub struct Increment {
    /// Per-account invocation counter, keyed by [`CounterKey`].
    #[state(key_by = CounterKey)]
    counters: State<u64>,
}

#[hooks]
impl Increment {
    /// `account`(0): `AccountId` (`STI_ACCOUNT`, `0x08`). `count`(1): `u16`
    /// (`STI_UINT16`, `0x01`). Both are declared signature parameters and
    /// are already decoded by the time this body runs: the
    /// `#[hooks]`-generated prologue reads and big-endian-decodes each
    /// from the originating transaction's Hook parameters, and rolls back
    /// with `b"rshooks: bad sig param '<name>'"` (code = the argument's
    /// own index) if either is missing or the wrong length — this body
    /// never sees a partially-decoded invocation.
    #[hook(0, on = [Invoke])]
    fn increment(&self, account: AccountId, count: u16) -> HookResult {
        let counter = self.state.counters.at(CounterKey { account });
        let current = counter.get().unwrap_or(Some(0)).unwrap_or(0);
        let next = current.wrapping_add(u64::from(count));

        if counter.set(&next).is_err() {
            rollback!(
                b"param-signature: state_set failed",
                ParamSignatureError::StateSetFailed
            );
        }

        accept!(b"param-signature: incremented", next as i64)
    }
}
