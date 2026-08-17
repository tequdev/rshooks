#![no_std]

use rshooks::prelude::*;
use rshooks::*;

hook_errors! {
    /// Errors returned by the slot example.
    pub enum SlotLedgerError {
        /// The originating transaction could not be loaded.
        OtxnSlotFailed = 1,
        /// The transaction has no destination field.
        NoDestinationField = 2,
        /// The destination has an unexpected size.
        UnexpectedDestinationSize = 3,
        /// The transaction has no amount field.
        NoAmountField = 4,
        /// The amount is not native.
        UnsupportedAmount = 5,
        /// The amount size could not be read.
        SlotSizeFailed = 6,
        /// The amount could not be read.
        AmountReadFailed = 7,
    }
}

#[hooks]
pub struct SlotLedger;

#[hooks]
impl SlotLedger {
    #[hook(0, on = [Payment])]
    fn main() -> i64 {
        let Ok(txn) = SlotObject::from_otxn() else {
            rollback!(
                b"slot-ledger: otxn_slot failed",
                SlotLedgerError::OtxnSlotFailed
            )
        };

        let Ok(dest_slot) = txn.get(sfDestination) else {
            rollback!(
                b"slot-ledger: no Destination field on otxn",
                SlotLedgerError::NoDestinationField
            )
        };
        let Ok(dest) = dest_slot.value() else {
            rollback!(
                b"slot-ledger: Destination has unexpected size",
                SlotLedgerError::UnexpectedDestinationSize
            )
        };

        let Ok(amount_slot) = txn.get(sfAmount) else {
            rollback!(
                b"slot-ledger: no Amount field on otxn",
                SlotLedgerError::NoAmountField
            )
        };

        match amount_slot.size() {
            Ok(n) if n as usize == NATIVE_AMOUNT_LEN => {}
            Ok(_) => rollback!(
                b"slot-ledger: unsupported (non-native) Amount",
                SlotLedgerError::UnsupportedAmount
            ),
            Err(_) => rollback!(
                b"slot-ledger: slot_size failed for Amount",
                SlotLedgerError::SlotSizeFailed
            ),
        }

        let amount_buf: NativeAmount = match amount_slot.raw_exact::<NATIVE_AMOUNT_LEN>() {
            Ok(a) => NativeAmount(a),
            Err(_) => rollback!(
                b"slot-ledger: reading Amount from its slot failed",
                SlotLedgerError::AmountReadFailed
            ),
        };

        let marker = u16::from(dest.first().copied().unwrap_or(0))
            .wrapping_add(u16::from(amount_buf.first().copied().unwrap_or(0)));
        accept!(
            b"slot-ledger: read Destination and native Amount",
            i64::from(marker)
        )
    }
}
