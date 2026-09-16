//! Every `optional` scalar kind, `any_amount`/`optional any_amount`,
//! `vl`/`optional vl`, whole-container `optional object`/`optional
//! array` (both the explicit `optional object(sfX) { .. }`/`optional
//! array(sfX) [ .. ]` spelling, on `opt_obj`/`opt_arr`, and the bare
//! `optional sfX { .. }` spelling, on `second`), and a homogeneous array
//! of `optional object` elements compile, and each generated setter/
//! accessor is reachable and callable. A named `optional` container has
//! no view type: its own fields are plain `set_<name>_<..>` methods on
//! the parent, and any of them makes the container present. Split across
//! several nested `object`s -- each field's worst-case NOP charge
//! (`docs/NOP_PADDING_DESIGN.md` §3.1) is checked against the
//! 63-per-container budget independently per container, and no single
//! container can hold every kind from this fixture at once.

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

        // Small kinds: u8/u16/u32/u64/hash128/native_amount/empty_vl/fixed_vl
        // (3 + 3 + 5 + 9 + 17 + 9 + 2 + 6 = 54).
        small: object(sfMemo) {
            b: optional u16_field(sfSignerWeight),
            c: optional u32_field(sfSourceTag),
            d: optional u64_field(sfIndexNext),
            e: optional hash128(sfEmailHash),
            f: optional sfAmount = NativeAmount(),
            h: optional fixed_vl(sfMemoType, 4),
            g: optional empty_vl(sfMemoData),
            a: optional u8_field(sfTransactionResult),
        },

        // hash160 + currency (22 + 22 = 44).
        large1: object(sfSignerEntry) {
            a: optional hash160(sfTakerPaysCurrency),
            b: optional currency(sfBaseAsset),
        },

        // hash256, alone (34).
        large2: object(sfHook) {
            a: optional hash256(sfInvoiceID),
        },

        // issue, alone (42).
        misc1: object(sfSigner) {
            a: optional issue(sfClaimCurrency),
        },

        // account_id, alone (22).
        misc2: object(sfMajority) {
            a: optional account_id(sfAuthorize),
        },

        // any_amount (40) + hash160 (22) = 62.
        amt1: object(sfDisabledValidator) {
            a: any_amount(sfBalance),
            b: optional hash160(sfTakerPaysCurrency),
        },

        // optional any_amount, alone (49).
        amt2: object(sfEmittedTxn) {
            a: optional sfLimitAmount = AnyAmount(),
        },

        // optional amount (48-byte issued form), alone (49).
        amt3: object(sfHookExecution) {
            a: optional sfSendMax = IouAmount(),
        },

        // vl(2..=6) (4) + optional vl(1..=4) (6) = 10.
        blobs: object(sfHookParameter) {
            b: optional vl(sfURI, 1, 4),
            a: vl(sfDomain, 2, 6),
        },

        // Whole-container-optional `object` (12), top level.
        opt_obj: optional object(sfHookGrant) {
            amount: native_amount(sfAmount) = 0,
        },

        // Homogeneous array of optional elements (self-contained budget:
        // 2 * 12 = 24), array itself always present, top level.
        opt_elems: array(sfHookGrants) [
            Grant: optional object(sfHookGrant) {
                amount: native_amount(sfAmount) = 0,
            }; 2
        ],

        // Whole-container-optional `array` (15), top level.
        opt_arr: optional array(sfAmounts) [
            entry: object(sfAmountEntry) {
                amount: native_amount(sfAmount) = 0,
            },
        ],

        emit_details: emit_details,
    }
}

txn_template! {
    /// Motivating case: a named array with one required element and one
    /// `optional` element -- a homogeneous array's elements share one
    /// shape, so a named array with per-element presence is the right
    /// tool for "one required entry, one that may or may not be there".
    struct RemitAmounts {
        transaction_type = ttREMIT,
        sequence: u32_field(sfSequence) = 0,
        destination_tag: optional sfDestinationTag,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        amounts: sfAmounts [
            first: sfAmountEntry {
                amount: amount(sfAmount),
            },
            second: optional sfAmountEntry {
                amount: amount(sfAmount),
            },
        ],
        emit_details: emit_details,
    }
}

fn main() {
    let mut txn = Remit::new();

    txn.set_small_a(1);
    txn.clear_small_a();
    txn.set_small_b(2);
    txn.clear_small_b();
    txn.set_small_c(3);
    txn.clear_small_c();
    txn.set_small_d(4);
    txn.clear_small_d();
    txn.set_small_e(&[0u8; 16]);
    txn.clear_small_e();
    txn.set_small_f(5).expect("5 drops is in range");
    txn.clear_small_f();
    txn.set_small_g();
    txn.clear_small_g();
    txn.set_small_h(b"note");
    txn.clear_small_h();

    txn.set_large1_a(&[0u8; 20]);
    txn.clear_large1_a();
    let currency = CurrencyCode::from_iso(b"USD");
    txn.set_large1_b(&currency);
    txn.clear_large1_b();

    let hash = Hash([0u8; 32]);
    txn.set_large2_a(&hash);
    txn.clear_large2_a();

    let issuer = AccountId::default();
    txn.set_misc1_a(&currency, &issuer);
    txn.clear_misc1_a();

    txn.set_misc2_a(&issuer);
    txn.clear_misc2_a();

    txn.set_amt1_a_native(1).expect("1 drop is in range");
    txn.set_amt1_a_issued(XFL!(0), &currency, &issuer);
    txn.set_amt1_b(&[0u8; 20]);
    txn.clear_amt1_b();

    txn.set_amt2_a_native(1).expect("1 drop is in range");
    txn.set_amt2_a_issued(XFL!(0), &currency, &issuer);
    txn.clear_amt2_a();

    txn.set_amt3_a(XFL!(0), &currency, &issuer);
    txn.clear_amt3_a();

    txn.set_blobs_a(&[1, 2, 3]).expect("3 bytes is within [2, 6]");
    txn.set_blobs_b(&[9]).expect("1 byte is within [1, 4]");
    txn.clear_blobs_b();

    txn.set_opt_obj_amount(1).expect("1 drop is in range");
    let _ = txn.is_opt_obj_present();
    txn.enable_opt_obj();
    txn.clear_opt_obj();

    let mut g0 = txn.opt_elems(0).expect("index 0 is in range");
    g0.enable();
    g0.set_amount(1).expect("1 drop is in range");
    g0.clear();
    assert!(txn.opt_elems(2).is_none());

    txn.set_opt_arr_entry_amount(1).expect("1 drop is in range");
    let _ = txn.is_opt_arr_present();
    txn.enable_opt_arr();
    txn.clear_opt_arr();

    let _ = &txn;

    let mut amounts = RemitAmounts::new();
    amounts.set_destination_tag(1);
    amounts.clear_destination_tag();
    let currency = CurrencyCode::from_iso(b"USD");
    let issuer = AccountId::default();
    amounts.set_amounts_first_amount(XFL!(0), &currency, &issuer);
    amounts.set_amounts_second_amount(XFL!(0), &currency, &issuer);
    let _ = amounts.is_amounts_second_present();
    amounts.enable_amounts_second();
    amounts.clear_amounts_second();
    let _ = &amounts;
}
