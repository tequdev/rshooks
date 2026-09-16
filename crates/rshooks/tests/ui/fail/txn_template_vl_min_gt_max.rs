//! `vl(sfX, MIN, MAX)` with `MIN > MAX` is a compile error.

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
        memo_format: vl(sfMemoFormat, 6, 2),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
