//! `balances(id=0): key(account: AccountId, token: u32), value(amount: u64,
//! updated: u32)` and `config(id=1): value(paused: u8)` — the Hook State
//! Interface draft's own worked example (`docs/STATE_INTERFACE_DESIGN.md`
//! §2/§7): a keyed per-`(account, token)` balance and a singleton
//! configuration flag, both declared directly on the chain struct rather
//! than a hand-rolled `#[state(..)]` key. `token` is fixed to `0` here (one
//! balance per sender) — a real multi-asset hook would read it from a
//! parameter, the same way `examples/19_param-signature` reads `count`.

#![no_std]

use rshooks::prelude::*;
use rshooks::*;

hook_errors! {
    /// `state-interface` rollback codes.
    pub enum StateInterfaceError {
        /// The originating transaction has no `sfAccount` field (should be
        /// unreachable for any real transaction type).
        MissingAccount = 1,
        /// The updated balance could not be persisted.
        StateSetFailed = 2,
    }
}

/// The single balance token this example tracks.
const TOKEN: u32 = 0;

#[hooks]
pub struct Treasury {
    /// Per-`(account, token)` balance — a keyed declaration.
    #[state_interface(
        id = 0,
        key(account: AccountId, token: u32),
        value(amount: u64, updated: u32)
    )]
    balances: State<Balance>,

    /// Hook-wide configuration — a singleton declaration (no key fields).
    #[state_interface(id = 1, value(paused: u8))]
    config: State<Config>,
}

#[hooks]
impl Treasury {
    /// Credits the sender's balance by 1 and bumps `updated`, then makes
    /// sure the singleton `config` entry exists (never pausing on its own —
    /// an administrator would flip it out of band).
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let Ok(account) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"state-interface: sfAccount missing",
                StateInterfaceError::MissingAccount
            );
        };

        let entry = self.state.balances.at((account, TOKEN));
        let current = entry.get().unwrap_or(None).unwrap_or(Balance {
            amount: 0,
            updated: 0,
        });
        let next = Balance {
            amount: current.amount.wrapping_add(1),
            updated: current.updated.wrapping_add(1),
        };
        if entry.set(&next).is_err() {
            rollback!(
                b"state-interface: state_set failed",
                StateInterfaceError::StateSetFailed
            );
        }

        if self.state.config.get().unwrap_or(None).is_none()
            && self.state.config.set(&Config { paused: 0 }).is_err()
        {
            rollback!(
                b"state-interface: state_set failed",
                StateInterfaceError::StateSetFailed
            );
        }

        accept!(b"state-interface: credited", next.amount as i64)
    }
}
