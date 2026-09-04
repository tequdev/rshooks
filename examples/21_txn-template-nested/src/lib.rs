#![cfg_attr(not(test), no_std)]

use rshooks::prelude::*;
use rshooks::*;

/// Baked currency/issuer default for every `sfAmounts` entry — compiled
/// into `AmountEntry::TEMPLATE`, so the hot path (`set_amount_value`) is a
/// single 8-byte store with no host call. `main` overrides the issuer at
/// runtime, through the full 48-byte `set_amount` setter, for every entry
/// when an `ISSUER` hook parameter is supplied.
const USD: CurrencyCode = CurrencyCode::from_iso(b"USD");
const USD_ISSUER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");

txn_template! {
    /// A Remit template whose `sfAmounts` array is a fixed, homogeneous
    /// `array(sfX) [ Elem: object(sfY) { .. } ; N ]` shape: every element
    /// has the identical declared shape (an issued `amount` with a baked
    /// `USD`/`USD_ISSUER` default), so the macro generates one
    /// `AmountEntry<'a>` view type — with the same `set_amount`/
    /// `set_amount_value` setters a plain field of that shape would get —
    /// plus a runtime-indexed `amounts(index) -> Option<AmountEntry<'_>>`
    /// accessor on `Remit`, instead of per-element setters. No
    /// `StoWriter` needed, since the element count and shape are both
    /// known at declaration time (contrast `examples/17_sto-writer`,
    /// whose second entry is only present conditionally at runtime).
    ///
    /// `sfMemos` is a single-element array of the same homogeneous form,
    /// with two `fixed_vl` fields: a length-`N` `VL` blob whose length
    /// (and so its rippled length prefix) is fixed at declaration time —
    /// `memo_type` bakes a `*b"note"` default (the whole field needs no
    /// runtime write at all), `memo_data` defaults to `N` zero bytes and
    /// is overwritten by `main`. `empty_vl` remains the one spelling for
    /// an *empty* blob, so `signing_pub_key` above stays `empty_vl`, not
    /// `fixed_vl(sfSigningPubKey, 0)` — the macro rejects `N = 0`.
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
        memos: array(sfMemos) [
            Memo: object(sfMemo) {
                memo_type: fixed_vl(sfMemoType, 4) = *b"note",
                memo_data: fixed_vl(sfMemoData, 8),
            }; 1
        ],
        amounts: array(sfAmounts) [
            AmountEntry: object(sfAmountEntry) {
                amount: amount(sfAmount) = (XFL::from_raw_bits(0), USD, USD_ISSUER),
            }; 2
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
        /// An `sfAmounts` index was out of range — unreachable by
        /// construction (both indexes are literals below the declared
        /// element count), kept only because the accessor returns
        /// `Option`.
        AmountsIndexOutOfRange = 4,
        /// `XFL::new` failed to normalize an entry's value.
        AmountValueFailed = 5,
        /// The `sfMemos` index was out of range — unreachable by
        /// construction (the index is a literal below the declared
        /// element count), kept only because the accessor returns
        /// `Option`.
        MemosIndexOutOfRange = 6,
        /// The Remit could not be prepared.
        PrepareFailed = 7,
        /// The prepared Remit could not be emitted.
        EmitFailed = 8,
    }
}

#[hooks(description = "Emits a Remit whose sfAmounts nesting is declared in txn_template!.")]
pub struct TxnTemplateNested {
    /// The Remit's destination account; required.
    #[hook_param(name = b"DEST", required)]
    dest: HookParam<AccountId>,
    /// An issuer overriding the baked `USD_ISSUER` for every `sfAmounts`
    /// entry; optional.
    #[hook_param(name = b"ISSUER")]
    issuer: HookParam<AccountId>,
}

