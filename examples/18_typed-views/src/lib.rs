#![no_std]

use rshooks::prelude::*;
use rshooks::views::{ledger, tx};
use rshooks::*;

// The billionths-encoded `sfTransferRate` value for 1.0.
const NO_TRANSFER_FEE: u32 = 1_000_000_000;

hook_errors! {
    /// Errors returned by the incoming-payment gate.
    pub enum ViewError {
        NotAPayment = 1,
        MissingAmount = 2,
        MissingDestinationTag = 3,
        NoHookAccount = 4,
        KeyletFailed = 5,
        NoTrustLine = 6,
        NoLineFlags = 7,
        FrozenByUs = 8,
        FrozenByCounterparty = 9,
        NoIssuerAccount = 10,
        IssuerChargesFee = 11,
    }
}

/// Reads the canonical low account from `sfLowLimit` rather than re-sorting
/// the accounts. This measured cheaper in WCE; see the README.
#[inline(never)]
fn hook_is_low_side(line: &ledger::RippleState, me: &AccountId) -> bool {
    match line.low_limit() {
        Ok(AmountBytes::Iou(low)) => buf_eq_20(&low.issuer().0, &me.0),
        // This only affects which freeze error is reported on a malformed line.
        _ => false,
    }
}

/// Treats an absent, zero, or unit `sfTransferRate` as no fee.
#[inline(never)]
fn issuer_charges_fee(issuer: &ledger::AccountRoot) -> bool {
    match issuer.transfer_rate() {
        Ok(None) | Ok(Some(0)) | Ok(Some(NO_TRANSFER_FEE)) => false,
        Ok(Some(_)) => true,
        // A read failure is not equivalent to an absent optional field.
        Err(_) => true,
    }
}

#[hooks(
    description = "Gates incoming IOU payments on trust-line and issuer state, read through generated views."
)]
pub struct TypedViews;

#[hooks]
impl TypedViews {
    /// Gates incoming IOU payments on a destination tag, an unfrozen trust
    /// line, and a fee-free issuer. Cheaper checks run first.
    #[hook(0, name = "gate", on = [Payment])]
    fn main(&self) -> HookResult {
        let Ok(payment) = tx::Payment::otxn() else {
            rollback!(b"typed-views: not a Payment", ViewError::NotAPayment)
        };

        let Ok(amount) = payment.amount() else {
            rollback!(b"typed-views: no Amount", ViewError::MissingAmount)
        };
        let AmountBytes::Iou(iou) = amount else {
            accept!(b"typed-views: native payment, not gated", 0)
        };

        match payment.destination_tag() {
            Ok(Some(_)) => {}
            _ => rollback!(
                b"typed-views: IOU payment without a DestinationTag",
                ViewError::MissingDestinationTag
            ),
        }

        let Ok(me) = hook_account_buf() else {
            rollback!(b"typed-views: no hook account", ViewError::NoHookAccount)
        };
        let asset = iou.asset();
        let Ok(keylet) = keylet_line_for_asset(&me, &asset) else {
            rollback!(b"typed-views: keylet_line failed", ViewError::KeyletFailed)
        };
        let Ok(line) = ledger::RippleState::from_keylet(&keylet) else {
            rollback!(
                b"typed-views: no trust line to the issuer",
                ViewError::NoTrustLine
            )
        };

        let Ok(flags) = line.flags() else {
            rollback!(b"typed-views: no line Flags", ViewError::NoLineFlags)
        };
        if flags & (lsfLowFreeze | lsfHighFreeze) != 0 {
            let ours = if hook_is_low_side(&line, &me) {
                lsfLowFreeze
            } else {
                lsfHighFreeze
            };
            if flags & ours != 0 {
                rollback!(
                    b"typed-views: this account froze the line",
                    ViewError::FrozenByUs
                )
            }
            rollback!(
                b"typed-views: the counterparty froze the line",
                ViewError::FrozenByCounterparty
            )
        }

        let Ok(issuer_keylet) = keylet_account(&asset.issuer) else {
            rollback!(
                b"typed-views: keylet_account failed",
                ViewError::KeyletFailed
            )
        };
        let Ok(issuer) = ledger::AccountRoot::from_keylet(&issuer_keylet) else {
            rollback!(
                b"typed-views: issuer is unfunded",
                ViewError::NoIssuerAccount
            )
        };
        if issuer_charges_fee(&issuer) {
            rollback!(
                b"typed-views: issuer charges a transfer fee",
                ViewError::IssuerChargesFee
            )
        }

        accept!(b"typed-views: incoming IOU accepted", 0)
    }
}
