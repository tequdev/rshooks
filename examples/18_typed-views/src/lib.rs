#![no_std]

use rshooks::prelude::*;
use rshooks::views::{ledger, tx};
use rshooks::*;

/// The `sfTransferRate` value that means "no transfer fee": rippled encodes
/// the rate as a billionth, so 1.0 is `1_000_000_000`. The field is
/// `soeOPTIONAL`, and both its absence and an explicit `0` also mean no fee
/// — see [`issuer_charges_fee`].
const NO_TRANSFER_FEE: u32 = 1_000_000_000;

hook_errors! {
    /// Errors returned by the incoming-payment gate.
    pub enum ViewError {
        /// The originating transaction is not a `Payment` — the view's own
        /// constructor check, not a hand-written `otxn_type` comparison.
        NotAPayment = 1,
        /// `sfAmount` could not be read. Required by the format, so this is
        /// a malformed transaction rather than an ordinary absence.
        MissingAmount = 2,
        /// The payment's `sfDestinationTag` is missing or unreadable —
        /// this arm takes both `Ok(None)` and a read error, since neither
        /// lets the policy be satisfied.
        MissingDestinationTag = 3,
        /// The hook account's own address could not be read.
        NoHookAccount = 4,
        /// The trust line's keylet could not be built.
        KeyletFailed = 5,
        /// No usable trust line between this account and the issuer for the
        /// currency being paid: the object is absent, is not a
        /// `ltRIPPLE_STATE` (the view's constructor check), or its type
        /// field could not be read.
        NoTrustLine = 6,
        /// The line's `sfFlags` could not be read.
        NoLineFlags = 7,
        /// This account has frozen the line; it does not want these tokens.
        FrozenByUs = 8,
        /// The counterparty has frozen the line; the balance is unusable.
        FrozenByCounterparty = 9,
        /// No usable `AccountRoot` for the issuer: unfunded, not an
        /// `ltACCOUNT_ROOT`, or its type field could not be read.
        NoIssuerAccount = 10,
        /// The issuer charges a transfer fee.
        IssuerChargesFee = 11,
    }
}

/// Which side of a `RippleState` this hook's account is on.
///
/// A trust line has no "sender" and "receiver" side. The protocol sorts the
/// two accounts canonically — `keylet_line`'s own doc comment says the
/// argument order does not matter for exactly this reason — and calls the
/// smaller one *low* and the larger one *high*. `lsfLowFreeze` means the
/// low account froze the line; `lsfHighFreeze` means the high one did. So
/// "did *we* freeze this, or did they" is not answerable from the flags
/// alone; it needs to know which side we are.
///
/// The answer is read off the line rather than re-derived: rippled stores
/// `sfLowLimit` as an IOU amount **issued by the low account**, so its
/// issuer field *is* the low account (`sfHighLimit` likewise carries the
/// high one). Comparing that against the hook account is [`buf_eq_20`], a
/// fixed-size 20-byte equality test.
///
/// # Why not just compare the accounts
///
/// The obvious alternative re-derives the canonicalization instead: both
/// accounts are already in hand, so `me < asset.issuer` answers the same
/// question with **three fewer host calls**. It is even loop-free —
/// [`AccountId`]'s `Ord` goes through [`buf_cmp_20`] rather than a raw
/// `[u8; 20]` compare, so it does not reintroduce the `memcmp` loop the
/// guard checker rejects (`examples/06_guard-patterns` documents that
/// pitfall).
///
/// It is nonetheless the more expensive option here, measurably:
///
/// | side determination | WCE | size | nesting |
/// |---|---:|---:|---:|
/// | this one (`low_limit()` + `buf_eq_20`) | 845 | 2559 | 3 |
/// | `me < asset.issuer` (`buf_cmp_20`) | 980 | 2815 | 4 |
///
/// Fewer host calls, more instructions — and instructions are what Xahau
/// budgets. A host call is a single `call` in the worst-case-instruction
/// count, while [`buf_cmp_20`] inlines a three-stage early-exit ladder
/// (8 bytes, 8 bytes, 4 bytes) whose branches cost far more than the three
/// `call`s they replace, and add a nesting level besides. [`buf_eq_20`] has
/// no such ladder: equality can OR the word differences together and test
/// once, so it stays branch-free.
///
/// The lesson generalizes past this hook: "fewer host calls" and "fewer
/// instructions" are different objectives, and on Xahau only the second one
/// is metered. Measure rather than assume.
///
/// `#[inline(never)]`: this is only reached on a rejection path, and keeping
/// it out of line keeps its `match` out of `hook()`'s own nesting budget.
#[inline(never)]
fn hook_is_low_side(line: &ledger::RippleState, me: &AccountId) -> bool {
    match line.low_limit() {
        Ok(AmountBytes::Iou(low)) => buf_eq_20(&low.issuer().0, &me.0),
        // A line whose `sfLowLimit` is unreadable or not an IOU amount is
        // malformed. Reporting "not the low side" only phrases a rollback
        // that is already happening.
        _ => false,
    }
}

