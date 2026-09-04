//! An unrecognized kind keyword is rejected with a named
//! "unrecognized field declaration" error, not the bare "no rules
//! expected the token" failure `macro_rules!` would otherwise produce.

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
        account: account_id(sfAccount),
        bogus: bogus_kind(sfDestination),
        emit_details: emit_details,
    }
}

fn main() {}
