//! Only `object(sfX) { .. }` elements may appear directly inside an
//! `array(sfX) [ .. ]` -- a scalar field there has no defined element shape
//! and is rejected as an unrecognized declaration.

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
            amount: native_amount(sfAmount) = 1,
        ],
        emit_details: emit_details,
    }
}

fn main() {}
