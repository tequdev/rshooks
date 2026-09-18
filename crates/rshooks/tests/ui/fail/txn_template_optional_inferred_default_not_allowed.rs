//! A bare `field: optional sfXxx` takes no `= default` -- the default is
//! "absent". This is a macro-parse rejection (the catch-all "unrecognized
//! field declaration" arm), not a named `const` assertion, matching
//! `optional <scalar>(sfXxx) = default`'s own rejection.

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
        source_tag: optional sfSourceTag = 1,
        emit_details: emit_details,
    }
}

fn main() {}
