#![cfg_attr(not(test), no_std)]

use rshooks::prelude::*;
use rshooks::*;

/// Baked currency for the issued form of either `amounts` entry; the
/// issuer comes from the `ISSUER` hook parameter at runtime.
const USD: CurrencyCode = CurrencyCode::from_iso(b"USD");

txn_template! {
    /// A Remit that sends one or two amounts, with an `optional`
    /// `DestinationTag` — written entirely in the inferred style: every
    /// field infers its kind from its own `sfXxx` constant, explicit only
    /// where a kind cannot infer (`any_amount`, via the `= AnyAmount()`
    /// default-shape marker).
    ///
    /// Field order (canonical `(type, field)`): `sequence` (2,4) <
    /// `destination_tag` (2,14) < `first_ledger_sequence` (2,26) <
    /// `last_ledger_sequence` (2,27) < `fee` (6,8) < `signing_pub_key`
    /// (7,3) < `account` (8,1) < `destination` (8,3) < `amounts` (15,92).
    struct Remit {
        transaction_type = ttREMIT,
        sequence: sfSequence = 0,
        destination_tag: optional sfDestinationTag,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        fee: sfFee = NativeAmount(0),
        signing_pub_key: sfSigningPubKey = [],
        account: sfAccount,
        destination: sfDestination,
        amounts: sfAmounts [
            first: sfAmountEntry { amount: sfAmount = AnyAmount() },
            second: optional sfAmountEntry { amount: sfAmount = AnyAmount() },
        ],
        emit_details: emit_details,
    }
}

/// The reusable `Remit` template.
static REMIT_TXN: HookStatic<Remit> = HookStatic::new(Remit::new());

hook_errors! {
    /// Errors returned by the `remit` entry.
    pub enum RemitError {
        /// An emission slot could not be reserved.
        ReserveFailed = 1,
        /// The `DEST` hook parameter was missing or not a 20-byte AccountID.
        MissingDestination = 2,
        /// The reusable template buffer was unavailable.
        BufferAlreadyTaken = 3,
        /// `AMT1`/`AMT2` was explicitly `0` -- Remit rejects a zero amount.
        ZeroAmount = 4,
        /// An `any_amount` value was out of range.
        AmountFailed = 5,
        /// The template could not be prepared.
        PrepareFailed = 6,
        /// The prepared transaction could not be emitted.
        EmitFailed = 7,
    }
}

#[hooks(description = "Emits a Remit sending one or two amounts, with an optional DestinationTag.")]
pub struct TxnTemplateOptional {
    /// The destination account; required.
    #[hook_param(name = b"DEST", required)]
    dest: HookParam<AccountId>,
    /// `destination_tag`, if present.
    #[hook_param(name = b"DTAG")]
    dest_tag: HookParam<[u8; 4]>,
    /// `amounts.first`'s value in drops; defaults to `1` if absent, must
    /// not be `0` if present.
    #[hook_param(name = b"AMT1")]
    amt1: HookParam<[u8; 8]>,
    /// `amounts.second`'s value in drops, if present (absent leaves
    /// `second` unwritten); must not be `0` if present.
    #[hook_param(name = b"AMT2")]
    amt2: HookParam<[u8; 8]>,
    /// When present, both `amounts.first` and (if enabled) `.second` are
    /// written in the issued form (`USD`, this account as issuer) instead
    /// of the native form.
    #[hook_param(name = b"ISSUER")]
    issuer: HookParam<AccountId>,
}