#[hooks]
impl TxnTemplateNested {
    /// Reserves one emission slot, reads the required `DEST` hook
    /// parameter, fills both `sfAmounts` entries through
    /// `Remit::amounts`'s runtime-indexed accessor — entry 0 gets `1.0`,
    /// entry 1 gets `2.0`, via the baked `USD`/`USD_ISSUER` default unless
    /// an `ISSUER` hook parameter overrides the issuer for both entries,
    /// which routes through the full 48-byte `amount` setter instead of
    /// the 8-byte hot path — writes the single `sfMemos` entry's
    /// `memo_data` through `Remit::memos`'s accessor (`memo_type` stays at
    /// its baked default), and emits.
    #[hook(0, name = "tplremit", on = [Invoke], can_emit = [Remit])]
    fn main(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"txn-template-nested: etxn_reserve failed",
                TxnTemplateNestedError::ReserveFailed
            );
        }

        let Ok(destination) = self.hook_param.dest.get_required() else {
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

        let issuer: Option<AccountId> = self.hook_param.issuer.get().unwrap_or_default();

        // Both `sfAmounts` entries, indexed through `Remit::amounts`: entry
        // 0 carries 1.0, entry 1 carries 2.0, each written through the
        // baked-issuer 8-byte hot path, or the full 48-byte setter when
        // `ISSUER` overrides the issuer — exercised on the real wasm target
        // so a compiler-generated copy loop over the `[u8; 48]` region would
        // be caught by `rshooks build`/`check`.
        let Some(mut first) = txn.amounts(0) else {
            rollback!(
                b"txn-template-nested: amounts index out of range",
                TxnTemplateNestedError::AmountsIndexOutOfRange
            );
        };
        let Ok(one) = XFL::new(0, 1) else {
            rollback!(
                b"txn-template-nested: XFL::new failed",
                TxnTemplateNestedError::AmountValueFailed
            );
        };
        match issuer {
            Some(iss) => first.set_amount(one, &USD, &iss),
            None => first.set_amount_value(one),
        }

        let Some(mut second) = txn.amounts(1) else {
            rollback!(
                b"txn-template-nested: amounts index out of range",
                TxnTemplateNestedError::AmountsIndexOutOfRange
            );
        };
        let Ok(two) = XFL::new(0, 2) else {
            rollback!(
                b"txn-template-nested: XFL::new failed",
                TxnTemplateNestedError::AmountValueFailed
            );
        };
        match issuer {
            Some(iss) => second.set_amount(two, &USD, &iss),
            None => second.set_amount_value(two),
        }

        // The single `sfMemos` entry: `memo_type` stays at its baked
        // `*b"note"` default; only `memo_data` is written at runtime.
        let Some(mut memo) = txn.memos(0) else {
            rollback!(
                b"txn-template-nested: memos index out of range",
                TxnTemplateNestedError::MemosIndexOutOfRange
            );
        };
        memo.set_memo_data(b"rshooks!");

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
        sfLastLedgerSequence, sfMemo, sfMemoData, sfMemoType, sfMemos, sfSequence, sfSigningPubKey,
        sfTransactionType, tfCANONICAL,
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

    /// Byte-exact check of the declared `sfAmounts` region: an `sfAmounts`
    /// header around two `sfAmountEntry` images (`header(sfAmountEntry)
    /// 0x61 <8 value bytes> <USD> <USD_ISSUER> 0xE1` each), closed by the
    /// array end marker — element 0 holds `XFL::new(0, 1)` (`1.0`),
    /// element 1 holds `XFL::new(0, 2)` (`2.0`), both against the baked
    /// `USD`/`USD_ISSUER` default. Headers are derived via
    /// `txn::codec::field_header` rather than hardcoded, so this test
    /// tracks the codec's own header rule instead of duplicating it; the
    /// value bytes are hand-derived from the XFL bit layout the same way
    /// `rshooks/src/txn.rs`'s `encode_iou_amount_value_const_one` test
    /// documents for `1.0` (element 1 follows the identical normalization
    /// rule: mantissa `2_000_000_000_000_000`, exponent `-15`, positive).
    /// Goes through the real entry (`env().invoke`), so it also proves
    /// `XFL::new`'s real host normalization agrees with the hand
    /// derivation.
    #[test]
    fn amounts_region_matches_the_two_declared_issued_entries() {
        let env = env();
        let exit = env.invoke::<TxnTemplateNested>(0);
        assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
        let emitted = env.emitted();
        assert_eq!(emitted.len(), 1);
        let blob = emitted[0].blob();

        let (amounts_hdr, amounts_hdr_len) = rshooks::txn::codec::field_header(sfAmounts);
        let (entry_hdr, entry_hdr_len) = rshooks::txn::codec::field_header(sfAmountEntry);
        let (amount_hdr, amount_hdr_len) = rshooks::txn::codec::field_header(sfAmount);

        let mut currency = [0u8; 20];
        currency[12..15].copy_from_slice(b"USD");

        let mut expected = Vec::new();
        expected.extend_from_slice(&amounts_hdr[..amounts_hdr_len]);
        for value in [
            [0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00], // XFL::new(0, 1) == 1.0
            [0xD4, 0x87, 0x1A, 0xFD, 0x49, 0x8D, 0x00, 0x00], // XFL::new(0, 2) == 2.0
        ] {
            expected.extend_from_slice(&entry_hdr[..entry_hdr_len]);
            expected.extend_from_slice(&amount_hdr[..amount_hdr_len]);
            expected.extend_from_slice(&value);
            expected.extend_from_slice(&currency);
            expected.extend_from_slice(&USD_ISSUER.0);
            expected.push(0xE1); // object end marker
        }
        expected.push(0xF1); // array end marker

        assert!(
            blob.windows(expected.len())
                .any(|w| w == expected.as_slice()),
            "sfAmounts region not found in the emitted blob: {blob:02x?}"
        );
    }

    /// Byte-exact check of the declared `sfMemos` region:
    /// `header(sfMemos) header(sfMemo) header(sfMemoType) 0x04 "note"
    /// header(sfMemoData) 0x08 "rshooks!" 0xE1 0xF1` — `memo_type` at its
    /// baked default, `memo_data` written by `main`. `fixed_vl`'s VL
    /// length prefix (`0x04`/`0x08`) is a single byte since both lengths
    /// are `<= 192`; headers are derived via `txn::codec::field_header`
    /// rather than hardcoded.
    #[test]
    fn memos_region_matches_the_declared_memo() {
        let env = env();
        let exit = env.invoke::<TxnTemplateNested>(0);
        assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
        let emitted = env.emitted();
        assert_eq!(emitted.len(), 1);
        let blob = emitted[0].blob();

        let (memos_hdr, memos_hdr_len) = rshooks::txn::codec::field_header(sfMemos);
        let (memo_hdr, memo_hdr_len) = rshooks::txn::codec::field_header(sfMemo);
        let (type_hdr, type_hdr_len) = rshooks::txn::codec::field_header(sfMemoType);
        let (data_hdr, data_hdr_len) = rshooks::txn::codec::field_header(sfMemoData);

        let mut expected = Vec::new();
        expected.extend_from_slice(&memos_hdr[..memos_hdr_len]);
        expected.extend_from_slice(&memo_hdr[..memo_hdr_len]);
        expected.extend_from_slice(&type_hdr[..type_hdr_len]);
        expected.push(4); // fixed_vl(sfMemoType, 4)'s length prefix
        expected.extend_from_slice(b"note");
        expected.extend_from_slice(&data_hdr[..data_hdr_len]);
        expected.push(8); // fixed_vl(sfMemoData, 8)'s length prefix
        expected.extend_from_slice(b"rshooks!");
        expected.push(0xE1); // object end marker
        expected.push(0xF1); // array end marker

        assert!(
            blob.windows(expected.len())
                .any(|w| w == expected.as_slice()),
            "sfMemos region not found in the emitted blob: {blob:02x?}"
        );
    }

    /// `main`'s optional `ISSUER` hook parameter routes through the full
    /// 48-byte `set_amount` setter (rather than the 8-byte `_value` hot
    /// path) and overrides the issuer on **every** `sfAmounts` entry — the
    /// currency stays the baked `USD` on both.
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
        let mut currency = [0u8; 20];
        currency[12..15].copy_from_slice(b"USD");

        for value in [
            [0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00], // XFL::new(0, 1) == 1.0
            [0xD4, 0x87, 0x1A, 0xFD, 0x49, 0x8D, 0x00, 0x00], // XFL::new(0, 2) == 2.0
        ] {
            let mut expected = Vec::new();
            expected.extend_from_slice(&entry_hdr[..entry_hdr_len]);
            expected.extend_from_slice(&amount_hdr[..amount_hdr_len]);
            expected.extend_from_slice(&value);
            expected.extend_from_slice(&currency);
            expected.extend_from_slice(&[4u8; 20]); // the overridden issuer
            expected.push(0xE1); // object end marker
            assert!(
                blob.windows(expected.len())
                    .any(|w| w == expected.as_slice()),
                "issued sfAmountEntry with the overridden issuer not found: {blob:02x?}"
            );
        }
    }

    /// A hand-written [`rshooks::raw::backend::HostBackend`] whose
    /// `float_sto` assembles `STAmount`'s issued-amount wire encoding
    /// component by component (sign, biased exponent, mantissa) from the
    /// byte layout xahaud's own `float_sto` writes — not from the bit-OR
    /// identity `txn::codec` relies on — so installing it under
    /// [`StoWriter::iou_amount`] exercises a second, hand-derived encoder
    /// rather than the one `txn_template!`'s setters call. `float_set`
    /// similarly reimplements XFL's integer-mantissa normalization from
    /// its own invariant (a canonical mantissa always has exactly 16
    /// significant digits), scoped to the small positive integers this
    /// test passes — not a call into `rshooks-testenv`'s private
    /// `host::float` module.
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
        // cites) — needed so `XFL::one()` resolves to a real value instead
        // of the host stub's error code.
        fn float_one(&self) -> i64 {
            6_089_866_696_204_910_592
        }
        fn float_set(&self, exponent: i32, mantissa: i64) -> i64 {
            if mantissa <= 0 {
                return -1; // not exercised by this test
            }
            let mut m = mantissa as u64;
            let mut e = exponent;
            while m < 1_000_000_000_000_000 {
                m *= 10;
                e -= 1;
            }
            while m >= 10_000_000_000_000_000 {
                m /= 10;
                e += 1;
            }
            let bits = (1u64 << 62) | (((e + 97) as u64 & 0xFF) << 54) | (m & ((1u64 << 54) - 1));
            bits as i64
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
    /// `EmitDetails` region), with both `sfAmounts` entries issued
    /// (`XFL::new(0, 1)`, `XFL::new(0, 2)`), matching `Remit`'s
    /// homogeneous two-element shape.
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
        w.begin_array(sfMemos).expect("fits");
        w.begin_object(sfMemo).expect("fits");
        w.vl(sfMemoType, b"note").expect("fits");
        w.vl(sfMemoData, b"rshooks!").expect("fits");
        w.end_object().expect("fits");
        w.end_array().expect("fits");
        w.begin_array(sfAmounts).expect("fits");
        for i in 0..2i64 {
            let value = XFL::new(0, i + 1).expect("normalizes");
            w.begin_object(sfAmountEntry).expect("fits");
            w.iou_amount(sfAmount, value, &USD, &USD_ISSUER)
                .expect("fits");
            w.end_object().expect("fits");
        }
        w.end_array().expect("fits");

        let mut tpl = Remit::new();
        tpl.set_destination(&AccountId::default());
        for i in 0..2usize {
            let value = XFL::new(0, i as i64 + 1).expect("normalizes");
            let mut entry = tpl.amounts(i).expect("index in range");
            entry.set_amount_value(value);
        }
        let mut memo = tpl.memos(0).expect("index in range");
        memo.set_memo_data(b"rshooks!");

        assert_eq!(w.as_bytes(), &tpl.bytes()[..PREFIX_LEN]);
    }
}
