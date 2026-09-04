//! A homogeneous array's element count must be at least 1 -- `; 0` is
//! rejected at compile time rather than silently producing a zero-length
//! reserved region no accessor could ever return `Some` for.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        amounts: array(sfAmounts) [
            AmountEntry: object(sfAmountEntry) {
                amount: native_amount(sfAmount) = 1,
            }; 0
        ],
        emit_details: emit_details,
    }
}

fn main() {}