#[hooks]
impl TxnTemplateOptional {
    /// Reserves one emission slot, reads the required `DEST` hook
    /// parameter, fills `Remit`'s optional fields from the remaining
    /// (all optional) hook parameters, and emits.
    #[hook(0, name = "remit", on = [Invoke], can_emit = [Remit])]
    fn remit(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"txn-template-optional: etxn_reserve failed",
                RemitError::ReserveFailed
            );
        }

        let Ok(destination) = self.hook_param.dest.get_required() else {
            rollback!(
                b"txn-template-optional: missing DEST hook parameter",
                RemitError::MissingDestination
            )
        };

        let Some(txn) = REMIT_TXN.take() else {
            rollback!(
                b"txn-template-optional: remit buffer already taken",
                RemitError::BufferAlreadyTaken
            );
        };

        txn.set_destination(&destination);

        if let Ok(Some(tag)) = self.hook_param.dest_tag.get() {
            txn.set_destination_tag(u32::from_be_bytes(tag));
        }

        let issuer = match self.hook_param.issuer.get() {
            Ok(Some(iss)) => Some(iss),
            _ => None,
        };

        let amt1 = match self.hook_param.amt1.get() {
            Ok(Some(bytes)) => u64::from_be_bytes(bytes),
            _ => 1,
        };
        if amt1 == 0 {
            rollback!(
                b"txn-template-optional: AMT1 must not be zero",
                RemitError::ZeroAmount
            );
        }
        match &issuer {
            Some(iss) => {
                let Ok(xfl) = XFL::new(0, amt1 as i64) else {
                    rollback!(
                        b"txn-template-optional: XFL::new failed for AMT1",
                        RemitError::AmountFailed
                    );
                };
                txn.set_amounts_first_amount_issued(xfl, &USD, iss);
            }
            None => {
                if txn.set_amounts_first_amount_native(amt1).is_err() {
                    rollback!(
                        b"txn-template-optional: AMT1 native amount out of range",
                        RemitError::AmountFailed
                    );
                }
            }
        }

        if let Ok(Some(bytes)) = self.hook_param.amt2.get() {
            let amt2 = u64::from_be_bytes(bytes);
            if amt2 == 0 {
                rollback!(
                    b"txn-template-optional: AMT2 must not be zero",
                    RemitError::ZeroAmount
                );
            }
            match &issuer {
                Some(iss) => {
                    let Ok(xfl) = XFL::new(0, amt2 as i64) else {
                        rollback!(
                            b"txn-template-optional: XFL::new failed for AMT2",
                            RemitError::AmountFailed
                        );
                    };
                    txn.set_amounts_second_amount_issued(xfl, &USD, iss);
                }
                None => {
                    if txn.set_amounts_second_amount_native(amt2).is_err() {
                        rollback!(
                            b"txn-template-optional: AMT2 native amount out of range",
                            RemitError::AmountFailed
                        );
                    }
                }
            }
        }

        let Ok(prepared) = txn.prepare_for_emit() else {
            rollback!(
                b"txn-template-optional: prepare_for_emit failed",
                RemitError::PrepareFailed
            )
        };

        match prepared.emit() {
            Ok(_hash) => accept!(b"txn-template-optional: remit emitted", 0),
            Err(_) => rollback!(
                b"txn-template-optional: emit failed",
                RemitError::EmitFailed
            ),
        }
    }

    #[cbak(0)]
    fn remit_cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

// Off-chain unit tests exercising the private `Remit` type directly -- no
// host backend needed, since none of these calls reach
// `prepare_for_emit`/`emit` (only `HookParam`-free setters on a
// freshly-constructed template) -- for byte-level NOP-padding assertions
// only reachable from an in-crate test (`Remit` is private). `tests/remit.rs`
// covers the through-`TestEnv` functional/decoded-value side, against
// `emitted()`'s canonical (NOP-free) bytes.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]
mod tests {
    use super::*;
    use rshooks::txn::codec;

