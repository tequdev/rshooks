#![cfg_attr(not(test), no_std)]

use rshooks::prelude::*;
use rshooks::*;

/// Baked currency for every issued `Amount` this crate writes (`send_max`'s
/// issued form, `amounts`' baked defaults); the issuer for a
/// runtime-supplied issued amount comes from the `ISSUER` hook parameter,
/// the baked defaults from [`USD_ISSUER`].
const USD: CurrencyCode = CurrencyCode::from_iso(b"USD");

/// Baked issuer for `OptionalRemit::amounts`' `second` entry and
/// `OptionalVl::amounts`' single entry's baked `IouAmount(0, USD, ..)`
/// default — an arbitrary, real-looking `r`-address, matching
/// `examples/21_txn-template-nested`'s own `USD_ISSUER`.
const USD_ISSUER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");

/// Backing bytes for `OptionalVl::note` (`vl(sfBlob, 190, 194)`) — a
/// top-level `const`, not a local array, so referencing a slice of it is
/// a pointer/length pair with no runtime fill (`docs/DESIGN.md` §2's
/// "static-buffer idiom" for anything past the memset-libcall threshold).
/// `vl` writes either its first 190 bytes (a one-byte `VL` length prefix)
/// or the full 194 (a two-byte prefix), so one hook parameter exercises
/// both `vl_length_prefix` widths.
const NOTE: [u8; 194] = [b'n'; 194];

/// Backing bytes for `OptionalRemit::blob` (`optional vl(sfBlob, 2, 8)`),
/// written in full when present.
const BLOB: [u8; 5] = *b"blob!";

txn_template! {
    /// A Payment exercising an inferred `optional` scalar
    /// (`destination_tag`) and `optional any_amount` (`send_max`) —
    /// every field legal for a Payment per
    /// `crates/rshooks-core/protocol_formats.json`.
    ///
    /// Field order (canonical `(type, field)`): `sequence` (2,4) <
    /// `destination_tag` (2,14) < `first_ledger_sequence` (2,26) <
    /// `last_ledger_sequence` (2,27) < `amount` (6,1) < `fee` (6,8) <
    /// `send_max` (6,9) < `signing_pub_key` (7,3) < `account` (8,1) <
    /// `destination` (8,3).
    struct OptionalPayment {
        transaction_type = ttPAYMENT,
        sequence: sfSequence = 0,
        destination_tag: optional sfDestinationTag,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        amount: native_amount(sfAmount) = 1,
        fee: native_amount(sfFee) = 0,
        send_max: optional any_amount(sfSendMax),
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: sfAccount,
        destination: sfDestination,
        emit_details: emit_details,
    }
}

txn_template! {
    /// A Remit exercising `optional vl`, a whole-container `optional
    /// array` view, `any_amount` (always present, `amounts.first`), and
    /// a named array with one required element and one `optional`
    /// element (`amounts.second`) — every field legal for a Remit per
    /// `crates/rshooks-core/protocol_formats.json`.
    ///
    /// Field order: `sequence` (2,4) < `destination_tag` (2,14) <
    /// `first_ledger_sequence` (2,26) < `last_ledger_sequence` (2,27) <
    /// `fee` (6,8) < `signing_pub_key` (7,3) < `blob` (7,26) < `account`
    /// (8,1) < `destination` (8,3) < `memos` (15,9) < `amounts` (15,92).
    struct OptionalRemit {
        transaction_type = ttREMIT,
        sequence: sfSequence = 0,
        destination_tag: optional sfDestinationTag,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        blob: optional vl(sfBlob, 2, 8),
        account: sfAccount,
        destination: sfDestination,
        memos: optional Memos: sfMemos [
            memo: sfMemo {
                memo_type: fixed_vl(sfMemoType, 4) = *b"note",
            }
        ],
        amounts: sfAmounts [
            first: sfAmountEntry {
                amount: any_amount(sfAmount),
            },
            second: optional Second: sfAmountEntry {
                amount: sfAmount = IouAmount(XFL::from_raw_bits(0), USD, USD_ISSUER),
            },
        ],
        emit_details: emit_details,
    }
}

