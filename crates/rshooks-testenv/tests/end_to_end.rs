//! End-to-end: a real `#[hooks]` chain (a counter entry exercising
//! accept/rollback, an emitter entry using `txn_template!`, and a second
//! emitter whose `txn_template!` declares fixed nested `object`/`array`
//! fields) driven through `TestEnv::invoke` — state assertions, exit
//! codes, emission capture.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::prelude::*;
use rshooks::*;
use rshooks_testenv::prelude::*;

txn_template! {
    /// A simple payout template: destination + amount, everything else
    /// filled in by `prepare_for_emit`.
    pub struct PayoutTemplate {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        amount: native_amount(sfAmount) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        destination: account_id(sfDestination),
        emit_details: emit_details,
    }
}

/// The issued entry's baked currency and issuer for [`RemitTemplate`].
const REMIT_USD: CurrencyCode = CurrencyCode::from_iso(b"USD");
const REMIT_USD_ISSUER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");

txn_template! {
    /// A Remit template with a fixed two-entry `sfAmounts` array: one
    /// native-amount entry, one issued-amount entry with a baked
    /// currency/issuer default.
    pub struct RemitTemplate {
        transaction_type = ttREMIT,
        flags: u32_field(sfFlags) = tfCANONICAL,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        destination: account_id(sfDestination),
        amounts: array(sfAmounts) [
            native: object(sfAmountEntry) {
                amount: native_amount(sfAmount) = 1,
            },
            usd: object(sfAmountEntry) {
                amount: amount(sfAmount) = (
                    XFL::from_raw_bits(0),
                    REMIT_USD,
                    REMIT_USD_ISSUER
                ),
            },
        ],
        emit_details: emit_details,
    }
}

#[hooks]
pub struct Payments {
    /// Persistent invocation counter.
    #[state(key = b"counter")]
    counter: State<u64>,
}

#[hooks]
impl Payments {
    /// Increments the counter and accepts with the new count — rolls back
    /// if the store itself fails.
    #[hook(0, on = [Payment])]
    fn on_payment(&self) -> HookResult {
        let count = self.state.counter.get().unwrap_or(None).unwrap_or(0);
        let next = count.wrapping_add(1);
        if self.state.counter.set(&next).is_err() {
            rollback!(b"counter: store failed", 1);
        }
        accept!(b"counted", next as i64)
    }

    /// Writes the counter, then unconditionally rolls back — proves the
    /// state write is undone (design §5's "rollback reverts state" rule).
    #[hook(1, on = [Invoke])]
    fn write_then_force_rollback(&self) -> HookResult {
        let _ = self.state.counter.set(&999u64);
        rollback!(b"forced rollback", 42)
    }

    /// Reserves one emission slot and emits a single Payment.
    #[hook(2, on = [Invoke], can_emit = [Payment])]
    fn payout(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(b"reserve failed", 1);
        }
        let mut tpl = PayoutTemplate::new();
        tpl.set_destination(&AccountId([0xABu8; 20]));
        if tpl.set_amount(1_000_000).is_err() {
            rollback!(b"amount out of range", 2);
        }
        let prepared = match tpl.prepare_for_emit() {
            Ok(p) => p,
            Err(_) => rollback!(b"prepare_for_emit failed", 3),
        };
        if prepared.emit().is_err() {
            rollback!(b"emit failed", 4);
        }
        accept!(b"paid", 0)
    }

    /// Reserves one emission slot and emits a single Remit whose
    /// `sfAmounts` array is a fixed, two-entry nested shape declared in
    /// `RemitTemplate`.
    #[hook(3, on = [Invoke], can_emit = [Remit])]
    fn remit(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(b"reserve failed", 1);
        }
        let mut tpl = RemitTemplate::new();
        tpl.set_destination(&AccountId([0xCDu8; 20]));
        if tpl.set_amounts_native_amount(2_000_000).is_err() {
            rollback!(b"native amount out of range", 2);
        }
        tpl.set_amounts_usd_amount_value(XFL::one());
        let prepared = match tpl.prepare_for_emit() {
            Ok(p) => p,
            Err(_) => rollback!(b"prepare_for_emit failed", 3),
        };
        if prepared.emit().is_err() {
            rollback!(b"emit failed", 4);
        }
        accept!(b"remitted", 0)
    }
}

