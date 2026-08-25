#![cfg_attr(not(test), no_std)]

use rshooks::prelude::*;
use rshooks::*;

/// Backing storage for the writer: the fixed emit-plumbing prefix, up to
/// two `sfAmounts` entries (one native, one issued), and `EmitDetails`
/// headroom — see the README's "Buffer sizing" section for the byte
/// breakdown.
const BUF_LEN: usize = 285;
static BUF: HookStatic<[u8; BUF_LEN]> = HookStatic::new([0u8; BUF_LEN]);

/// Builds a Remit transaction into `buf` with `StoWriter`: the fixed
/// emit-plumbing fields, `destination`, one native-amount `sfAmounts`
/// entry always, and a second issued-amount entry only when `issued` is
/// `Some`. Field order here is chosen for readability, not because
/// `StoWriter` requires it — see `rshooks::sto_writer`'s module doc
/// comment.
fn build_remit<'a>(
    buf: &'a mut [u8; BUF_LEN],
    destination: &AccountId,
    issued: Option<&(CurrencyCode, AccountId)>,
) -> Result<StoWriter<'a>> {
    let mut w = StoWriter::new(buf);
    w.u16_field(sfTransactionType, rshooks::raw::tts::ttREMIT)?;
    w.u32_field(sfFlags, tfCANONICAL)?;
    w.u32_field(sfSequence, 0)?;
    w.u32_field(sfFirstLedgerSequence, 0)?;
    w.u32_field(sfLastLedgerSequence, 0)?;
    w.native_amount(sfFee, 0)?;
    w.empty_vl(sfSigningPubKey)?;
    w.account_id(sfAccount, &AccountId::default())?;
    w.account_id(sfDestination, destination)?;

    w.begin_array(sfAmounts)?;
    w.begin_object(sfAmountEntry)?;
    w.native_amount(sfAmount, 1)?;
    w.end_object()?;
    if let Some((currency, issuer)) = issued {
        w.begin_object(sfAmountEntry)?;
        w.iou_amount(sfAmount, XFL::one(), currency, issuer)?;
        w.end_object()?;
    }
    w.end_array()?;

    Ok(w)
}

hook_errors! {
    /// Errors returned by the `StoWriter`-based Remit emission.
    pub enum StoWriterError {
        /// An emission slot could not be reserved.
        ReserveFailed = 1,
        /// The `DEST` hook parameter was missing or not a 20-byte AccountID.
        MissingDestination = 2,
        /// The static build buffer had already been `take()`n.
        BufferAlreadyTaken = 3,
        /// A `StoWriter` write failed (out of space, bad nesting, or a
        /// duplicate write of a required field).
        BuildFailed = 4,
        /// `prepare_for_emit` failed to fill in the host-supplied fields.
        PrepareFailed = 5,
        /// The prepared transaction could not be emitted.
        EmitFailed = 6,
    }
}

#[hooks(description = "Emits a runtime-shaped Remit built with StoWriter.")]
pub struct StoWriterRemit;

#[hooks]
impl StoWriterRemit {
    /// Reserves one emission slot, reads `DEST` (required) and `CUR`/
    /// `ISSUER` (optional, together) from the hook parameters, builds a
    /// Remit transaction with `StoWriter` — a native-amount `sfAmounts`
    /// entry always, plus an issued-amount entry only when `CUR`/`ISSUER`
    /// are both present — and emits it.
    #[hook(0, name = "remit", on = [Invoke], can_emit = [Remit])]
    fn main(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"sto-writer: etxn_reserve failed",
                StoWriterError::ReserveFailed
            );
        }

        let Ok(destination) = hook_param_exact::<AccountId>(b"DEST") else {
            rollback!(
                b"sto-writer: missing DEST hook parameter",
                StoWriterError::MissingDestination
            )
        };

        let issued = match (
            hook_param_exact::<CurrencyCode>(b"CUR"),
            hook_param_exact::<AccountId>(b"ISSUER"),
        ) {
            (Ok(currency), Ok(issuer)) => Some((currency, issuer)),
            _ => None,
        };

        let Some(buf) = BUF.take() else {
            rollback!(
                b"sto-writer: static buffer already taken",
                StoWriterError::BufferAlreadyTaken
            );
        };

        let Ok(mut w) = build_remit(buf, &destination, issued.as_ref()) else {
            rollback!(b"sto-writer: build failed", StoWriterError::BuildFailed)
        };

        let Ok(prepared) = w.prepare_for_emit() else {
            rollback!(
                b"sto-writer: prepare_for_emit failed",
                StoWriterError::PrepareFailed
            )
        };

        match prepared.emit() {
            Ok(_hash) => accept!(b"sto-writer: emitted", 0),
            Err(_) => rollback!(b"sto-writer: emit failed", StoWriterError::EmitFailed),
        }
    }

    #[cbak(0)]
    fn cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