txn_template! {
    /// A second Remit isolating `vl`'s runtime-chosen length in its own
    /// entry, plus a whole-container `optional object` view
    /// (`sfMintURIToken`) and a homogeneous array of `0` or `1`
    /// `optional` elements — every field legal for a Remit.
    ///
    /// `note`'s `set_note` is a `guard_m!`-protected copy loop bounded by
    /// `MAX`, so its WCE contribution is proportional to `MAX`
    /// (`docs/NOP_PADDING_DESIGN.md` §3.1) — keeping it isolated from
    /// `OptionalRemit`'s other, independent kinds keeps that cost legible
    /// in `metrics.json` on its own. `MAX = 194` also crosses the
    /// 192/193-byte `VL`-length-prefix-width boundary (one hook
    /// parameter selects a 190-byte write, one-byte prefix, or the full
    /// 194-byte write, two-byte prefix), which needs no more budget than
    /// a small `MAX` would (the charge is `slot(MAX) - slot(MIN)`, not
    /// `MAX` itself).
    ///
    /// Field order: `sequence` (2,4) < `first_ledger_sequence` (2,26) <
    /// `last_ledger_sequence` (2,27) < `invoice_id` (5,17) < `fee` (6,8)
    /// < `signing_pub_key` (7,3) < `note` (7,26) < `account` (8,1) <
    /// `destination` (8,3) < `mint` (14,92) < `amounts` (15,92).
    struct OptionalVl {
        transaction_type = ttREMIT,
        sequence: sfSequence = 0,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        invoice_id: optional sfInvoiceID,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        note: vl(sfBlob, 190, 194),
        account: sfAccount,
        destination: sfDestination,
        mint: optional Mint: sfMintURIToken {
            flags: optional sfFlags,
            uri: fixed_vl(sfURI, 4) = *b"ipfs",
        },
        amounts: sfAmounts [
            Entry: optional sfAmountEntry {
                amount: sfAmount = IouAmount(XFL::from_raw_bits(0), USD, USD_ISSUER),
            }; 1
        ],
        emit_details: emit_details,
    }
}

/// The reusable `OptionalPayment` template.
static PAYMENT_TXN: HookStatic<OptionalPayment> = HookStatic::new(OptionalPayment::new());
/// The reusable `OptionalRemit` template.
static REMIT_TXN: HookStatic<OptionalRemit> = HookStatic::new(OptionalRemit::new());
/// The reusable `OptionalVl` template.
static VL_TXN: HookStatic<OptionalVl> = HookStatic::new(OptionalVl::new());

hook_errors! {
    /// Errors returned by the three emission hooks.
    pub enum TxnTemplateOptionalError {
        /// An emission slot could not be reserved.
        ReserveFailed = 1,
        /// The `DEST` hook parameter was missing or not a 20-byte AccountID.
        MissingDestination = 2,
        /// The reusable template buffer was unavailable.
        BufferAlreadyTaken = 3,
        /// An `any_amount`/`native_amount`/`amount` value was out of
        /// range.
        AmountFailed = 4,
        /// `OptionalVl::note`'s length fell outside `[190, 194]` —
        /// unreachable by construction (both lengths are fixed literals),
        /// kept only because the setter returns `Result`.
        NoteFailed = 5,
        /// `OptionalRemit::blob`'s length fell outside `[2, 8]` —
        /// unreachable by construction, kept only because the setter
        /// returns `Result`.
        BlobFailed = 6,
        /// `OptionalVl::amounts`'s single-element index was out of range
        /// — unreachable by construction (the index is the literal `0`,
        /// always below the declared element count `1`), kept only
        /// because the accessor returns `Option`.
        AmountsIndexOutOfRange = 7,
        /// The template could not be prepared.
        PrepareFailed = 8,
        /// The prepared transaction could not be emitted.
        EmitFailed = 9,
    }
}

