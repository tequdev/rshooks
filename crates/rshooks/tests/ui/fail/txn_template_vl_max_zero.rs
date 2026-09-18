//! `vl(sfX, MAX)` with `MAX = 0` is a compile error -- `empty_vl` is the
//! one spelling for an always-empty blob.

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
        memo_format: vl(sfMemoFormat, 0),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
