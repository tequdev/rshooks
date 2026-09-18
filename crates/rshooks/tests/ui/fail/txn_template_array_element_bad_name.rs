//! Array elements are positional only -- an explicit name is rejected
//! before any arm sees it, not treated as an override.

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
        amounts: sfAmounts [
            usd: sfAmountEntry { amount: sfAmount = AnyAmount() },
        ],
        emit_details: emit_details,
    }
}

fn main() {}
