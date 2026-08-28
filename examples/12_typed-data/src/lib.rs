//! A per-account deposit ledger using typed state, Hook parameters, and
//! signature parameters.
//!
//! `deposit` creates or extends a locked balance; `withdraw` removes it once
//! its lock window expires. The `CFG` parameter configures the minimum amount
//! and lock window, while `ADMIN_PAUSE` can disable new deposits. Which
//! action to run, and how much, arrive as declared *signature parameters*
//! (`docs/PARAM_SIGNATURE_DESIGN.md`) — `main`'s own `action`/`amount`
//! arguments — rather than a hand-packed `#[otxn_param(..)]` struct: this
//! example pairs the two parameter surfaces this crate offers side by side,
//! `CFG`/`ADMIN_PAUSE` (struct-field `HookParam`) and `action`/`amount`
//! (entry-fn signature parameters), so their differences are visible in one
//! hook.

#![no_std]

use rshooks::prelude::*;
use rshooks::*;

/// Discriminant for deposit records.
const DEPOSIT_TAG: u8 = 1;

/// `action` signature parameter value for a deposit.
const ACTION_DEPOSIT: u8 = 1;
/// `action` signature parameter value for a withdrawal.
const ACTION_WITHDRAW: u8 = 2;

/// Default minimum deposit in drops.
const DEFAULT_MIN_AMOUNT: u64 = 1_000_000;
/// Lock window (in ledgers) used when `CFG` isn't configured.
const DEFAULT_LOCK_LEDGERS: u32 = 10;

/// Per-account deposit record key: a fixed discriminant tag plus the
/// account it belongs to.
#[derive(HookKey, Clone, Copy)]
struct DepositKey {
    tag: u8,
    owner: AccountId,
}

/// Per-account deposit record value.
#[derive(HookData, Clone, Copy)]
struct DepositValue {
    amount: u64,
    deadline: u32,
    flags: u8,
}

/// Install-time configuration, read from the `CFG` Hook parameter.
#[derive(ParamValue)]
struct Config {
    min_amount: u64,
    lock_ledgers: u32,
}

/// Composite name for administrative parameters.
#[derive(ParamName, Clone, Copy)]
struct AdminName {
    section: u8,
    field: u8,
}

/// Hook-wide deposit pause switch, read from the [`AdminName`]-addressed
/// administrative parameter.
#[derive(ParamValue)]
struct PauseSwitch {
    paused: u8,
}

/// This chain's fixed [`AdminName`] instance — the pause switch lives at a
/// single, hard-coded administrative address (`section = 0, field = 0`),
/// never per-call-site.
const ADMIN_PAUSE_NAME: AdminName = AdminName {
    section: 0,
    field: 0,
};

hook_errors! {
    /// `typed-data` rollback codes.
    ///
    /// Numbered from 16, not 1: this hook declares signature parameters
    /// (`main`'s own `action`/`amount` arguments — `docs/PARAM_SIGNATURE_DESIGN.md`
    /// §1), and the `#[hooks]`-generated prologue rolls back with the
    /// argument's own 0-based index as its code (here, `0` or `1`). A
    /// hook-authored code has to stay clear of every possible argument
    /// index (`0x00..=0x0F`, i.e. `0..=15`) or the two rollback sources
    /// become ambiguous by code alone — so every variant here starts at
    /// `16`, the convention `book/src/data/parameters.md` documents for any
    /// hook that declares signature parameters.
    pub enum TypedDataError {
        /// The originating transaction has no `sfAccount` field (should be
        /// unreachable — every real transaction has one).
        AccountFieldMissing = 16,
        /// `action` is neither [`ACTION_DEPOSIT`] nor [`ACTION_WITHDRAW`].
        /// (A missing/malformed `action`/`amount` signature parameter is
        /// caught earlier — by the `#[hooks]`-generated prologue, before
        /// `main`'s body ever runs — and never reaches this variant.)
        UnknownAction = 17,
        /// A `deposit` instruction's amount fell below [`Config::min_amount`].
        BelowMinimum = 18,
        /// A `withdraw` instruction, but the account has no outstanding
        /// deposit.
        NothingToWithdraw = 19,
        /// A `withdraw` instruction, but the deposit's lock window hasn't
        /// elapsed yet.
        StillLocked = 20,
        /// Reading this account's `DepositValue` failed with something
        /// other than "no entry" (`state`'s `DOESNT_EXIST`).
        StateReadFailed = 21,
        /// Writing the updated `DepositValue` back — or, on a full
        /// withdrawal, deleting it — failed.
        StateSetFailed = 22,
        /// A `deposit` instruction, but the [`AdminName`] pause switch is
        /// currently set. Withdrawals are never rejected for this reason.
        DepositsPaused = 23,
    }
}