// In-crate off-chain unit test, driven through `TestEnv::invoke` against
// the entry declared above — no wasm build, no node. See `tests/remit.rs`
// for the equivalent integration-test-style layout, and
// `book/src/testing/unit-tests.md` for both layouts documented side by
// side.
//
// `build_remit`/`prepare_for_emit` are additionally exercised directly
// (not through `TestEnv::invoke`) via a small local `HostBackend`, for
// byte-level assertions on the prepared blob that `build_remit` being
// private keeps out of reach of `tests/remit.rs`'s integration-test form.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::rc::Rc;
    use std::vec::Vec;

    use rshooks_testenv::prelude::*;

    use super::{AccountId, CurrencyCode, HookError, StoWriterRemit, build_remit};

    const DEST: [u8; 20] = [3u8; 20];

    fn env() -> TestEnv {
        TestEnv::new()
            .hook_account([1u8; 20])
            .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
            .hook_param(b"DEST", &DEST)
    }

    #[test]
    fn accepts_and_emits_a_remit_through_the_real_entry() {
        let env = env();
        let exit = env.invoke::<StoWriterRemit>(0);
        assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
        let emitted = env.emitted();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
    }

    #[test]
    fn missing_destination_rolls_back() {
        let env = TestEnv::new()
            .hook_account([1u8; 20])
            .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]));
        let exit = env.invoke::<StoWriterRemit>(0);
        assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
        assert_eq!(env.emitted().len(), 0);
    }

    /// A minimal mock covering exactly what `prepare_for_emit`/`iou_amount`
    /// need — see `rshooks::sto_writer`'s own `testenv_tests` module for
    /// the identical pattern this mirrors.
    struct MockBackend;

    impl rshooks::raw::backend::HostBackend for MockBackend {
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
        fn float_sto(
            &self,
            _currency: Option<&[u8]>,
            _issuer: Option<&[u8]>,
            _amount: i64,
            _field_code: u32,
        ) -> core::result::Result<Vec<u8>, i64> {
            let mut out = std::vec![0x61u8]; // sfAmount (6,1) -> 1-byte header
            out.extend_from_slice(&[0u8; rshooks::types::IOU_AMOUNT_LEN]);
            Ok(out)
        }
        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("MockBackend::accept: not exercised by these tests")
        }
        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("MockBackend::rollback: not exercised by these tests")
        }
    }

    #[test]
    fn build_remit_prepares_successfully_native_only() {
        let _guard = rshooks::raw::backend::install(Rc::new(MockBackend));
        let mut buf = [0u8; super::BUF_LEN];
        let dest = AccountId([9u8; 20]);
        let mut w = build_remit(&mut buf, &dest, None).expect("fits");
        let prepared = w
            .prepare_for_emit()
            .expect("mock backend satisfies every requirement");
        let bytes = prepared.as_bytes();
        // One `sfAmounts` entry: exactly one ObjectEndMarker before the
        // ArrayEndMarker, `EmitDetails` appended after it.
        assert_eq!(bytes.iter().filter(|&&b| b == 0xE1).count(), 1);
        assert!(bytes.contains(&0xF1));
    }

    #[test]
    fn build_remit_prepares_successfully_with_issued_amount() {
        let _guard = rshooks::raw::backend::install(Rc::new(MockBackend));
        let mut buf = [0u8; super::BUF_LEN];
        let dest = AccountId([9u8; 20]);
        let issued = (CurrencyCode::default(), AccountId([7u8; 20]));
        let mut w = build_remit(&mut buf, &dest, Some(&issued)).expect("fits");
        let native_only_prepared_len = {
            let mut probe_buf = [0u8; super::BUF_LEN];
            let mut probe = build_remit(&mut probe_buf, &dest, None).expect("fits");
            probe
                .prepare_for_emit()
                .expect("mock backend satisfies every requirement")
                .as_bytes()
                .len()
        };
        let prepared = w
            .prepare_for_emit()
            .expect("mock backend satisfies every requirement");
        let bytes = prepared.as_bytes();
        // Two `sfAmounts` entries now: two ObjectEndMarkers, and a strictly
        // longer prepared blob than the native-only shape.
        assert_eq!(bytes.iter().filter(|&&b| b == 0xE1).count(), 2);
        assert!(bytes.len() > native_only_prepared_len);
    }

    #[test]
    fn iou_amount_write_reaches_the_issued_branch() {
        // Proves `build_remit` actually calls `iou_amount` (not silently
        // skipping it) even without a mock backend installed: the host
        // stub's deterministic `NOT_IMPLEMENTED` surfaces from exactly
        // that call.
        let mut buf = [0u8; super::BUF_LEN];
        let dest = AccountId([9u8; 20]);
        let issued = (CurrencyCode::default(), AccountId([7u8; 20]));
        assert_eq!(
            build_remit(&mut buf, &dest, Some(&issued)).err(),
            Some(HookError::NotImplemented)
        );
    }
}
