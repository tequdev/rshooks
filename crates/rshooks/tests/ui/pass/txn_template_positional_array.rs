//! A named array whose elements omit their names entirely: each is
//! numbered by its zero-based position instead, both a plain
//! (always-present) element and an `optional` one, exercising the
//! generated `set_<arr>_<N>_<field>`/`enable_<arr>_<N>`/
//! `clear_<arr>_<N>`/`is_<arr>_<N>_present` names.

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
            sfAmountEntry { amount: sfAmount = AnyAmount() },
            optional sfAmountEntry { amount: sfAmount = AnyAmount() },
        ],
        emit_details: emit_details,
    }
}

fn main() {
    let mut txn = Remit::new();
    let currency = CurrencyCode::from_iso(b"USD");
    let issuer = AccountId::default();

    txn.set_amounts_0_amount_native(1)
        .expect("1 drop is in range");
    txn.set_amounts_0_amount_iou(XFL!(0), &currency, &issuer);

    assert!(!txn.is_amounts_1_present());
    txn.set_amounts_1_amount_native(1)
        .expect("1 drop is in range");
    assert!(txn.is_amounts_1_present());
    txn.clear_amounts_1();
    assert!(!txn.is_amounts_1_present());
    txn.enable_amounts_1();
    txn.set_amounts_1_amount_iou(XFL!(0), &currency, &issuer);

    let _ = &txn;
}
