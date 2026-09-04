#![cfg_attr(not(test), no_std)]

use rshooks::prelude::*;
use rshooks::*;

/// The issued entry's baked currency and issuer — compiled into the
/// template's default bytes, so the hot path (`set_amounts_usd_amount_value`)
/// is a single 8-byte store with no host call. `main` overrides the issuer
/// at runtime, through the full 48-byte `set_amounts_usd_amount` setter,
/// when an `ISSUER` hook parameter is supplied.
const USD: CurrencyCode = CurrencyCode::from_iso(b"USD");
const USD_ISSUER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");

txn_template! {
    /// A Remit template whose `sfAmounts` array is a fixed, two-entry
    /// nested shape declared entirely in `txn_template!`: one
    /// `native_amount` entry and one `amount` entry with a baked
    /// currency/issuer default — no `StoWriter` needed, since both the
    /// element count and every element's shape are known at declaration
    /// time (contrast `examples/17_sto-writer`, whose second entry is only
    /// present conditionally at runtime).
    struct Remit {
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
                amount: amount(sfAmount) = (XFL::from_raw_bits(0), USD, USD_ISSUER),
            },
        ],
        emit_details: emit_details,
    }
}

/// The reusable Remit template.
static TXN: HookStatic<Remit> = HookStatic::new(Remit::new());

hook_errors! {
    /// Errors returned by the emission hook.
    pub enum TxnTemplateNestedError {
        /// An emission slot could not be reserved.
        ReserveFailed = 1,
        /// The `DEST` hook parameter was missing or not a 20-byte AccountID.
        MissingDestination = 2,
        /// The Remit template was unavailable.
        BufferAlreadyTaken = 3,
        /// The native-amount entry's setter failed.
        SetAmountFailed = 4,
        /// The Remit could not be prepared.
        PrepareFailed = 5,
        /// The prepared Remit could not be emitted.
        EmitFailed = 6,
    }
}

#[hooks(description = "Emits a Remit whose sfAmounts nesting is declared in txn_template!.")]
pub struct TxnTemplateNested;

#[hooks]
impl TxnTemplateNested {
    /// Reserves one emission slot, reads the required `DEST` hook
    /// parameter (a 20-byte `AccountId`), fills in the two `sfAmounts`
    /// entries — the issued entry's currency/issuer stay at their baked
    /// default unless an `ISSUER` hook parameter overrides the issuer,
    /// which routes through the full 48-byte `amount` setter instead of
    /// the 8-byte hot path — and emits.
    #[hook(0, name = "tplremit", on = [Invoke], can_emit = [Remit])]
    fn main(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"txn-template-nested: etxn_reserve failed",
                TxnTemplateNestedError::ReserveFailed
            );
        }

        let Ok(destination) = hook_param_exact::<AccountId>(b"DEST") else {
            rollback!(
                b"txn-template-nested: missing DEST hook parameter",
                TxnTemplateNestedError::MissingDestination
            )
        };

        let Some(txn) = TXN.take() else {
            rollback!(
                b"txn-template-nested: static buffer already taken",
                TxnTemplateNestedError::BufferAlreadyTaken
            );
        };

        txn.set_destination(&destination);
        if txn.set_amounts_native_amount(1).is_err() {
            rollback!(
                b"txn-template-nested: native amount setter failed",
                TxnTemplateNestedError::SetAmountFailed
            );
        }
        // Baked `USD`/`USD_ISSUER`: a single 8-byte store. An optional
        // `ISSUER` hook parameter overrides the issuer at runtime instead,
        // which needs the full 48-byte setter — exercised on the real wasm
        // target so a future compiler-generated copy loop over the
        // `[u8; 48]` region would be caught by `rshooks build`/`check`.
        match hook_param_exact::<AccountId>(b"ISSUER") {
            Ok(issuer) => txn.set_amounts_usd_amount(XFL::one(), &USD, &issuer),
            Err(_) => txn.set_amounts_usd_amount_value(XFL::one()),
        }

        let Ok(prepared) = txn.prepare_for_emit() else {
            rollback!(
                b"txn-template-nested: prepare_for_emit failed",
                TxnTemplateNestedError::PrepareFailed
            )
        };

        match prepared.emit() {
            Ok(_hash) => accept!(b"txn-template-nested: emitted", 0),
            Err(_) => rollback!(
                b"txn-template-nested: emit failed",
                TxnTemplateNestedError::EmitFailed
            ),
        }
    }

    #[cbak(0)]
    fn cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