/// Whether `issuer` charges a transfer fee.
///
/// `sfTransferRate` is `soeOPTIONAL`, so the view hands back
/// `Result<Option<u32>>` and `Ok(None)` means the field is simply not
/// there. **The default is the caller's to supply, not the view's**: the
/// format macro upstream records only that the field may be omitted, never
/// what an omitted value stands for, so the generated accessor reports
/// absence rather than inventing 1.0. Here absence and an explicit `0` both
/// mean "no fee", which is rippled's own reading.
///
/// `#[inline(never)]` for the nesting reason above.
#[inline(never)]
fn issuer_charges_fee(issuer: &ledger::AccountRoot) -> bool {
    match issuer.transfer_rate() {
        Ok(None) | Ok(Some(0)) | Ok(Some(NO_TRANSFER_FEE)) => false,
        Ok(Some(_)) => true,
        // An unreadable optional field is not an absent one; refuse rather
        // than guess.
        Err(_) => true,
    }
}

#[hooks(
    description = "Gates incoming IOU payments on trust-line and issuer state, read through generated views."
)]
pub struct TypedViews;

#[hooks]
impl TypedViews {
    /// Accepts an incoming payment only when every fact the views can check
    /// holds:
    ///
    /// 1. It really is a `Payment` — [`tx::Payment::otxn`] verifies that
    ///    before any field is read.
    /// 2. A native (XAH) payment is out of scope and accepted immediately.
    /// 3. An IOU payment carries a `sfDestinationTag`.
    /// 4. This account has a trust line to the issuer for that currency,
    ///    and neither side has frozen it.
    /// 5. The issuer charges no transfer fee.
    ///
    /// The order is deliberate: each step is cheaper than the one after it,
    /// so the common rejections cost the fewest host calls. Nothing is read
    /// that is not used — see the README's cost table.
    #[hook(0, name = "gate", on = [Payment])]
    fn main(&self) -> HookResult {
        // One `otxn_type` host call and one integer compare against
        // `ttPAYMENT`. The view is a ZST: every accessor below is a single
        // `otxn_field` call, the same call a hand-written hook would make.
        let Ok(payment) = tx::Payment::otxn() else {
            rollback!(b"typed-views: not a Payment", ViewError::NotAPayment)
        };

        // `sfAmount` is `soeREQUIRED`, so the accessor returns `Result<T>`,
        // not `Result<Option<T>>`. `AmountBytes` is what the host actually
        // hands back — 8 bytes native, 48 bytes IOU — classified by length.
        let Ok(amount) = payment.amount() else {
            rollback!(b"typed-views: no Amount", ViewError::MissingAmount)
        };
        let AmountBytes::Iou(iou) = amount else {
            accept!(b"typed-views: native payment, not gated", 0)
        };

        // `sfDestinationTag` is `soeOPTIONAL`: absent is `Ok(None)`, not an
        // error. Being able to *see* that distinction is the whole reason
        // optional accessors exist. This policy happens to reject both
        // absence and a failed read — but it is choosing to, rather than
        // being unable to tell them apart, which is what an accessor that
        // folded absence into `Err` would leave you with.
        match payment.destination_tag() {
            Ok(Some(_)) => {}
            _ => rollback!(
                b"typed-views: IOU payment without a DestinationTag",
                ViewError::MissingDestinationTag
            ),
        }

        // The trust line that gates *receipt* is this account's line to the
        // issuer of the currency being paid — which the payment's own
        // `Amount` names. `keylet_line_for_asset` takes the
        // currency/issuer pair straight from it.
        let Ok(me) = hook_account_buf() else {
            rollback!(b"typed-views: no hook account", ViewError::NoHookAccount)
        };
        let asset = iou.asset();
        let Ok(keylet) = keylet_line_for_asset(&me, &asset) else {
            rollback!(b"typed-views: keylet_line failed", ViewError::KeyletFailed)
        };
        // `from_keylet` is `slot_set` plus the view's own
        // `sfLedgerEntryType == ltRIPPLE_STATE` check. A missing line and a
        // wrong-typed object both land here.
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
            // Which freeze bit is *ours* depends on which side we are — one
            // integer compare, no host call.
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

        // The issuer's own account settings. `AccountRoot` is the second
        // generated ledger view; `from_keylet` checks
        // `sfLedgerEntryType == ltACCOUNT_ROOT` the same way.
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
