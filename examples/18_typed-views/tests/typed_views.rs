//! Off-chain unit tests for the `typed-views` example, driven through
//! `TestEnv::invoke` against the real `TypedViews` chain — no wasm build, no
//! node. Every policy branch the hook can take is exercised against a
//! seeded trust line and issuer account.
//!
//! Keylets are computed independently here (not by calling into
//! `rshooks-testenv`'s crate-private `host::keylet`, and not through
//! `rshooks::api::keylet` either — those need a live backend installed,
//! which isn't the case while building the seed data *before*
//! `TestEnv::invoke` runs) — the same two-tier verification pattern
//! `examples/13_keylets/tests/keylets.rs` and
//! `examples/15_slot-objects/tests/slot_objects.rs` use:
//! `sha512Half(ledgerSpace ++ args)`, cross-checked against
//! `crates/rshooks-testenv/src/host/keylet.rs`'s own vectors.
//!
//! Serialized objects, by contrast, are built from the generated
//! `rshooks::sfield` codes rather than hand-transcribed field headers:
//! those codes are already cross-validated against the vendored
//! `sfcodes.h`, so re-typing them here would add a transcription risk
//! without adding an independent oracle.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks::prelude::*;
use rshooks_testenv::prelude::*;
use sha2::{Digest, Sha512};
use typed_views::{TypedViews, ViewError};

// -- Independent keylet computation (see module doc comment) --

fn index_hash(space: u16, parts: &[&[u8]]) -> [u8; 32] {
    let mut buf = Vec::new();
    buf.extend_from_slice(&space.to_be_bytes());
    for p in parts {
        buf.extend_from_slice(p);
    }
    let mut hasher = Sha512::new();
    hasher.update(&buf);
    let full = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&full[..32]);
    out
}

fn keylet(ty: u16, key: [u8; 32]) -> [u8; 34] {
    let mut out = [0u8; 34];
    out[0..2].copy_from_slice(&ty.to_be_bytes());
    out[2..34].copy_from_slice(&key);
    out
}

const LT_ACCOUNT_ROOT: u16 = 0x0061;
const LT_RIPPLE_STATE: u16 = 0x0072;
const SPACE_ACCOUNT: u16 = b'a' as u16;
const SPACE_TRUST_LINE: u16 = b'r' as u16;

fn keylet_account(account: [u8; 20]) -> [u8; 34] {
    keylet(LT_ACCOUNT_ROOT, index_hash(SPACE_ACCOUNT, &[&account]))
}

/// The protocol sorts the two accounts before hashing — the same
/// canonical low/high ordering the hook's `hook_is_low_side` has to
/// recover from the line's own `sfLowLimit`.
fn keylet_line(a: [u8; 20], b: [u8; 20], currency: [u8; 20]) -> [u8; 34] {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    keylet(
        LT_RIPPLE_STATE,
        index_hash(SPACE_TRUST_LINE, &[&lo, &hi, &currency]),
    )
}

// -- Wire-format builders --

/// The serialized type IDs whose wire form carries a VL length prefix.
const STI_VL: u32 = 7;
const STI_ACCOUNT: u32 = 8;

fn header(code: u32) -> Vec<u8> {
    let ty = code >> 16;
    let field = code & 0xFFFF;
    match (ty < 16, field < 16) {
        (true, true) => vec![((ty << 4) | field) as u8],
        (true, false) => vec![(ty << 4) as u8, field as u8],
        (false, true) => vec![field as u8, ty as u8],
        (false, false) => vec![0u8, ty as u8, field as u8],
    }
}

/// A root object's canonical field sequence: sorted by field code,
/// VL-prefixed where the type calls for it, no wrapping header or
/// terminator — what `slot_set`/`World::ledger_objects` expects.
fn sto(fields: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut sorted = fields.to_vec();
    sorted.sort_by_key(|(code, _)| *code);
    let mut out = Vec::new();
    for (code, value) in sorted {
        out.extend_from_slice(&header(code));
        if matches!(code >> 16, STI_VL | STI_ACCOUNT) {
            out.push(value.len() as u8);
        }
        out.extend_from_slice(&value);
    }
    out
}