// In-crate off-chain unit test, driven through `TestEnv::invoke` against
// the entry declared above — no wasm build, no node. See `tests/remit.rs`
// for the equivalent integration-test-style layout.
//
// `Remit` and `StoWriter` are additionally exercised directly, for
// byte-level assertions: `Remit` is private, so only reachable from an
// in-crate test.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::rc::Rc;
    use std::vec::Vec;

    use rshooks_testenv::prelude::*;

    use super::{
        AccountId, Remit, TxnTemplateNested, USD, USD_ISSUER, XFL, sfAccount, sfAmount,
        sfAmountEntry, sfAmounts, sfDestination, sfFee, sfFirstLedgerSequence, sfFlags,
        sfLastLedgerSequence, sfSequence, sfSigningPubKey, sfTransactionType, tfCANONICAL,
    };

    const DEST: [u8; 20] = [3u8; 20];

    fn env() -> TestEnv {
        TestEnv::new()
            .hook_account([1u8; 20])
            .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
            .hook_param(b"DEST", &DEST)
    }

    #[test]
    fn accepts_and_emits_one_remit() {
        let env = env();
        let exit = env.invoke::<TxnTemplateNested>(0);
        assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
        let emitted = env.emitted();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
    }

    #[test]
    fn missing_destination_rolls_back_without_emitting() {
        let env = TestEnv::new()
            .hook_account([1u8; 20])
            .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]));
        let exit = env.invoke::<TxnTemplateNested>(0);
        assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
        assert_eq!(env.emitted().len(), 0);
    }

    /// Byte-exact check of the declared `sfAmounts` region: an
    /// `sfAmounts`/`sfAmountEntry` header pair around a native 1-drop
    /// entry, then another `sfAmountEntry` around an issued entry holding
    /// `XFL::one()`'s value bytes plus the baked `USD`/`USD_ISSUER`
    /// currency and issuer, closed by the object and array end markers.
    /// Headers are derived via `txn::codec::field_header` rather than
    /// hardcoded, so this test tracks the codec's own header rule instead
    /// of duplicating it.
    #[test]
    fn amounts_region_matches_the_declared_native_and_issued_entries() {
        // `XFL::one()` makes a `float_one` host call, which needs a
        // backend installed under the `testenv` feature; this test only
        // checks byte layout, so it uses the same canonical XFL 1.0 bit
        // pattern `rshooks/src/txn.rs`'s own
        // `encode_iou_amount_value_const_one` test cites, rather than
        // installing one.
        let xfl_one = XFL::from_raw_bits(6_089_866_696_204_910_592);

        let mut tpl = Remit::new();
        tpl.set_amounts_native_amount(1)
            .expect("1 drop is in range");
        tpl.set_amounts_usd_amount_value(xfl_one);

        let (amounts_hdr, amounts_hdr_len) = rshooks::txn::codec::field_header(sfAmounts);
        let (entry_hdr, entry_hdr_len) = rshooks::txn::codec::field_header(sfAmountEntry);
        let (amount_hdr, amount_hdr_len) = rshooks::txn::codec::field_header(sfAmount);

        let mut expected = Vec::new();
        expected.extend_from_slice(&amounts_hdr[..amounts_hdr_len]);
        expected.extend_from_slice(&entry_hdr[..entry_hdr_len]);
        expected.extend_from_slice(&amount_hdr[..amount_hdr_len]);
        expected.extend_from_slice(&[0x40, 0, 0, 0, 0, 0, 0, 1]); // native 1 drop
        expected.push(0xE1); // object end marker
        expected.extend_from_slice(&entry_hdr[..entry_hdr_len]);
        expected.extend_from_slice(&amount_hdr[..amount_hdr_len]);
        // XFL::one()'s issued STAmount value bytes (rshooks/src/txn.rs's
        // `encode_iou_amount_value_const_one` test derives the same
        // vector by hand from the XFL bit layout).
        expected.extend_from_slice(&[0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]);
        let mut currency = [0u8; 20];
        currency[12..15].copy_from_slice(b"USD");
        expected.extend_from_slice(&currency);
        expected.extend_from_slice(&USD_ISSUER.0);
        expected.push(0xE1); // object end marker
        expected.push(0xF1); // array end marker

        let bytes = tpl.bytes();
        let start = bytes
            .windows(expected.len())
            .position(|w| w == expected.as_slice())
            .expect("sfAmounts region not found in the template's bytes");
        assert_eq!(&bytes[start..start + expected.len()], expected.as_slice());
    }

    /// `main`'s optional `ISSUER` hook parameter routes through the full
    /// 48-byte `set_amounts_usd_amount` setter (rather than the 8-byte
    /// `_value` hot path) and overrides only the issuer — the currency
    /// stays the baked `USD`.
    #[test]
    fn issuer_hook_param_overrides_the_baked_issuer_via_the_full_setter() {
        let env = env().hook_param(b"ISSUER", &[4u8; 20]);
        let exit = env.invoke::<TxnTemplateNested>(0);
        assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
        let emitted = env.emitted();
        assert_eq!(emitted.len(), 1);
        let blob = emitted[0].blob();

        let (entry_hdr, entry_hdr_len) = rshooks::txn::codec::field_header(sfAmountEntry);
        let (amount_hdr, amount_hdr_len) = rshooks::txn::codec::field_header(sfAmount);
        let mut expected = Vec::new();
        expected.extend_from_slice(&entry_hdr[..entry_hdr_len]);
        expected.extend_from_slice(&amount_hdr[..amount_hdr_len]);
        // XFL::one()'s issued STAmount value bytes (see the byte-exact
        // `sfAmounts` test above).
        expected.extend_from_slice(&[0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]);
        let mut currency = [0u8; 20];
        currency[12..15].copy_from_slice(b"USD");
        expected.extend_from_slice(&currency);
        expected.extend_from_slice(&[4u8; 20]); // the overridden issuer
        expected.push(0xE1); // object end marker
        assert!(
            blob.windows(expected.len())
                .any(|w| w == expected.as_slice()),
            "issued sfAmountEntry with the overridden issuer not found: {blob:02x?}"
        );
    }

    /// A hand-written [`rshooks::raw::backend::HostBackend`] whose
    /// `float_sto` assembles `STAmount`'s issued-amount wire encoding
    /// component by component (sign, biased exponent, mantissa) from the
    /// byte layout xahaud's own `float_sto` writes — not from the bit-OR
    /// identity `txn::codec` relies on — so installing it under
    /// [`StoWriter::iou_amount`] exercises a second, hand-derived encoder
    /// rather than the one `txn_template!`'s setters call.
    struct RealFloatSto;

    impl rshooks::raw::backend::HostBackend for RealFloatSto {
        fn hook_account(&self) -> core::result::Result<[u8; 20], i64> {
            Ok([1u8; 20])
        }
        fn ledger_seq(&self) -> i64 {
            1000
        }
        fn etxn_fee_base(&self, _tx_blob: &[u8]) -> i64 {
            10
        }
        fn etxn_details(&self) -> core::result::Result<Vec<u8>, i64> {
            Ok(std::vec![0u8; 116])
        }
        // Canonical XFL 1.0 bit pattern (same reference value
        // `rshooks/src/txn.rs`'s `encode_iou_amount_value_const_one` test
        // cites) — needed so `XFL::one()` (called both by
        // `StoWriter::iou_amount` below and by `set_amounts_usd_amount_value`)
        // resolves to a real value instead of the host stub's error code.
        fn float_one(&self) -> i64 {
            6_089_866_696_204_910_592
        }
        fn float_sto(
            &self,
            currency: Option<&[u8]>,
            issuer: Option<&[u8]>,
            amount: i64,
            _field_code: u32,
        ) -> core::result::Result<Vec<u8>, i64> {
            // Decompose the XFL into the components the Hook API's
            // `float_mantissa`/`float_exponent`/`float_sign` expose, then
            // assemble `STAmount`'s issued value byte by byte from its own
            // layout rules (bit 63 set, bit 62 = positive, bits 54..=61 =
            // exponent + 97, bits 0..=53 = mantissa, canonical zero =
            // `0x80 00 .. 00`) — the same construction xahaud's `float_sto`
            // performs, rather than the bit-OR identity `txn::codec` relies on.
            let bits = amount as u64;
            let mantissa = bits & ((1u64 << 54) - 1);
            let exponent_biased = ((bits >> 54) & 0xFF) as u8;
            let negative = bits & (1u64 << 62) == 0;
            let mut value = [0u8; 8];
            if mantissa == 0 {
                value[0] = 0b1000_0000;
            } else {
                value[0] =
                    (if negative { 0b1000_0000 } else { 0b1100_0000 }) | (exponent_biased >> 2);
                value[1] = ((exponent_biased & 0b11) << 6) | ((mantissa >> 48) & 0x3F) as u8;
                value[2] = ((mantissa >> 40) & 0xFF) as u8;
                value[3] = ((mantissa >> 32) & 0xFF) as u8;
                value[4] = ((mantissa >> 24) & 0xFF) as u8;
                value[5] = ((mantissa >> 16) & 0xFF) as u8;
                value[6] = ((mantissa >> 8) & 0xFF) as u8;
                value[7] = (mantissa & 0xFF) as u8;
            }
            let mut out = std::vec![0x61u8]; // sfAmount (6,1): one-byte header
            out.extend_from_slice(&value);
            out.extend_from_slice(currency.unwrap_or(&[0u8; 20]));
            out.extend_from_slice(issuer.unwrap_or(&[0u8; 20]));
            Ok(out)
        }
        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("RealFloatSto::accept: not exercised by this test")
        }
        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("RealFloatSto::rollback: not exercised by this test")
        }
    }

    /// Cross-checks `txn_template!`'s local, `const fn` XFL -> `STAmount`
    /// encoding against [`rshooks::sto_writer::StoWriter::iou_amount`]
    /// (which goes through `float_sto`) by building the identical
    /// fixed-prefix bytes both ways, over exactly `Remit`'s fixed prefix
    /// (`Remit::LEN - EMIT_DETAILS_MAX_LEN` bytes — everything before the
    /// `EmitDetails` region).
    #[test]
    fn matches_sto_writer_bytes_for_the_same_fixed_prefix() {
        const PREFIX_LEN: usize = Remit::LEN - rshooks::types::EMIT_DETAILS_MAX_LEN;

        let _guard = rshooks::raw::backend::install(Rc::new(RealFloatSto));
        let mut buf = [0u8; PREFIX_LEN];
        let mut w = rshooks::sto_writer::StoWriter::new(&mut buf);
        w.u16_field(sfTransactionType, rshooks::raw::tts::ttREMIT)
            .expect("fits");
        w.u32_field(sfFlags, tfCANONICAL).expect("fits");
        w.u32_field(sfSequence, 0).expect("fits");
        w.u32_field(sfFirstLedgerSequence, 0).expect("fits");
        w.u32_field(sfLastLedgerSequence, 0).expect("fits");
        w.native_amount(sfFee, 0).expect("fits");
        w.empty_vl(sfSigningPubKey).expect("fits");
        w.account_id(sfAccount, &AccountId::default())
            .expect("fits");
        w.account_id(sfDestination, &AccountId::default())
            .expect("fits");
        w.begin_array(sfAmounts).expect("fits");
        w.begin_object(sfAmountEntry).expect("fits");
        w.native_amount(sfAmount, 1).expect("fits");
        w.end_object().expect("fits");
        w.begin_object(sfAmountEntry).expect("fits");
        w.iou_amount(sfAmount, XFL::one(), &USD, &USD_ISSUER)
            .expect("fits");
        w.end_object().expect("fits");
        w.end_array().expect("fits");

        let mut tpl = Remit::new();
        tpl.set_destination(&AccountId::default());
        tpl.set_amounts_native_amount(1)
            .expect("1 drop is in range");
        tpl.set_amounts_usd_amount_value(XFL::one());

        assert_eq!(w.as_bytes(), &tpl.bytes()[..PREFIX_LEN]);
    }
}
