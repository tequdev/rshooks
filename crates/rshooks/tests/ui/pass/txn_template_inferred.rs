//! Inferred-kind `txn_template!` fields (`name: sfXxx`/`name: sfXxx =
//! default`/`name: sfXxx { .. }`/`name: sfXxx [ .. ]`) compile, and every
//! generated setter -- top-level, through a homogeneous element view, and
//! through a named array's inferred object element -- is reachable and
//! callable.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        flags: sfFlags = 0,
        sequence: sfSequence = 0,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        invoice_id: sfInvoiceID,
        fee: sfFee = NativeAmount(0,),
        signing_pub_key: sfSigningPubKey = [],
        account: sfAccount,
        destination: sfDestination,
        // Homogeneous, indexed array whose element's `fixed_vl` fields
        // infer their length from the default literal itself.
        memos: sfMemos [
            Memo: sfMemo {
                memo_type: sfMemoType = *b"note",
                memo_data: sfMemoData = [0; 8],
            }; 1
        ],
        // Named array (no repetition count) with an inferred object
        // element.
        grants: sfHookGrants [
            grant: sfHookGrant {
                hook_hash: sfHookHash,
                authorize: sfAuthorize,
            },
        ],
        // Homogeneous, indexed array whose element type is inferred.
        amounts: sfAmounts [
            Entry: sfAmountEntry {
                amount: sfAmount = IouAmount(
                    XFL::from_raw_bits(0),
                    CurrencyCode::from_iso(b"USD"),
                    AccountId([0x11; ACC_ID_LEN]),
                ),
            }; 2
        ],
        emit_details: emit_details,
    }
}

fn main() {
    let mut txn = Remit::new();
    txn.set_flags(1);
    txn.set_sequence(1);
    txn.set_first_ledger_sequence(1);
    txn.set_last_ledger_sequence(1);
    txn.set_account(&AccountId::default());
    txn.set_destination(&AccountId::default());
    txn.set_invoice_id(&Hash::default());

    let mut memo = txn.memos(0).expect("index in range");
    memo.set_memo_data(&[0xAB; 8]);
    assert!(txn.memos(1).is_none());

    for i in 0..2 {
        let mut entry = txn.amounts(i).expect("index in range");
        entry.set_amount_value(XFL::from_raw_bits(0));
    }
    assert!(txn.amounts(2).is_none());

    txn.set_grants_grant_hook_hash(&Hash::default());
    txn.set_grants_grant_authorize(&AccountId::default());
}