    /// `destination_tag` (`optional sfDestinationTag`) defaults absent --
    /// its whole `header + value` slot, header included, is `NOP`s -- and,
    /// once set, carries the header and big-endian value at that slot;
    /// `clear_destination_tag()` restores the `NOP`s.
    #[test]
    fn destination_tag_defaults_absent_and_round_trips() {
        let mut txn = Remit::new();
        let (hdr, hdr_len) = codec::field_header(sfDestinationTag);

        txn.set_destination_tag(7);
        let bytes = txn.bytes();
        let start = bytes
            .windows(hdr_len)
            .position(|w| w == &hdr[..hdr_len])
            .expect("sfDestinationTag header present once set");
        assert_eq!(&bytes[start..start + hdr_len], &hdr[..hdr_len]);
        assert_eq!(
            &bytes[start + hdr_len..start + hdr_len + 4],
            &7u32.to_be_bytes()
        );

        txn.clear_destination_tag();
        let bytes = txn.bytes();
        assert_eq!(&bytes[start..start + hdr_len + 4], &[codec::NOP; 9][..5]);
    }

    /// `amounts.first` (`amount: sfAmount = AnyAmount()`, always present)
    /// leaves 40 trailing `NOP` bytes after the 8-byte value in its native
    /// form; `amounts.second` (`optional sfAmountEntry { .. }`, no view
    /// type -- its own `amount` field is a plain `set_amounts_second_
    /// amount_native`/`_issued` pair on `Remit` itself) defaults absent
    /// (the whole element, header included, is `NOP`s) and, once its
    /// setter is called, carries the same native-form shape at the same
    /// offset; `clear_amounts_second()` restores the `NOP`s.
    #[test]
    fn first_leaves_nop_tail_second_defaults_absent_and_enables() {
        let mut txn = Remit::new();
        let (entry_hdr, entry_hdr_len) = codec::field_header(sfAmountEntry);
        let (amount_hdr, amount_hdr_len) = codec::field_header(sfAmount);
        // header + amount header + 48-byte value region + object end marker.
        let region_len = entry_hdr_len + amount_hdr_len + 48 + 1;

        let bytes = txn.bytes();
        let first_start = bytes
            .windows(entry_hdr_len)
            .position(|w| w == &entry_hdr[..entry_hdr_len])
            .expect("sfAmountEntry header present for `first`");
        let second_start = first_start + region_len;
        assert_eq!(
            &bytes[second_start..second_start + region_len],
            &[codec::NOP; 128][..region_len],
            "`second` defaults absent"
        );

        txn.set_amounts_first_amount_native(5)
            .expect("5 drops is in range");
        let bytes = txn.bytes();
        assert_eq!(
            &bytes[first_start + entry_hdr_len..first_start + entry_hdr_len + amount_hdr_len],
            &amount_hdr[..amount_hdr_len]
        );
        let mut native_value = 5u64.to_be_bytes();
        native_value[0] |= 0x40;
        assert_eq!(
            &bytes[first_start + entry_hdr_len + amount_hdr_len
                ..first_start + entry_hdr_len + amount_hdr_len + 8],
            &native_value
        );
        assert_eq!(
            &bytes[first_start + entry_hdr_len + amount_hdr_len + 8
                ..first_start + entry_hdr_len + amount_hdr_len + 48],
            &[codec::NOP; codec::ANY_AMOUNT_NATIVE_NOPS]
        );

        txn.set_amounts_second_amount_native(9)
            .expect("9 drops is in range");
        let bytes = txn.bytes();
        assert_eq!(
            &bytes[second_start..second_start + entry_hdr_len],
            &entry_hdr[..entry_hdr_len]
        );
        assert_eq!(
            &bytes[second_start + entry_hdr_len..second_start + entry_hdr_len + amount_hdr_len],
            &amount_hdr[..amount_hdr_len]
        );
        let mut second_native = 9u64.to_be_bytes();
        second_native[0] |= 0x40;
        assert_eq!(
            &bytes[second_start + entry_hdr_len + amount_hdr_len
                ..second_start + entry_hdr_len + amount_hdr_len + 8],
            &second_native
        );
        assert_eq!(bytes[second_start + region_len - 1], 0xE1); // object end marker

        txn.clear_amounts_second();
        let bytes = txn.bytes();
        assert_eq!(
            &bytes[second_start..second_start + region_len],
            &[codec::NOP; 128][..region_len]
        );
    }
}