#[hooks(
    description = "Emits templates exercising txn_template!'s NOP-padded optional and variable-length field kinds."
)]
pub struct TxnTemplateOptional {
    /// The destination account for all three templates; required.
    #[hook_param(name = b"DEST", required)]
    dest: HookParam<AccountId>,
    /// `OptionalPayment::destination_tag`/`OptionalRemit::destination_tag`,
    /// if present.
    #[hook_param(name = b"DEST_TAG")]
    dest_tag: HookParam<[u8; 4]>,
    /// When present, `OptionalPayment::send_max` is set to the issued form
    /// (currency `USD`, this account as issuer) instead of the native
    /// form `SEND_MAX` alone would select.
    #[hook_param(name = b"ISSUER")]
    issuer: HookParam<AccountId>,
    /// Presence-only: enables `OptionalPayment::send_max`'s native form
    /// (500 drops) when `ISSUER` is absent.
    #[hook_param(name = b"SEND_MAX")]
    send_max: HookParam<[u8; 1]>,
    /// Presence-only: writes `OptionalVl::note` at its 194-byte (two-byte
    /// `VL` prefix) length instead of its 190-byte (one-byte prefix)
    /// length.
    #[hook_param(name = b"NOTE_LONG")]
    note_long: HookParam<[u8; 1]>,
    /// Presence-only: enables `OptionalRemit::memos`.
    #[hook_param(name = b"MEMO")]
    memo: HookParam<[u8; 1]>,
    /// `OptionalVl::invoice_id`, if present.
    #[hook_param(name = b"INVOICE")]
    invoice: HookParam<Hash>,
    /// Presence-only: writes `OptionalRemit::blob`.
    #[hook_param(name = b"BLOB")]
    blob: HookParam<[u8; 1]>,
    /// Presence-only: enables `OptionalRemit::amounts`'s `second` element
    /// (`tplremit`) or `OptionalVl::amounts`'s single element (`tplvl`).
    #[hook_param(name = b"AMT_ENTRY")]
    amt_entry: HookParam<[u8; 1]>,
    /// Presence-only: enables `OptionalVl::mint`.
    #[hook_param(name = b"MINT")]
    mint: HookParam<[u8; 1]>,
}

