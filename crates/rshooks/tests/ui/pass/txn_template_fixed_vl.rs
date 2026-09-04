//! `fixed_vl(sfX, N)` compiles, both with and without a declared default,
//! and its setter (reached through a homogeneous array element's own
//! accessor) is reachable and callable.

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
                memo_type: fixed_vl(sfMemoType, 4) = *b"note",
                memo_data: fixed_vl(sfMemoData, 8),
            }; 1
        ],
        emit_details: emit_details,
    }
}

fn main() {
    let mut txn = Remit::new();
    let mut memo = txn.memos(0).expect("index in range");
    memo.set_memo_data(&[0xAB; 8]);
    memo.set_memo_type(b"tag2");
    assert!(txn.memos(1).is_none());
}
