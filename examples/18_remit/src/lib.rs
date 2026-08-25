//! Emits a two-entry `Remit` (one native drop, one issued demo amount) to
//! the sender of the originating transaction, using `rshooks::RemitBuilder`
//! — no hand-written `sfAmounts`/`sfAmountEntry` container bytes, no
//! manually tracked `Fee`/`EmitDetails` offsets.
//!
//! Compare with `examples/10_emit-txn`, which uses `rshooks::txn_template!`
//! for a fixed-shape `Payment`: `Remit`'s `sfAmounts` is a *runtime-sized*
//! `STArray`, which a compile-time template cannot describe (see
//! `rshooks::sto_writer` and `rshooks::remit`'s module doc comments). The
//! byte buffer `RemitBuilder` writes into is a `static` (`REMIT_BUF`,
//! `HookStatic`) for the same reason `Payment` is a `static` in
//! `examples/10_emit-txn`: a wasm data segment / BSS buffer instead of a
//! runtime-materialized stack array — see that example's README.

#![cfg_attr(not(test), no_std)]

use rshooks::prelude::*;
use rshooks::*;

/// Room for the fixed prefix (`TransactionType` through `Destination`), the
/// `Amounts` array header, one native `sfAmountEntry` (12 bytes) and one
/// issued `sfAmountEntry` (52 bytes), the array terminator, and
/// `EMIT_DETAILS_MAX_LEN` bytes for `prepare_for_emit` to append
/// `sfEmitDetails` into — with headroom to spare.
const REMIT_BUF_LEN: usize = 320;

static REMIT_BUF: HookStatic<[u8; REMIT_BUF_LEN]> = HookStatic::new([0u8; REMIT_BUF_LEN]);

/// Demo issued-currency code: the standard 3-letter-code encoding of "USD"
/// (all-zero 160 bits except the ISO code at byte offset 12..15).
const USD: CurrencyCode = CurrencyCode({
    let mut bytes = [0u8; 20];
    bytes[12] = b'U';
    bytes[13] = b'S';
    bytes[14] = b'D';
    bytes
});

/// Demo issuer for [`USD`] — an arbitrary fixed `AccountId`, not a real
/// account; this example only demonstrates `RemitBuilder`'s wire encoding,
/// not trustline/issuer validity (out of scope — see `rshooks::remit`'s
/// module doc comment).
const ISSUER: AccountId = AccountId([0x02; ACC_ID_LEN]);

/// 10 USD, as a compile-time `XFL`.
const ISSUED_AMOUNT: XFL = XFL!(10);

hook_errors! {
    /// Errors returned by the Remit-emission hook.
    pub enum EmitRemitError {
        /// An emission slot could not be reserved.
        ReserveFailed = 1,
        /// The originating account could not be read.
        CouldNotReadSender = 2,
        /// [`REMIT_BUF`] had already been `take()`n.
        BufferAlreadyTaken = 3,
        /// `RemitBuilder::new` failed (the fixed prefix did not fit).
        BuildFailed = 4,
        /// A `push_native_amount`/`push_issued_amount` call failed.
        PushAmountFailed = 5,
        /// The Remit could not be prepared for emission (closing the
        /// `Amounts` array or appending `sfEmitDetails` failed).
        PrepareFailed = 6,
        /// The prepared Remit could not be emitted.
        EmitFailed = 7,
    }
}

#[hooks(description = "Emits a two-amount Remit and handles its callback.")]
pub struct EmitRemit;

#[hooks]
impl EmitRemit {
    #[hook(0, name = "remit", on = [Invoke], can_emit = [Remit])]
    fn main(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"emit-remit: etxn_reserve failed",
                EmitRemitError::ReserveFailed
            );
        }

        let Ok(destination) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"emit-remit: could not read otxn sender",
                EmitRemitError::CouldNotReadSender
            )
        };

        let Some(buf) = REMIT_BUF.take() else {
            rollback!(
                b"emit-remit: static buffer already taken",
                EmitRemitError::BufferAlreadyTaken
            );
        };

        let Ok(mut remit) = RemitBuilder::new(buf, &destination) else {
            rollback!(
                b"emit-remit: RemitBuilder::new failed",
                EmitRemitError::BuildFailed
            )
        };

        if remit.push_native_amount(1).is_err() {
            rollback!(
                b"emit-remit: push_native_amount failed",
                EmitRemitError::PushAmountFailed
            );
        }
        if remit
            .push_issued_amount(ISSUED_AMOUNT, &USD, &ISSUER)
            .is_err()
        {
            rollback!(
                b"emit-remit: push_issued_amount failed",
                EmitRemitError::PushAmountFailed
            );
        }

        let Ok(prepared) = remit.prepare_for_emit() else {
            rollback!(
                b"emit-remit: prepare_for_emit failed",
                EmitRemitError::PrepareFailed
            )
        };

        match prepared.emit() {
            Ok(_hash) => accept!(b"emit-remit: emitted", 0),
            Err(_) => rollback!(b"emit-remit: emit failed", EmitRemitError::EmitFailed),
        }
    }

    #[cbak(0)]
    fn cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

// In-crate off-chain unit test, driven through `TestEnv::invoke` against
// the entry declared above — no wasm build, no node. Only reachable via
// `cargo test` (`--test` implies `cfg(test)`, which is what switches off
// `no_std` above); never part of the shipped wasm artifact. See
// `tests/remit.rs` for the equivalent integration-test-style layout, and
// `examples/10_emit-txn/src/lib.rs` for the same two-layout pattern.
#[cfg(test)]
mod tests {
    use rshooks_testenv::prelude::*;

    use super::EmitRemit;

    fn env() -> TestEnv {
        TestEnv::new()
            .hook_account([1u8; 20])
            .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
    }

    #[test]
    fn emit_accepts_and_records_one_remit() {
        let env = env();
        let exit = env.invoke::<EmitRemit>(0);
        assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
        let emitted = env.emitted();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].tx_type(), Some(TxType::Remit));
        assert!(!emitted[0].blob().is_empty());
    }
}