#[hooks]
impl TxnTemplateOptional {
    /// Reserves one emission slot, reads the required `DEST` hook
    /// parameter, fills `OptionalPayment`'s optional fields from the
    /// remaining (all optional) hook parameters, and emits.
    #[hook(0, name = "tplpay", on = [Invoke], can_emit = [Payment])]
    fn pay(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"txn-template-optional: etxn_reserve failed",
                TxnTemplateOptionalError::ReserveFailed
            );
        }

        let Ok(destination) = self.hook_param.dest.get_required() else {
            rollback!(
                b"txn-template-optional: missing DEST hook parameter",
                TxnTemplateOptionalError::MissingDestination
            )
        };

        let Some(txn) = PAYMENT_TXN.take() else {
            rollback!(
                b"txn-template-optional: payment buffer already taken",
                TxnTemplateOptionalError::BufferAlreadyTaken
            );
        };

        txn.set_destination(&destination);

        if let Ok(Some(tag)) = self.hook_param.dest_tag.get() {
            txn.set_destination_tag(u32::from_be_bytes(tag));
        }

        match self.hook_param.issuer.get() {
            Ok(Some(iss)) => {
                let Ok(one) = XFL::new(0, 1) else {
                    rollback!(
                        b"txn-template-optional: XFL::new failed",
                        TxnTemplateOptionalError::AmountFailed
                    );
                };
                txn.set_send_max_issued(one, &USD, &iss);
            }
            _ => {
                if matches!(self.hook_param.send_max.get(), Ok(Some(_)))
                    && txn.set_send_max_native(500).is_err()
                {
                    rollback!(
                        b"txn-template-optional: send_max native amount out of range",
                        TxnTemplateOptionalError::AmountFailed
                    );
                }
            }
        }

        let Ok(prepared) = txn.prepare_for_emit() else {
            rollback!(
                b"txn-template-optional: prepare_for_emit failed",
                TxnTemplateOptionalError::PrepareFailed
            )
        };

        match prepared.emit() {
            Ok(_hash) => accept!(b"txn-template-optional: payment emitted", 0),
            Err(_) => rollback!(
                b"txn-template-optional: emit failed",
                TxnTemplateOptionalError::EmitFailed
            ),
        }
    }

    #[cbak(0)]
    fn pay_cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    /// Reserves one emission slot, reads the required `DEST` hook
    /// parameter, fills `OptionalRemit`'s optional/variable fields from
    /// the remaining (all optional) hook parameters, and emits. `amounts`'
    /// `first` entry is always written to a real, constructible amount
    /// (1 native drop) — the required element of a named array is never
    /// left at its raw encoding default the way an `optional` one is.
    #[hook(1, name = "tplremit", on = [Invoke], can_emit = [Remit])]
    fn remit(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"txn-template-optional: etxn_reserve failed",
                TxnTemplateOptionalError::ReserveFailed
            );
        }

        let Ok(destination) = self.hook_param.dest.get_required() else {
            rollback!(
                b"txn-template-optional: missing DEST hook parameter",
                TxnTemplateOptionalError::MissingDestination
            )
        };

        let Some(txn) = REMIT_TXN.take() else {
            rollback!(
                b"txn-template-optional: remit buffer already taken",
                TxnTemplateOptionalError::BufferAlreadyTaken
            );
        };

        txn.set_destination(&destination);

        if let Ok(Some(tag)) = self.hook_param.dest_tag.get() {
            txn.set_destination_tag(u32::from_be_bytes(tag));
        }

        if matches!(self.hook_param.blob.get(), Ok(Some(_))) && txn.set_blob(&BLOB).is_err() {
            rollback!(
                b"txn-template-optional: blob out of the declared [2, 8] range",
                TxnTemplateOptionalError::BlobFailed
            );
        }

        if matches!(self.hook_param.memo.get(), Ok(Some(_))) {
            let _ = txn.enable_memos();
        }

        if txn.set_amounts_first_amount_native(1).is_err() {
            rollback!(
                b"txn-template-optional: amounts.first native amount out of range",
                TxnTemplateOptionalError::AmountFailed
            );
        }

        if matches!(self.hook_param.amt_entry.get(), Ok(Some(_))) {
            let Ok(one) = XFL::new(0, 1) else {
                rollback!(
                    b"txn-template-optional: XFL::new failed",
                    TxnTemplateOptionalError::AmountFailed
                );
            };
            txn.enable_amounts_second().set_amount_value(one);
        }

        let Ok(prepared) = txn.prepare_for_emit() else {
            rollback!(
                b"txn-template-optional: prepare_for_emit failed",
                TxnTemplateOptionalError::PrepareFailed
            )
        };

        match prepared.emit() {
            Ok(_hash) => accept!(b"txn-template-optional: remit emitted", 0),
            Err(_) => rollback!(
                b"txn-template-optional: emit failed",
                TxnTemplateOptionalError::EmitFailed
            ),
        }
    }

    #[cbak(1)]
    fn remit_cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    /// Reserves one emission slot, reads the required `DEST` hook
    /// parameter, writes `OptionalVl::note` at either its `MIN` (190,
    /// one-byte `VL` prefix) or `MAX` (194, two-byte prefix) length
    /// depending on `NOTE_LONG`, optionally enables `mint`/`amounts`'
    /// single element, and emits.
    #[hook(2, name = "tplvl", on = [Invoke], can_emit = [Remit])]
    fn vl(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            rollback!(
                b"txn-template-optional: etxn_reserve failed",
                TxnTemplateOptionalError::ReserveFailed
            );
        }

        let Ok(destination) = self.hook_param.dest.get_required() else {
            rollback!(
                b"txn-template-optional: missing DEST hook parameter",
                TxnTemplateOptionalError::MissingDestination
            )
        };

        let Some(txn) = VL_TXN.take() else {
            rollback!(
                b"txn-template-optional: vl buffer already taken",
                TxnTemplateOptionalError::BufferAlreadyTaken
            );
        };

        txn.set_destination(&destination);

        if let Ok(Some(hash)) = self.hook_param.invoice.get() {
            txn.set_invoice_id(&hash);
        }

        let long = matches!(self.hook_param.note_long.get(), Ok(Some(_)));
        let note_len: usize = if long { 194 } else { 190 };
        let Some(note_bytes) = NOTE.get(..note_len) else {
            rollback!(
                b"txn-template-optional: note length out of range",
                TxnTemplateOptionalError::NoteFailed
            );
        };
        if txn.set_note(note_bytes).is_err() {
            rollback!(
                b"txn-template-optional: note out of the declared [190, 194] range",
                TxnTemplateOptionalError::NoteFailed
            );
        }

        if matches!(self.hook_param.amt_entry.get(), Ok(Some(_))) {
            let Some(mut entry) = txn.amounts(0) else {
                rollback!(
                    b"txn-template-optional: amounts index out of range",
                    TxnTemplateOptionalError::AmountsIndexOutOfRange
                );
            };
            let Ok(one) = XFL::new(0, 1) else {
                rollback!(
                    b"txn-template-optional: XFL::new failed",
                    TxnTemplateOptionalError::AmountFailed
                );
            };
            entry.enable();
            entry.set_amount_value(one);
        }

        if matches!(self.hook_param.mint.get(), Ok(Some(_))) {
            let mut m = txn.enable_mint();
            m.set_flags(1);
        }

        let Ok(prepared) = txn.prepare_for_emit() else {
            rollback!(
                b"txn-template-optional: prepare_for_emit failed",
                TxnTemplateOptionalError::PrepareFailed
            )
        };

        match prepared.emit() {
            Ok(_hash) => accept!(b"txn-template-optional: vl remit emitted", 0),
            Err(_) => rollback!(
                b"txn-template-optional: emit failed",
                TxnTemplateOptionalError::EmitFailed
            ),
        }
    }

    #[cbak(2)]
    fn vl_cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

