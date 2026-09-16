//! An array element's explicit name must be an identifier or a plain
//! index (`0`, `1`, ..): a suffixed literal such as `1u8` cannot be spliced
//! into a generated method name, so the element list is rejected before
//! any arm sees it.

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
            1u8: object(sfAmountEntry) { amount: native_amount(sfAmount) = 1 },
        ],
        emit_details: emit_details,
    }
}

fn main() {}
