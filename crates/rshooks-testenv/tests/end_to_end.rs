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
        sequence: sfSequence = 0,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        amount: sfAmount = NativeAmount(0),
        fee: sfFee = NativeAmount(0),
        signing_pub_key: sfSigningPubKey = [],
        account: sfAccount,
        destination: sfDestination,
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
        flags: sfFlags = tfCANONICAL,
        sequence: sfSequence = 0,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        fee: sfFee = NativeAmount(0),
        signing_pub_key: sfSigningPubKey = [],
        account: sfAccount,
        destination: sfDestination,
        amounts: sfAmounts [
            sfAmountEntry {
                amount: sfAmount = NativeAmount(1),
            },
            sfAmountEntry {
                amount: sfAmount = IouAmount(
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
        if tpl.set_amounts_0_amount(2_000_000).is_err() {
            rollback!(b"native amount out of range", 2);
        }
        tpl.set_amounts_1_amount_value(XFL::one());
        let prepared = match tpl.prepare_for_emit() {
            Ok(p) => p,
            Err(_) => rollback!(b"prepare_for_emit failed", 3),
        };
        if prepared.emit().is_err() {
            rollback!(b"emit failed", 4);
        }
        accept!(b"remitted", 0)
    }

    /// Like `remit`, but hand-splices NOP (`0x99`) bytes into the prepared
    /// bytes at two field-header positions before emitting them directly
    /// via `emit_buf` — one run between two top-level fields (right before
    /// `sfDestination`'s own header), one run inside the nested
    /// `sfAmountEntry` object of the native `sfAmounts` entry (right after
    /// its own `sfAmount` field, before that entry's own `0xE1`
    /// terminator). Proves the emission walker's tolerant NOP handling end
    /// to end: `crate::backend::Backend::emit`'s acceptance check accepts
    /// the NOP-padded bytes, and the stored `EmittedTxn` holds their
    /// canonical (NOP-free) form. The pre-splice bytes are stashed in state
    /// (`nop_test_plain_blob`) so the test can assert the stored emitted
    /// blob is byte-for-byte identical to them — this hook's own `emit`
    /// call is the only emission in its invocation, so the pre-splice and
    /// post-emit bytes share the exact same `EmitDetails` (same nonce,
    /// generation, burden), making a plain equality check meaningful.
    #[hook(4, on = [Invoke], can_emit = [Remit])]
    fn remit_with_nops(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(b"reserve failed", 1);
        }
        let mut tpl = RemitTemplate::new();
        tpl.set_destination(&AccountId([0xCDu8; 20]));
        if tpl.set_amounts_0_amount(2_000_000).is_err() {
            rollback!(b"native amount out of range", 2);
        }
        tpl.set_amounts_1_amount_value(XFL::one());
        let prepared = match tpl.prepare_for_emit() {
            Ok(p) => p,
            Err(_) => rollback!(b"prepare_for_emit failed", 3),
        };
        let base = prepared.as_bytes().to_vec();

        // The native `sfAmountEntry`'s own field bytes: locating this
        // (header-included, unbroken) pattern gives the offset right
        // after its `sfAmount` value, i.e. right before that entry's own
        // `0xE1` — a NOP inserted there stays "inside a nested object,
        // after a field" without disturbing this pattern's own bytes.
        let (entry_hdr, entry_hdr_len) = rshooks::txn::codec::field_header(sfAmountEntry);
        let (amount_hdr, amount_hdr_len) = rshooks::txn::codec::field_header(sfAmount);
        let mut native_prefix = Vec::new();
        native_prefix.extend_from_slice(&entry_hdr[..entry_hdr_len]);
        native_prefix.extend_from_slice(&amount_hdr[..amount_hdr_len]);
        native_prefix.extend_from_slice(&(2_000_000u64 | 0x4000_0000_0000_0000).to_be_bytes());

        // `sfDestination`'s own header+value: a NOP inserted right before
        // it stays "between two top-level fields" without disturbing this
        // pattern's own bytes either.
        let (dest_hdr, dest_hdr_len) = rshooks::txn::codec::field_header(sfDestination);
        let mut dest_prefix = Vec::new();
        dest_prefix.extend_from_slice(&dest_hdr[..dest_hdr_len]);
        dest_prefix.push(rshooks::types::ACC_ID_LEN as u8);
        dest_prefix.extend_from_slice(&[0xCDu8; 20]);

        let Some(native_end) = base
            .windows(native_prefix.len())
            .position(|w| w == native_prefix.as_slice())
            .map(|i| i + native_prefix.len())
        else {
            rollback!(b"native amount pattern not found", 5);
        };
        let Some(dest_start) = base
            .windows(dest_prefix.len())
            .position(|w| w == dest_prefix.as_slice())
        else {
            rollback!(b"destination pattern not found", 6);
        };

        let mut spliced = Vec::with_capacity(base.len() + 5);
        if dest_start >= native_end {
            spliced.extend_from_slice(&base[..native_end]);
            spliced.extend_from_slice(&[0x99, 0x99]); // inside the nested object
            spliced.extend_from_slice(&base[native_end..dest_start]);
            spliced.extend_from_slice(&[0x99, 0x99, 0x99]); // between top-level fields
            spliced.extend_from_slice(&base[dest_start..]);
        } else {
            spliced.extend_from_slice(&base[..dest_start]);
            spliced.extend_from_slice(&[0x99, 0x99, 0x99]);
            spliced.extend_from_slice(&base[dest_start..native_end]);
            spliced.extend_from_slice(&[0x99, 0x99]);
            spliced.extend_from_slice(&base[native_end..]);
        }

        if rshooks::api::state::state_set(&base, b"nop_test_plain_blob").is_err() {
            rollback!(b"state_set failed", 7);
        }
        if rshooks::api::etxn::emit_buf(&spliced).is_err() {
            rollback!(b"emit failed", 4);
        }
        accept!(b"remitted with nops", 0)
    }
}

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        // `remit_with_nops` stashes its full prepared `RemitTemplate` bytes
        // (comfortably over the default 256-byte cap) into state for the
        // matching test to compare against.
        .max_state_value_len(512)
        .otxn(
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
    assert!(!emitted[0].blob.is_empty());
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
    let blob = &emitted[0].blob;

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

/// `Payments::remit_with_nops` hand-splices `0x99` NOP bytes into an
/// otherwise-valid Remit blob (one run between two top-level fields, one
/// run inside the nested native `sfAmountEntry` object) before emitting
/// directly via `emit_buf` — see that entry's own doc comment for exactly
/// where — and stashes the pre-splice bytes into state. Proves NOP
/// canonicalization end to end: the NOP-padded blob is accepted
/// (`crate::backend::Backend::emit`'s NOP-tolerant acceptance check), its
/// `TransactionType` still decodes correctly, and the *stored* `EmittedTxn`
/// blob is byte-for-byte identical to the pre-splice bytes — i.e. `emit`
/// stores the canonical (NOP-free) form, exactly as real xahaud's own
/// `STTx(SerialIter&)` parse-then-store would, not the padded bytes the
/// hook actually passed in.
#[test]
fn remit_with_nops_stores_the_canonical_nop_free_blob() {
    let env = env();
    let exit = env.invoke::<Payments>(4);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    let emitted = env.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
    let blob = &emitted[0].blob;

    let plain = env
        .state(b"nop_test_plain_blob")
        .expect("remit_with_nops stashes the pre-splice bytes");
    assert_eq!(
        *blob, plain,
        "the stored emitted blob must be byte-for-byte identical to the \
         pre-splice (NOP-free) bytes: {blob:02x?} vs {plain:02x?}"
    );
}
