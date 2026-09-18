//! A `fixed_vl` field's declared length must be at least 1 -- `empty_vl` is
//! the one spelling for an empty blob, so `fixed_vl(sfX, 0)` is rejected at
//! compile time rather than accepted as a confusing synonym.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct Payment {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        blob: fixed_vl(sfBlob, 0),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