/// A 48-byte IOU `STAmount`: an 8-byte value with the not-native/positive
/// flags and a normalized exponent, then the 20-byte currency, then the
/// 20-byte issuer. The hook reads the currency/issuer halves of the
/// payment's amount (to build the trust-line keylet) and the issuer half
/// of `sfLowLimit` (to learn which side it is on), so both must be real.
fn iou_amount(mantissa: u64, exponent: i32, currency: [u8; 20], issuer: [u8; 20]) -> Vec<u8> {
    let mut out = vec![0u8; 48];
    let exp_biased = (exponent + 97) as u8;
    out[0] = 0b1100_0000 | (exp_biased >> 2);
    out[1] = ((exp_biased & 0b11) << 6) | ((mantissa >> 48) & 0x3F) as u8;
    out[2] = ((mantissa >> 40) & 0xFF) as u8;
    out[3] = ((mantissa >> 32) & 0xFF) as u8;
    out[4] = ((mantissa >> 24) & 0xFF) as u8;
    out[5] = ((mantissa >> 16) & 0xFF) as u8;
    out[6] = ((mantissa >> 8) & 0xFF) as u8;
    out[7] = (mantissa & 0xFF) as u8;
    out[8..28].copy_from_slice(&currency);
    out[28..48].copy_from_slice(&issuer);
    out
}

const HOOK: [u8; 20] = [1u8; 20];
const SENDER: [u8; 20] = [2u8; 20];
const ISSUER: [u8; 20] = [6u8; 20];
/// A hook account that sorts *above* the issuer, so the canonical low/high
/// assignment flips — see `frozen_by_us_and_by_them_swap_with_the_side`.
const HIGH_HOOK: [u8; 20] = [9u8; 20];
const USD: [u8; 20] = [0xAAu8; 20];
const TAG: u32 = 42;

/// A `RippleState` between `hook` and [`ISSUER`] in [`USD`]: the required
/// fields, plus `sfLowLimit`/`sfHighLimit` whose issuer halves carry the
/// canonically-ordered low and high accounts, exactly as rippled stores
/// them.
fn trust_line_bytes(hook: [u8; 20], flags: u32) -> Vec<u8> {
    let (low, high) = if hook <= ISSUER {
        (hook, ISSUER)
    } else {
        (ISSUER, hook)
    };
    sto(&[
        (
            sfLedgerEntryType.code(),
            LT_RIPPLE_STATE.to_be_bytes().to_vec(),
        ),
        (sfFlags.code(), flags.to_be_bytes().to_vec()),
        (sfBalance.code(), iou_amount(5, -2, USD, [0u8; 20])),
        (sfLowLimit.code(), iou_amount(1, 3, USD, low)),
        (sfHighLimit.code(), iou_amount(1, 3, USD, high)),
    ])
}

/// An `AccountRoot` for [`ISSUER`]. `transfer_rate` is `soeOPTIONAL`:
/// `None` here leaves the field off the wire entirely, which is what a
/// no-fee issuer's real ledger object looks like.
fn issuer_account_bytes(transfer_rate: Option<u32>) -> Vec<u8> {
    let mut fields = vec![
        (
            sfLedgerEntryType.code(),
            LT_ACCOUNT_ROOT.to_be_bytes().to_vec(),
        ),
        (sfFlags.code(), 0u32.to_be_bytes().to_vec()),
        (sfSequence.code(), 7u32.to_be_bytes().to_vec()),
        (sfOwnerCount.code(), 3u32.to_be_bytes().to_vec()),
        (sfAccount.code(), ISSUER.to_vec()),
    ];
    if let Some(rate) = transfer_rate {
        fields.push((sfTransferRate.code(), rate.to_be_bytes().to_vec()));
    }
    sto(&fields)
}

// -- Environments --

fn iou_payment(tag: Option<u32>) -> Otxn {
    let mut otxn = Otxn::new(TxType::Payment)
        .account(SENDER)
        .destination(HOOK)
        .field_raw(sfAmount.code(), &iou_amount(1, 0, USD, ISSUER));
    if let Some(tag) = tag {
        otxn = otxn.field_raw(sfDestinationTag.code(), &tag.to_be_bytes());
    }
    otxn
}

/// The happy path: an IOU payment with a tag, an unfrozen line, and a
/// no-fee issuer.
fn env_with(hook: [u8; 20], flags: u32, transfer_rate: Option<u32>) -> TestEnv {
    TestEnv::new()
        .hook_account(hook)
        .otxn(iou_payment(Some(TAG)))
        .ledger_object(
            keylet_line(hook, ISSUER, USD),
            &trust_line_bytes(hook, flags),
        )
        .ledger_object(keylet_account(ISSUER), &issuer_account_bytes(transfer_rate))
}

fn env() -> TestEnv {
    env_with(HOOK, 0, None)
}

// -- Tests --

#[test]
fn accepts_an_iou_payment_over_a_healthy_line() {
    let exit = env().invoke::<TypedViews>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert_eq!(exit.msg, b"typed-views: incoming IOU accepted");
}

/// A native payment is out of scope and short-circuits before any ledger
/// object is touched — the `AmountBytes::Native` arm.
#[test]
fn accepts_a_native_payment_without_reading_the_ledger() {
    let env = TestEnv::new().hook_account(HOOK).otxn(
        Otxn::new(TxType::Payment)
            .account(SENDER)
            .destination(HOOK)
            .amount_drops(1_000_000),
    );
    let exit = env.invoke::<TypedViews>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert_eq!(exit.msg, b"typed-views: native payment, not gated");
}