// Off-chain unit tests exercising the private template types directly —
// no host backend needed, since none of these calls reach
// `prepare_for_emit`/`emit` (only `HookParam`-free setters on a
// freshly-constructed template) — for byte-level NOP-padding assertions
// only reachable from an in-crate test (`OptionalPayment`/`OptionalRemit`/
// `OptionalVl` are private). `tests/pay.rs`, `tests/remit.rs`, and
// `tests/vl.rs` cover the through-`TestEnv` functional/decoded-value
// side, against `emitted()`'s canonical (NOP-free) bytes.
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

    /// `destination_tag` (`optional sfDestinationTag`) defaults absent —
    /// its whole `header + value` slot, header included, is `NOP`s (an
    /// `optional` scalar's setter writes `header + value` together, so
    /// nothing distinguishes the header position when absent) — and,
    /// once set, carries the header and big-endian value at that slot;
    /// `clear_destination_tag()` restores the `NOP`s.
    #[test]
    fn destination_tag_defaults_absent_and_round_trips() {
        let mut txn = OptionalPayment::new();
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

    /// `send_max` (`optional any_amount(sfSendMax)`) defaults absent
    /// (`header + 48` `NOP`s, header included); once present, its *form*
    /// varies: the native form leaves 40 trailing `NOP` bytes in the
    /// reserved 48-byte value region, the issued form fills it exactly;
    /// `clear_send_max()` restores the `NOP`s.
    #[test]
    fn send_max_defaults_absent_native_and_issued_forms() {
        let mut txn = OptionalPayment::new();
        let (hdr, hdr_len) = codec::field_header(sfSendMax);

        // An absent `optional` field has no header baked anywhere (the
        // *whole* slot is `NOP`s) -- locate the slot's fixed offset by
        // setting the field first, then check a *fresh* instance (same
        // compile-time-fixed offset) for the absent-by-default shape.
        txn.set_send_max_native(1000)
            .expect("1000 drops is in range");
        let bytes = txn.bytes();
        let start = bytes
            .windows(hdr_len)
            .position(|w| w == &hdr[..hdr_len])
            .expect("sfSendMax header present once set");
        assert_eq!(
            &OptionalPayment::new().bytes()[start..start + hdr_len + 48],
            &[codec::NOP; 128][..hdr_len + 48],
            "absent by default"
        );
        let mut native_value = 1000u64.to_be_bytes();
        native_value[0] |= 0x40;
        assert_eq!(&bytes[start + hdr_len..start + hdr_len + 8], &native_value);
        assert_eq!(
            &bytes[start + hdr_len + 8..start + hdr_len + 48],
            &[codec::NOP; 40]
        );

        // `XFL::from_raw_bits`, not `XFL::new`: this host-side test
        // installs no backend, and `XFL::new`'s normalization is a real
        // host `float_set` call — only the byte layout matters here, not
        // the value.
        let issuer = AccountId([9u8; 20]);
        txn.set_send_max_issued(XFL::from_raw_bits(0), &USD, &issuer);
        let bytes = txn.bytes();
        assert!(
            !bytes[start + hdr_len..start + hdr_len + 48].contains(&codec::NOP),
            "issued form must fill the whole 48-byte region"
        );

        txn.clear_send_max();
        let bytes = txn.bytes();
        assert_eq!(
            &bytes[start..start + hdr_len + 48],
            &[codec::NOP; 128][..hdr_len + 48]
        );
    }

    /// `note` (`vl(sfBlob, 190, 194)`) at its `MIN` length (190, a
    /// one-byte `VL` prefix) leaves five trailing `NOP` bytes in the
    /// `MAX`-sized (194, two-byte-prefix) slot; at `MAX` it fills the
    /// slot exactly; shrinking back from `MAX` to `MIN` restores those
    /// five trailing `NOP`s rather than leaving stale payload bytes
    /// behind.
    #[test]
    fn note_min_length_leaves_nop_tail_max_does_not() {
        let mut txn = OptionalVl::new();
        let (hdr, hdr_len) = codec::field_header(sfBlob);
        let bytes = txn.bytes();
        let start = bytes
            .windows(hdr_len)
            .position(|w| w == &hdr[..hdr_len])
            .expect("sfBlob header present");

        txn.set_note(&NOTE[..190])
            .expect("190 is within [190, 194]");
        let bytes = txn.bytes();
        assert_eq!(bytes[start + hdr_len], 190); // one-byte VL prefix, 190 <= 192
        assert_eq!(
            &bytes[start + hdr_len + 1..start + hdr_len + 1 + 190],
            &NOTE[..190]
        );
        assert_eq!(
            &bytes[start + hdr_len + 1 + 190..start + hdr_len + 1 + 190 + 5],
            &[codec::NOP; 5]
        );

        txn.set_note(&NOTE[..194])
            .expect("194 is within [190, 194]");
        let bytes = txn.bytes();
        // Two-byte VL prefix for 194 (193..=12480): `193 + ((194-193) >> 8)`,
        // `(194-193) & 0xFF`.
        assert_eq!(&bytes[start + hdr_len..start + hdr_len + 2], &[0xC1, 0x01]);
        assert_eq!(
            &bytes[start + hdr_len + 2..start + hdr_len + 2 + 194],
            &NOTE
        );

        // Shrink back from MAX (194, two-byte prefix) to MIN (190,
        // one-byte prefix): the vacated tail -- the byte the two-byte
        // prefix used plus the 4 bytes of payload no longer written --
        // must read back as NOPs, not stale bytes from the 194-byte write.
        txn.set_note(&NOTE[..190])
            .expect("190 is within [190, 194]");
        let bytes = txn.bytes();
        assert_eq!(bytes[start + hdr_len], 190);
        assert_eq!(
            &bytes[start + hdr_len + 1..start + hdr_len + 1 + 190],
            &NOTE[..190]
        );
        assert_eq!(
            &bytes[start + hdr_len + 1 + 190..start + hdr_len + 1 + 190 + 5],
            &[codec::NOP; 5],
            "shrinking back to MIN must NOP-fill the vacated tail, not leave MAX's stale bytes"
        );
    }

    /// `invoice_id` (`optional sfInvoiceID`) defaults absent (`header +
    /// 32` `NOP`s, header included) and carries the header and value once
    /// set; `clear_invoice_id()` restores the `NOP`s.
    #[test]
    fn invoice_id_defaults_absent_and_round_trips() {
        let mut txn = OptionalVl::new();
        let (hdr, hdr_len) = codec::field_header(sfInvoiceID);

        let hash = Hash([7u8; 32]);
        txn.set_invoice_id(&hash);
        let bytes = txn.bytes();
        let start = bytes
            .windows(hdr_len)
            .position(|w| w == &hdr[..hdr_len])
            .expect("sfInvoiceID header present once set");
        assert_eq!(&bytes[start..start + hdr_len], &hdr[..hdr_len]);
        assert_eq!(&bytes[start + hdr_len..start + hdr_len + 32], &[7u8; 32]);

        txn.clear_invoice_id();
        let bytes = txn.bytes();
        assert_eq!(
            &bytes[start..start + hdr_len + 32],
            &[codec::NOP; 41][..hdr_len + 32]
        );
    }

    /// `blob` (`optional vl(sfBlob, 2, 8)`) defaults absent (the whole
    /// `header + prefix(8) + 8` slot, header included, is `NOP`s) and
    /// carries the written payload, `NOP`-padded to the slot end, once
    /// set; `clear_blob()` restores the `NOP`s.
    #[test]
    fn blob_defaults_absent_and_round_trips() {
        let mut txn = OptionalRemit::new();
        let (hdr, hdr_len) = codec::field_header(sfBlob);
        let slot_len = hdr_len + 1 + 8; // header + one-byte prefix(8) + 8

        txn.set_blob(&BLOB).expect("5 bytes is within [2, 8]");
        let bytes = txn.bytes();
        let start = bytes
            .windows(hdr_len)
            .position(|w| w == &hdr[..hdr_len])
            .expect("sfBlob header present once set");
        assert_eq!(&bytes[start..start + hdr_len], &hdr[..hdr_len]);
        assert_eq!(bytes[start + hdr_len], 5); // one-byte VL prefix, 5 <= 192
        assert_eq!(&bytes[start + hdr_len + 1..start + hdr_len + 1 + 5], &BLOB);
        assert_eq!(
            &bytes[start + hdr_len + 1 + 5..start + slot_len],
            &[codec::NOP; 3]
        );

        txn.clear_blob();
        let bytes = txn.bytes();
        assert_eq!(
            &bytes[start..start + slot_len],
            &[codec::NOP; 128][..slot_len]
        );
    }

    /// `mint` (`optional Mint: sfMintURIToken { flags: optional sfFlags,
    /// uri: fixed_vl(sfURI, 4) = *b"ipfs" }`) defaults absent (the whole
    /// region, header included, is `NOP`s) and, once enabled, carries the
    /// baked `uri = "ipfs"` default (`flags` itself stays absent — it is
    /// independently `optional` inside the view) at the same offset;
    /// `clear_mint()` restores the `NOP`s.
    #[test]
    fn mint_defaults_absent_and_enables() {
        let mut txn = OptionalVl::new();
        let (mint_hdr, mint_hdr_len) = codec::field_header(sfMintURIToken);
        let (uri_hdr, uri_hdr_len) = codec::field_header(sfURI);
        // header + flags(header + 4, all-NOP) + uri(header + 1-byte prefix + 4) + object end marker.
        let region_len = mint_hdr_len + 5 + uri_hdr_len + 1 + 4 + 1;

        let _ = txn.enable_mint();
        let bytes = txn.bytes();
        let start = bytes
            .windows(mint_hdr_len)
            .position(|w| w == &mint_hdr[..mint_hdr_len])
            .expect("sfMintURIToken header present once enabled");
        assert_eq!(
            &bytes[start..start + mint_hdr_len],
            &mint_hdr[..mint_hdr_len]
        );
        // `flags` (optional, right after the container header) stays absent.
        assert_eq!(
            &bytes[start + mint_hdr_len..start + mint_hdr_len + 5],
            &[codec::NOP; 5]
        );
        let uri_start = start + mint_hdr_len + 5;
        assert_eq!(
            &bytes[uri_start..uri_start + uri_hdr_len],
            &uri_hdr[..uri_hdr_len]
        );
        assert_eq!(bytes[uri_start + uri_hdr_len], 4); // one-byte VL prefix, 4 <= 192
        assert_eq!(
            &bytes[uri_start + uri_hdr_len + 1..uri_start + uri_hdr_len + 1 + 4],
            b"ipfs"
        );
        assert_eq!(bytes[start + region_len - 1], 0xE1); // object end marker

        txn.clear_mint();
        let bytes = txn.bytes();
        assert_eq!(
            &bytes[start..start + region_len],
            &[codec::NOP; 128][..region_len]
        );
    }

    /// `amounts` (`sfAmounts [ first: sfAmountEntry { amount:
    /// any_amount(sfAmount) }, second: optional Second: sfAmountEntry {
    /// .. } ]`): `first` is always present (the baked issued-zero
    /// default `any_amount` shares with `amount`), immediately followed
    /// by `second`'s reserved region, absent by default; once enabled,
    /// `second` carries its own baked `IouAmount(0, USD, USD_ISSUER)`
    /// default at the same offset; `clear_amounts_second()` restores the
    /// `NOP`s.
    #[test]
    fn amounts_second_defaults_absent_and_enables() {
        let mut txn = OptionalRemit::new();
        let (entry_hdr, entry_hdr_len) = codec::field_header(sfAmountEntry);
        let (amount_hdr, amount_hdr_len) = codec::field_header(sfAmount);
        // header + amount header + 48-byte issued value + object end marker
        // -- `first`'s `any_amount` and `second`'s `amount` both reserve
        // the same 48-byte value region, so the two elements are the same
        // total size despite the different kind.
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

        let _ = txn.enable_amounts_second();
        let bytes = txn.bytes();
        assert_eq!(
            &bytes[second_start..second_start + entry_hdr_len],
            &entry_hdr[..entry_hdr_len]
        );
        assert_eq!(
            &bytes[second_start + entry_hdr_len..second_start + entry_hdr_len + amount_hdr_len],
            &amount_hdr[..amount_hdr_len]
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