/// This chain's shared state/parameter schema — see the module doc comment
/// for the overall behavior each field participates in.
#[hooks]
pub struct TypedData {
    /// Per-account deposit record, keyed by [`DepositKey`].
    #[state(key_by = DepositKey)]
    deposits: State<DepositValue>,

    /// Install-time configuration (`CFG`). Falls back to
    /// [`DEFAULT_MIN_AMOUNT`]/[`DEFAULT_LOCK_LEDGERS`] when absent.
    #[hook_param(name = b"CFG", default = Config { min_amount: DEFAULT_MIN_AMOUNT, lock_ledgers: DEFAULT_LOCK_LEDGERS })]
    config: HookParam<Config>,

    /// Administrative deposit pause switch, addressed by [`AdminName`].
    /// Falls back to "not paused" when absent.
    #[hook_param(name_by = AdminName, default = PauseSwitch { paused: 0 })]
    admin_pause: HookParam<PauseSwitch>,
}

/// Returns the configured `CFG` values, falling back to the default when
/// `CFG` is absent *or* present-but-malformed: `.unwrap_or(..)` masks any
/// `Err` from [`HookParam::get_or_default`], not just the "absent" case.
fn config() -> Config {
    TypedData
        .hook_param
        .config
        .get_or_default()
        .unwrap_or(Config {
            min_amount: DEFAULT_MIN_AMOUNT,
            lock_ledgers: DEFAULT_LOCK_LEDGERS,
        })
}

/// Returns whether new deposits are paused. Masks any read failure to
/// "not paused", exactly like [`config`] masks `CFG` read failures to its
/// default.
fn deposits_paused() -> bool {
    TypedData
        .hook_param
        .admin_pause
        .at(ADMIN_PAUSE_NAME)
        .get_or_default()
        .map(|s| s.paused != 0)
        .unwrap_or(false)
}

/// Deposit value used when no record exists.
const EMPTY_DEPOSIT: DepositValue = DepositValue {
    amount: 0,
    deadline: 0,
    flags: 0,
};

#[hooks]
impl TypedData {
    /// Hook entry point. `action`(0)/`amount`(1) are declared signature
    /// parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §1) — extra arguments
    /// after `&self` on this `#[hook(..)]` fn. Both are already decoded by
    /// the time this body runs: the `#[hooks]`-generated prologue reads and
    /// big-endian-decodes each from the originating transaction's own Hook
    /// parameters (`otxn_param`, per the interface's declared
    /// `HookParameterName` convention — never this crate's own
    /// little-endian `ParamValue` wire format, contrast `CFG` below), and
    /// rolls back with `b"rshooks: bad sig param '<name>'"` (code = the
    /// argument's index) before the body ever sees a partially-decoded
    /// value. See the module doc comment for the full behavior.
    #[hook(0, on = [Invoke])]
    fn main(&self, action: u8, amount: u64) -> HookResult {
        let Ok(owner) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"typed-data: sfAccount missing from the originating transaction",
                TypedDataError::AccountFieldMissing
            )
        };

        let deposit = self.state.deposits.at(DepositKey {
            tag: DEPOSIT_TAG,
            owner,
        });

        let current = match deposit.get() {
            Ok(existing) => existing.unwrap_or(EMPTY_DEPOSIT),
            Err(_) => rollback!(
                b"typed-data: state read failed",
                TypedDataError::StateReadFailed
            ),
        };

        let cfg = config();

        let next = match action {
            ACTION_DEPOSIT => {
                if deposits_paused() {
                    rollback!(
                        b"typed-data: deposits are currently paused",
                        TypedDataError::DepositsPaused
                    );
                }
                if amount < cfg.min_amount {
                    rollback!(
                        b"typed-data: deposit below configured minimum",
                        TypedDataError::BelowMinimum
                    );
                }
                DepositValue {
                    amount: current.amount.wrapping_add(amount),
                    deadline: ledger_seq().wrapping_add(cfg.lock_ledgers),
                    flags: 1,
                }
            }
            ACTION_WITHDRAW => {
                if current.flags == 0 {
                    rollback!(
                        b"typed-data: nothing to withdraw",
                        TypedDataError::NothingToWithdraw
                    );
                }
                if ledger_seq() < current.deadline {
                    rollback!(
                        b"typed-data: deposit still locked",
                        TypedDataError::StillLocked
                    );
                }
                // Delete the state entry to release its owner reserve.
                if deposit.delete().is_err() {
                    rollback!(
                        b"typed-data: state_set failed",
                        TypedDataError::StateSetFailed
                    );
                }
                accept!(b"typed-data: ok", 0)
            }
            _ => rollback!(b"typed-data: unknown action", TypedDataError::UnknownAction),
        };

        if deposit.set(&next).is_err() {
            rollback!(
                b"typed-data: state_set failed",
                TypedDataError::StateSetFailed
            );
        }

        accept!(b"typed-data: ok", next.amount as i64)
    }
}
