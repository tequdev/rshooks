#![no_std]

use rshooks::prelude::*;
use rshooks::*;

/// The numerator for the computed percentage.
const PERCENT_NUM: u32 = 1;
/// The denominator for the computed percentage.
const PERCENT_DEN: u32 = 100;

hook_errors! {
    /// Errors returned by the XFL example.
    pub enum XflMathError {
        /// The originating transaction could not be loaded.
        OtxnSlotFailed = 1,
        /// The transaction has no amount field.
        NoAmountField = 2,
        /// The amount is not valid XFL.
        InvalidAmount = 3,
        /// The percentage could not be computed.
        MulratioFailed = 4,
        /// The minimum share could not be constructed.
        MinShareConstructFailed = 5,
        /// The share comparison failed.
        ComparisonFailed = 6,
        /// The share is below the minimum.
        BelowMinimum = 7,
        /// The remaining amount could not be computed.
        RemainingComputeFailed = 8,
        /// The remaining amount comparison failed.
        RemainingComparisonFailed = 9,
        /// The remaining amount is not positive.
        NotEnoughRemaining = 10,
        /// The growth factor could not be constructed.
        GrowthConstructFailed = 11,
        /// The compounded amount is invalid.
        CompoundValidationFailed = 12,
        /// The compounded amount comparison failed.
        CompoundComparisonFailed = 13,
        /// The compounded amount did not increase.
        CompoundNotIncreasing = 14,
        /// The compounded amount exceeds the remaining amount.
        CompoundExceedsRemaining = 15,
    }
}

#[hooks]
pub struct XflMath;

#[hooks]
impl XflMath {
    // XFL operators delegate to checked Hook API calls.
    #[allow(clippy::arithmetic_side_effects)]
    #[hook(0, on = [Payment])]
    fn main(&self) -> HookResult {
        let Ok(txn) = SlotObject::from_otxn() else {
            rollback!(b"xfl-math: otxn_slot failed", XflMathError::OtxnSlotFailed)
        };
        let Ok(amount_slot) = txn.get(sfAmount) else {
            rollback!(
                b"xfl-math: no Amount field on otxn",
                XflMathError::NoAmountField
            )
        };

        let Ok(amount) = amount_slot.as_xfl() else {
            rollback!(
                b"xfl-math: Amount is not a valid XFL amount",
                XflMathError::InvalidAmount
            )
        };

        let Ok(share) = amount.mulratio(false, PERCENT_NUM, PERCENT_DEN) else {
            rollback!(
                b"xfl-math: mulratio failed (overflow?)",
                XflMathError::MulratioFailed
            )
        };

        let Ok(min_share) = XFL::new(-21, 1_000_000_000_000_000) else {
            rollback!(
                b"xfl-math: could not construct min_share",
                XflMathError::MinShareConstructFailed
            )
        };

        match share.lt(min_share) {
            Ok(true) => rollback!(
                b"xfl-math: computed share below minimum",
                XflMathError::BelowMinimum
            ),
            Ok(false) => {}
            Err(_) => rollback!(
                b"xfl-math: comparison failed",
                XflMathError::ComparisonFailed
            ),
        }

        let Ok(remaining) = amount - share else {
            rollback!(
                b"xfl-math: amount - share failed",
                XflMathError::RemainingComputeFailed
            )
        };
        match remaining.compare(XFL::from_raw_bits(0), COMPARE_LESS | COMPARE_EQUAL) {
            Ok(true) => rollback!(
                b"xfl-math: amount - share was not positive",
                XflMathError::NotEnoughRemaining
            ),
            Ok(false) => {}
            Err(_) => rollback!(
                b"xfl-math: remaining comparison failed",
                XflMathError::RemainingComparisonFailed
            ),
        }

        let Ok(growth) = XFL::new(-15, 1_010_000_000_000_000) else {
            rollback!(
                b"xfl-math: could not construct growth factor",
                XflMathError::GrowthConstructFailed
            )
        };
        let compounded_raw =
            share.unchecked() * growth.unchecked() * growth.unchecked() * growth.unchecked();
        let Ok(compounded) = compounded_raw.validate() else {
            rollback!(
                b"xfl-math: compounded share failed to validate",
                XflMathError::CompoundValidationFailed
            )
        };
        match compounded.compare(share, COMPARE_LESS | COMPARE_EQUAL) {
            Ok(true) => rollback!(
                b"xfl-math: compounded share did not increase",
                XflMathError::CompoundNotIncreasing
            ),
            Ok(false) => {}
            Err(_) => rollback!(
                b"xfl-math: compound comparison failed",
                XflMathError::CompoundComparisonFailed
            ),
        }

        if compounded > remaining {
            rollback!(
                b"xfl-math: compounded share unexpectedly exceeds remaining amount",
                XflMathError::CompoundExceedsRemaining
            );
        }

        let _ = txn.clear();

        accept!()
    }
}