fn env() -> TestEnv {
    TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .amount_drops(1_000_000),
    )
}

#[test]
fn first_payment_counts_to_one() {
    let env = env();
    let exit = env.invoke::<Payments>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, 1);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(1));
}

#[test]
fn counter_persists_across_invocations() {
    let env = env();
    env.invoke::<Payments>(0);
    env.invoke::<Payments>(0);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(2));
}

#[test]
fn rollback_reverts_state_writes() {
    let env = env().state_entry(b"counter", &7u64.to_le_bytes());
    let exit = env.invoke::<Payments>(1);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, 42);
    // The write to 999 inside the entry must be undone.
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(7));
}

#[test]
fn payout_emits_one_payment() {
    let env = env();
    let exit = env.invoke::<Payments>(2);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Payment));
    assert!(!emitted[0].blob().is_empty());
}

/// Byte-exact check of the nested `sfAmounts` region emitted by
/// `Payments::remit`, plus proof that `prepare_for_emit` patched the
/// top-level `sfAccount` to the hook account while leaving the nested
/// entries untouched — the "top-level only" rule `txn_template!`'s
/// plumbing-field detection documents.
#[test]
fn remit_emits_one_remit_with_nested_amounts() {
    let env = env();
    let exit = env.invoke::<Payments>(3);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
    let blob = emitted[0].blob();

    let (amounts_hdr, amounts_hdr_len) = rshooks::txn::codec::field_header(sfAmounts);
    let (entry_hdr, entry_hdr_len) = rshooks::txn::codec::field_header(sfAmountEntry);
    let (amount_hdr, amount_hdr_len) = rshooks::txn::codec::field_header(sfAmount);

    let mut expected_amounts = Vec::new();
    expected_amounts.extend_from_slice(&amounts_hdr[..amounts_hdr_len]);
    expected_amounts.extend_from_slice(&entry_hdr[..entry_hdr_len]);
    expected_amounts.extend_from_slice(&amount_hdr[..amount_hdr_len]);
    expected_amounts.extend_from_slice(&(2_000_000u64 | 0x4000_0000_0000_0000).to_be_bytes());
    expected_amounts.push(0xE1); // object end marker
    expected_amounts.extend_from_slice(&entry_hdr[..entry_hdr_len]);
    expected_amounts.extend_from_slice(&amount_hdr[..amount_hdr_len]);
    // `XFL::one()`'s issued STAmount value bytes (`rshooks/src/txn.rs`'s
    // `encode_iou_amount_value_const_one` test derives the same vector by
    // hand from the XFL bit layout).
    expected_amounts.extend_from_slice(&[0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]);
    let mut currency = [0u8; 20];
    currency[12..15].copy_from_slice(b"USD");
    expected_amounts.extend_from_slice(&currency);
    expected_amounts.extend_from_slice(&REMIT_USD_ISSUER.0);
    expected_amounts.push(0xE1); // object end marker
    expected_amounts.push(0xF1); // array end marker
    assert!(
        blob.windows(expected_amounts.len())
            .any(|w| w == expected_amounts.as_slice()),
        "sfAmounts region not found in the emitted blob: {blob:02x?}"
    );

    let (account_hdr, account_hdr_len) = rshooks::txn::codec::field_header(sfAccount);
    let mut expected_account = Vec::new();
    expected_account.extend_from_slice(&account_hdr[..account_hdr_len]);
    expected_account.push(rshooks::types::ACC_ID_LEN as u8);
    expected_account.extend_from_slice(&[1u8; 20]); // patched to the hook account
    assert!(
        blob.windows(expected_account.len())
            .any(|w| w == expected_account.as_slice()),
        "sfAccount was not patched to the hook account: {blob:02x?}"
    );
}
