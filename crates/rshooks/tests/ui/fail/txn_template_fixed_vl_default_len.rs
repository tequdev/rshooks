//! A `fixed_vl(sfX, N) = <expr>` default must be exactly `[u8; N]` -- a
//! 7-byte literal against a declared length of 4 is a type error, not a
//! silent truncation or panic.

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
        memos: array(sfMemos) [
            Memo: object(sfMemo) {
                memo_data: fixed_vl(sfMemoData, 4) = *b"toolong",
            }; 1
        ],
        emit_details: emit_details,
    }
}

fn main() {}
