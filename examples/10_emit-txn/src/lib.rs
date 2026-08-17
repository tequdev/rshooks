#![no_std]

use rshooks::prelude::*;
use rshooks::*;

txn_template! {
    /// A payment template for emitted transactions.
    struct Payment {
        transaction_type = ttPAYMENT,
        flags: u32_field(sfFlags) = tfCANONICAL,
        source_tag: u32_field(sfSourceTag) = 0,
        sequence: u32_field(sfSequence) = 0,
        destination_tag: u32_field(sfDestinationTag) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        amount: native_amount(sfAmount) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        destination: account_id(sfDestination),
        emit_details: emit_details,
    }
}

/// The reusable payment template.
static TXN: HookStatic<Payment> = HookStatic::new(Payment::new());

hook_errors! {
    /// Errors returned by the emission hook.
    pub enum EmitTxnError {
        /// An emission slot could not be reserved.
        ReserveFailed = 1,
        /// The originating account could not be read.
        CouldNotReadSender = 2,
        /// The payment template was unavailable.
        BufferAlreadyTaken = 3,
        /// The payment amount could not be set.
        SetAmountFailed = 4,
        /// The payment could not be prepared.
        PrepareFailed = 5,
        /// The prepared payment could not be emitted.
        EmitFailed = 6,
    }
}

#[hooks(description = "Emits a Payment and handles its callback.")]
pub struct EmitTxn;

#[hooks]
impl EmitTxn {
    #[hook(0, name = "emit-tx", on = [Invoke], can_emit = [Payment])]
    fn main() -> i64 {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"emit-txn: etxn_reserve failed",
                EmitTxnError::ReserveFailed
            );
        }

        let Ok(dest) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"emit-txn: could not read otxn sender",
                EmitTxnError::CouldNotReadSender
            )
        };

        let Some(txn) = TXN.take() else {
            rollback!(
                b"emit-txn: static buffer already taken",
                EmitTxnError::BufferAlreadyTaken
            );
        };

        if txn.set_amount(1).is_err() {
            rollback!(
                b"emit-txn: amount setter failed",
                EmitTxnError::SetAmountFailed
            );
        }
        txn.set_destination(&dest);

        let Ok(prepared) = txn.prepare_for_emit() else {
            rollback!(
                b"emit-txn: prepare_for_emit failed",
                EmitTxnError::PrepareFailed
            )
        };

        match prepared.emit() {
            Ok(_hash) => accept!(b"emit-txn: emitted", 0),
            Err(_) => rollback!(b"emit-txn: emit failed", EmitTxnError::EmitFailed),
        }
    }

    #[cbak(0)]
    fn cbak() -> i64 {
        accept!()
    }
}
