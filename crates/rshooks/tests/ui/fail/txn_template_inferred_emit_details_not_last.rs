//! `emit_details` declared but not last, at the top level, is rejected with
//! a named error — not the confusing `cannot find value \`emit_details\``
//! the inferred-scalar arm would otherwise produce by treating the bare
//! `emit_details` keyword as an unrecognized `sfXxx` ident.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct NotLast {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
        flags: sfFlags = 0,
    }
}

fn main() {}
