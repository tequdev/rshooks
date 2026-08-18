//! A per-account deposit ledger using typed state and Hook parameters.
//!
//! `deposit` creates or extends a locked balance; `withdraw` removes it once
//! its lock window expires. The `CFG` parameter configures the minimum amount
//! and lock window, while `ADMIN_PAUSE` can disable new deposits.

#![no_std]

use rshooks::prelude::*;
use rshooks::*;

/// Discriminant for deposit records.
const DEPOSIT_TAG: u8 = 1;

/// `Instruction::action` value for a deposit.
const ACTION_DEPOSIT: u8 = 1;
/// `Instruction::action` value for a withdrawal.
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

/// Per-invocation instruction, read from the `INS` originating-transaction
/// parameter.
#[derive(ParamValue)]
struct Instruction {
    action: u8,
    amount: u64,
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
    pub enum TypedDataError {
        /// The originating transaction has no `sfAccount` field (should be
        /// unreachable — every real transaction has one).
        AccountFieldMissing = 1,
        /// The `INS` Hook parameter is missing, or not exactly 9 bytes
        /// (`Instruction`'s encoded length).
        InstructionMissing = 2,
        /// `Instruction::action` is neither [`ACTION_DEPOSIT`] nor
        /// [`ACTION_WITHDRAW`].
        UnknownAction = 3,
        /// A `deposit` instruction's amount fell below [`Config::min_amount`].
        BelowMinimum = 4,
        /// A `withdraw` instruction, but the account has no outstanding
        /// deposit.
        NothingToWithdraw = 5,
        /// A `withdraw` instruction, but the deposit's lock window hasn't
        /// elapsed yet.
        StillLocked = 6,
        /// Reading this account's `DepositValue` failed with something
        /// other than "no entry" (`state`'s `DOESNT_EXIST`).
        StateReadFailed = 7,
        /// Writing the updated `DepositValue` back — or, on a full
        /// withdrawal, deleting it — failed.
        StateSetFailed = 8,
        /// A `deposit` instruction, but the [`AdminName`] pause switch is
        /// currently set. Withdrawals are never rejected for this reason.
        DepositsPaused = 9,
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

    /// Per-invocation instruction (`INS`). Missing or malformed is a
    /// rollback, never a silent default.
    #[otxn_param(name = b"INS", required)]
    instruction: OtxnParam<Instruction>,

    /// Administrative deposit pause switch, addressed by [`AdminName`].
    /// Falls back to "not paused" when absent.
    #[hook_param(name_by = AdminName, default = PauseSwitch { paused: 0 })]
    admin_pause: HookParam<PauseSwitch>,
}

/// Returns the configured `CFG` values, falling back to the default when
/// `CFG` is absent *or* present-but-malformed: `.unwrap_or(..)` masks any
/// `Err` from [`HookParam::get_or_default`], not just the "absent" case.
fn config() -> Config {
    TypedData.config.get_or_default().unwrap_or(Config {
        min_amount: DEFAULT_MIN_AMOUNT,
        lock_ledgers: DEFAULT_LOCK_LEDGERS,
    })
}

/// Returns whether new deposits are paused. Masks any read failure to
/// "not paused", exactly like [`config`] masks `CFG` read failures to its
/// default.
fn deposits_paused() -> bool {
    TypedData
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
    /// Hook entry point. See the module doc comment for the full behavior.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> i64 {
        let Ok(owner) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"typed-data: sfAccount missing from the originating transaction",
                TypedDataError::AccountFieldMissing
            )
        };

        let Ok(instruction) = self.instruction.get_required() else {
            rollback!(
                b"typed-data: INS parameter missing or malformed",
                TypedDataError::InstructionMissing
            )
        };

        let deposit = self.deposits.at(DepositKey {
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

        let next = match instruction.action {
            ACTION_DEPOSIT => {
                if deposits_paused() {
                    rollback!(
                        b"typed-data: deposits are currently paused",
                        TypedDataError::DepositsPaused
                    );
                }
                if instruction.amount < cfg.min_amount {
                    rollback!(
                        b"typed-data: deposit below configured minimum",
                        TypedDataError::BelowMinimum
                    );
                }
                DepositValue {
                    amount: current.amount.wrapping_add(instruction.amount),
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
            _ => rollback!(
                b"typed-data: unknown INS action",
                TypedDataError::UnknownAction
            ),
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