/// The optional-field showcase: an absent `sfDestinationTag` reads as
/// `Ok(None)`, which the hook can act on. It is not an error.
#[test]
fn rejects_an_iou_payment_with_no_destination_tag() {
    let env = TestEnv::new()
        .hook_account(HOOK)
        .otxn(iou_payment(None))
        .ledger_object(keylet_line(HOOK, ISSUER, USD), &trust_line_bytes(HOOK, 0))
        .ledger_object(keylet_account(ISSUER), &issuer_account_bytes(None));
    let exit = env.invoke::<TypedViews>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, i64::from(ViewError::MissingDestinationTag));
}

#[test]
fn rejects_when_no_trust_line_exists() {
    let env = TestEnv::new()
        .hook_account(HOOK)
        .otxn(iou_payment(Some(TAG)))
        .ledger_object(keylet_account(ISSUER), &issuer_account_bytes(None));
    let exit = env.invoke::<TypedViews>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, i64::from(ViewError::NoTrustLine));
}

/// `RippleState::from_slot`/`from_keylet` verify `sfLedgerEntryType`, so an
/// object of another type at the line's keylet is refused rather than
/// misread.
#[test]
fn rejects_when_the_lines_keylet_holds_another_object_type() {
    let env = TestEnv::new()
        .hook_account(HOOK)
        .otxn(iou_payment(Some(TAG)))
        .ledger_object(keylet_line(HOOK, ISSUER, USD), &issuer_account_bytes(None))
        .ledger_object(keylet_account(ISSUER), &issuer_account_bytes(None));
    let exit = env.invoke::<TypedViews>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, i64::from(ViewError::NoTrustLine));
}

/// The low/high determination, both ways round. `HOOK` sorts below
/// `ISSUER`, so it is the *low* side and `lsfLowFreeze` is its own freeze;
/// `HIGH_HOOK` sorts above, so the same bit becomes the counterparty's.
/// Nothing but the account bytes changes between the two halves.
#[test]
fn frozen_by_us_and_by_them_swap_with_the_side() {
    assert!(HOOK < ISSUER, "HOOK must sort as the low account");
    assert!(
        HIGH_HOOK > ISSUER,
        "HIGH_HOOK must sort as the high account"
    );

    for (hook, flag, expected) in [
        (HOOK, lsfLowFreeze, ViewError::FrozenByUs),
        (HOOK, lsfHighFreeze, ViewError::FrozenByCounterparty),
        (HIGH_HOOK, lsfHighFreeze, ViewError::FrozenByUs),
        (HIGH_HOOK, lsfLowFreeze, ViewError::FrozenByCounterparty),
    ] {
        let exit = env_with(hook, flag, None).invoke::<TypedViews>(0);
        assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
        assert_eq!(
            exit.code,
            i64::from(expected),
            "hook={:?} flag={flag:#x}",
            &hook[..1]
        );
    }
}

/// `sfTransferRate` absent and an explicit 1.0 both mean "no fee" — the
/// default is the hook's to supply, since upstream's format macro records
/// only that the field may be omitted.
#[test]
fn an_absent_or_unit_transfer_rate_is_no_fee() {
    for rate in [None, Some(0), Some(1_000_000_000)] {
        let exit = env_with(HOOK, 0, rate).invoke::<TypedViews>(0);
        assert_eq!(exit.exit, ExitType::Accept, "rate={rate:?}: {exit:?}");
    }
}

#[test]
fn rejects_an_issuer_that_charges_a_transfer_fee() {
    let exit = env_with(HOOK, 0, Some(1_005_000_000)).invoke::<TypedViews>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, i64::from(ViewError::IssuerChargesFee));
}

#[test]
fn rejects_an_unfunded_issuer() {
    let env = TestEnv::new()
        .hook_account(HOOK)
        .otxn(iou_payment(Some(TAG)))
        .ledger_object(keylet_line(HOOK, ISSUER, USD), &trust_line_bytes(HOOK, 0));
    let exit = env.invoke::<TypedViews>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, i64::from(ViewError::NoIssuerAccount));
}

/// `Payment::otxn()` verifies the transaction type before any field is
/// read. `TestEnv::invoke` is a direct entry call with no `HookOn`
/// filtering, so the wrong type reaches the entry here even though a live
/// node's `on = [Payment]` would not have triggered it.
#[test]
fn rejects_a_transaction_that_is_not_a_payment() {
    let env = TestEnv::new()
        .hook_account(HOOK)
        .otxn(Otxn::new(TxType::EscrowCreate).account(SENDER));
    let exit = env.invoke::<TypedViews>(0);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert_eq!(exit.code, i64::from(ViewError::NotAPayment));
}
