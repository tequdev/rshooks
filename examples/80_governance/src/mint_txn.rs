//! Encodes `GenesisMint` transactions.

use rshooks::prelude::*;
use rshooks::rollback;
use rshooks::txn::codec;

use crate::GENESIS_ACCOUNT;

/// Number of L1 governance seats.
pub const L1_SEATS: usize = 20;

/// Rewardee entry, plus up to one entry per L1 seat.
const MAX_ENTRIES: usize = L1_SEATS + 1;

/// STObject end marker (`0xE1`), fixed regardless of object type.
const OBJECT_END: u8 = 0xE1;
/// STArray end marker (`0xF1`), fixed regardless of array type.
const ARRAY_END: u8 = 0xF1;

/// One `GenesisMint` array entry: `Amount` (native) + `Destination`
/// (account) inside an object header/footer — `2 + 9 + 22 + 1 = 34` bytes,
/// computed the same way reward.c's own "34 bytes per entry" comment
/// arrives at the number, just derived from [`codec`] instead of counted
/// by hand.
const ENTRY_LEN: usize = codec::field_header(sfGenesisMint).1
    + codec::native_amount_field_size(sfAmount)
    + codec::account_id_field_size(sfDestination)
    + 1;

/// Upper bound on the whole encoded transaction: every fixed-size field up
/// to and including `Account`, the worst-case (with-callback)
/// `EmitDetails` region, the `GenesisMints` array header, every possible
/// entry, and the array's own end marker.
const MAX_LEN: usize = codec::transaction_type_field_size(sfTransactionType)
    + codec::u32_field_size(sfFlags)
    + codec::u32_field_size(sfSequence)
    + codec::u32_field_size(sfFirstLedgerSequence)
    + codec::u32_field_size(sfLastLedgerSequence)
    + codec::native_amount_field_size(sfFee)
    + PUBKEY_FIELD_LEN
    + codec::account_id_field_size(sfAccount)
    + EMIT_DETAILS_MAX_LEN
    + codec::field_header(sfGenesisMints).1
    + ENTRY_LEN * MAX_ENTRIES
    + 1;

/// Length of the zero-filled `SigningPubKey` field.
const PUBKEY_FIELD_LEN: usize = codec::field_header(sfSigningPubKey).1 + 1 + 33;

// Precomputed field headers.
const HDR_TRANSACTION_TYPE: ([u8; 3], usize) = codec::field_header(sfTransactionType);
const HDR_FLAGS: ([u8; 3], usize) = codec::field_header(sfFlags);
const HDR_SEQUENCE: ([u8; 3], usize) = codec::field_header(sfSequence);
const HDR_FIRST_LEDGER_SEQUENCE: ([u8; 3], usize) = codec::field_header(sfFirstLedgerSequence);
const HDR_LAST_LEDGER_SEQUENCE: ([u8; 3], usize) = codec::field_header(sfLastLedgerSequence);
const HDR_FEE: ([u8; 3], usize) = codec::field_header(sfFee);
const HDR_SIGNING_PUB_KEY: ([u8; 3], usize) = codec::field_header(sfSigningPubKey);
const HDR_ACCOUNT: ([u8; 3], usize) = codec::field_header(sfAccount);
const HDR_GENESIS_MINTS: ([u8; 3], usize) = codec::field_header(sfGenesisMints);
const HDR_GENESIS_MINT: ([u8; 3], usize) = codec::field_header(sfGenesisMint);
const HDR_AMOUNT: ([u8; 3], usize) = codec::field_header(sfAmount);
const HDR_DESTINATION: ([u8; 3], usize) = codec::field_header(sfDestination);

/// Rolls back an emission failure.
#[inline(always)]
fn fail(msg: &[u8]) -> ! {
    rollback!(msg, -104);
}

/// Encodes drops as a native XAH amount.
#[inline(always)]
fn write_native_amount(dst: &mut [u8], drops: u64) {
    let bytes = drops.to_be_bytes();
    let out: [u8; 8] = [
        0x40 | (bytes[0] & 0x3F),
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
    ];
    for (d, s) in dst.iter_mut().zip(out.iter()) {
        *d = *s;
    }
}

/// A `GenesisMint` transaction under construction: a linear cursor-based
/// writer over a fixed `MAX_LEN` buffer, patched in place for the fields
/// only known once the whole variable-length body has been written (`Fee`,
/// `FirstLedgerSequence`/`LastLedgerSequence`).
#[derive(Clone)]
pub struct MintTxn {
    buf: [u8; MAX_LEN],
    len: usize,
    fee_offset: usize,
    fls_offset: usize,
    lls_offset: usize,
}

impl MintTxn {
    /// Starts a new, empty builder. Call [`Self::start`] before adding
    /// entries.
    #[must_use]
    pub const fn new() -> Self {
        MintTxn {
            buf: [0u8; MAX_LEN],
            len: 0,
            fee_offset: 0,
            fls_offset: 0,
            lls_offset: 0,
        }
    }

    /// Appends a fixed-size `src` to the buffer, advancing the cursor.
    ///
    /// Generic over the const `N` (rather than taking `src: &[u8]`)
    /// deliberately: monomorphizing one small function per distinct `N`
    /// (1, 2, 4, 8, 20, 33 — the field-header/VL-prefix/value sizes this
    /// module ever writes) is what lets `wasm32v1-none`'s `opt-level = "z"`
    /// codegen lower each `copy_from_slice` to a handful of stores. A
    /// single function taking a runtime-length `&[u8]` and called from a
    /// dozen sites with different lengths compiles to a genuine byte-copy
    /// loop instead (empirically: `rshooks build` rejects it as an
    /// unguarded compiler-generated loop).
    #[inline(always)]
    fn push<const N: usize>(&mut self, src: &[u8; N]) -> usize {
        let start = self.len;
        // `wrapping_add`, not `checked_add`, for this range's end: `start`
        // never exceeds the small compile-time constant `MAX_LEN`, so
        // `start + N` cannot overflow `usize`, and `get_mut` below still
        // catches a bad range safely (`None`, not a panic) if it somehow
        // did. This whole `push`/`push_field_header`/`push_u32_field`
        // chain gets force-inlined at every call site inside
        // `push_l1_seat_entries`'s guarded seat loop, so a dead
        // `checked_add`/`else { fail(..) }` branch here is multiplied by
        // the loop's guard maxiter along with everything else.
        let end = start.wrapping_add(N);
        let Some(dst) = self.buf.get_mut(start..end) else {
            fail(b"reward: mint txn overflow");
        };
        // Element-wise (not `copy_from_slice`): `copy_from_slice` panics
        // if the two slices' lengths differ, and the compiler cannot
        // prove `dst`'s length equals `N` from `get_mut`'s return type
        // alone (it's `&mut [u8]`, not `&mut [u8; N]`) — so that
        // unreachable-in-practice panic's message-formatting machinery
        // (which needs to format both lengths) stays linked in, and
        // empirically compiles to an unguarded loop. `Iterator::zip`
        // instead has no length-mismatch case to panic on (it simply
        // stops at the shorter side), so there is no such path to keep.
        for (d, s) in dst.iter_mut().zip(src.iter()) {
            *d = *s;
        }
        self.len = end;
        start
    }

    /// Appends a precomputed STObject/STArray field header (see
    /// [`codec::field_header`] — this covers ordinary fields, the
    /// `GenesisMint` object-start marker, and the `GenesisMints`
    /// array-start marker uniformly, since all three are just a field
    /// header whose `(type, field)` happen to fall in the object/array
    /// ranges). Takes the header **already computed** (see the `HDR_*`
    /// constants below) rather than an `sfXxx` constant to derive one from
    /// at runtime: `field_header` is only documented as safe to call from a
    /// `const` context (its internal range checks become compile-time
    /// assertions there, never runtime code) — calling it at runtime here as
    /// `field_header(sfXxx)` compiled to a genuine, unreachable-in-
    /// practice assertion-failure path whose panic-message formatting
    /// pulled in enough machinery to blow this hook's nesting budget once
    /// inlined (see `crate`'s module doc comment). Every header this
    /// module ever writes is for a compile-time-constant field, so
    /// precomputing them as `const`s has no runtime cost either way.
    #[inline(always)]
    fn push_field_header(&mut self, header: ([u8; 3], usize)) -> usize {
        let (hdr, hdr_len) = header;
        match hdr_len {
            1 => self.push(&[hdr[0]]),
            2 => self.push(&[hdr[0], hdr[1]]),
            _ => self.push(&[hdr[0], hdr[1], hdr[2]]),
        }
    }

    #[inline(always)]
    fn push_u32_field(&mut self, header: ([u8; 3], usize), value: u32) -> usize {
        let offset = self.push_field_header(header);
        self.push(&value.to_be_bytes());
        offset
    }

    /// Writes the transaction's fixed-shape header, up to and including
    /// `Account`: `TransactionType = ttGENESIS_MINT`, `Flags =
    /// tfCANONICAL`, `Sequence = 0`, placeholder `FirstLedgerSequence`/
    /// `LastLedgerSequence`/`Fee` (patched by [`Self::finish`]),
    /// `SigningPubKey` (35-byte zero blob), and `Account = GENESIS_ACCOUNT`.
    pub fn start(&mut self) {
        self.push_field_header(HDR_TRANSACTION_TYPE);
        self.push(&ttGENESIS_MINT.to_be_bytes());

        self.push_u32_field(HDR_FLAGS, tfCANONICAL);
        self.push_u32_field(HDR_SEQUENCE, 0);
        self.fls_offset = self.push_u32_field(HDR_FIRST_LEDGER_SEQUENCE, 0);
        self.lls_offset = self.push_u32_field(HDR_LAST_LEDGER_SEQUENCE, 0);

        self.fee_offset = self.push_field_header(HDR_FEE);
        self.push(&[0u8; 8]); // Fee payload, patched in `finish`

        self.push_field_header(HDR_SIGNING_PUB_KEY);
        self.push(&[33u8]); // VL length prefix
        self.push(&[0u8; 33]); // all-zero pubkey payload

        self.push_field_header(HDR_ACCOUNT);
        self.push(&[20u8]); // VL length prefix
        self.push(&GENESIS_ACCOUNT.0);
    }

    /// Reserves and fills the `EmitDetails` region via `etxn_details`, then
    /// writes the `GenesisMints` array-start marker. Must be called after
    /// [`Self::start`] and before any [`Self::push_entry`].
    pub fn write_emit_details(&mut self) {
        let start = self.len;
        let end = start.wrapping_add(EMIT_DETAILS_MAX_LEN); // range end; see `push`'s overflow comment
        let Some(region) = self.buf.get_mut(start..end) else {
            fail(b"reward: mint txn overflow");
        };
        let written = match etxn_details(region) {
            Ok(n) => n,
            Err(_) => fail(b"reward: could not write EmitDetails"),
        };
        // `written` (116 without a declared `cbak`, 138 with one — this
        // hook declares neither) may be less than the reserved worst-case
        // region; only the actually-written prefix is part of the
        // transaction, so the cursor advances by `written`, not by the
        // full reservation. `written` is host-provided, not a value this
        // module bounds itself, and this assignment isn't a slice range
        // endpoint any `get_mut` here re-checks — `checked_add`, not
        // `wrapping_add`.
        let Some(new_len) = start.checked_add(written) else {
            fail(b"reward: mint txn overflow");
        };
        self.len = new_len;
        self.push_field_header(HDR_GENESIS_MINTS);
    }

    /// Appends one `GenesisMint { Amount, Destination }` array entry. At
    /// most [`MAX_ENTRIES`] are ever pushed (the rewardee plus one per L1
    /// seat), which [`MAX_LEN`] is sized for — see the module doc comment
    /// for what happens if that invariant is ever violated.
    pub fn push_entry(&mut self, drops: u64, destination: &AccountId) {
        self.push_field_header(HDR_GENESIS_MINT);

        self.push_field_header(HDR_AMOUNT);
        let amount_start = self.len;
        let amount_end = amount_start.wrapping_add(8); // range end; see `push`'s overflow comment
        let Some(dst) = self.buf.get_mut(amount_start..amount_end) else {
            fail(b"reward: mint txn overflow");
        };
        write_native_amount(dst, drops);
        self.len = amount_end;

        self.push_field_header(HDR_DESTINATION);
        self.push(&[20u8]); // VL length prefix
        self.push(&destination.0);

        self.push(&[OBJECT_END]);
    }

    /// Closes the `GenesisMints` array and patches `FirstLedgerSequence`,
    /// `LastLedgerSequence`, and `Fee` now that the final length is known.
    /// Returns the completed transaction's bytes.
    ///
    /// `current_ledger_seq` is `ledger_seq()`'s value at the time
    /// [`Self::start`] was called (`FirstLedgerSequence = seq + 1`,
    /// `LastLedgerSequence = seq + 5`, matching reward.c's `seq =
    /// ledger_seq() + 1` / `seq += 4`).
    pub fn finish(&mut self, current_ledger_seq: u32) -> &[u8] {
        self.push(&[ARRAY_END]);

        // `fls`/`lls` are serialized *values* (a ledger sequence plus a
        // small literal offset), not slice-range endpoints any `get_mut`
        // below re-checks, so they stay on `checked_add` rather than the
        // `wrapping_add` this file otherwise uses for range ends bounded by
        // `MAX_LEN` — see `push`'s overflow comment for that case.
        let Some(fls) = current_ledger_seq.checked_add(1) else {
            fail(b"reward: mint txn overflow");
        };
        let Some(lls) = current_ledger_seq.checked_add(5) else {
            fail(b"reward: mint txn overflow");
        };
        let fls_end = self.fls_offset.wrapping_add(4); // range end; see `push`'s overflow comment
        let Some(fls_dst) = self.buf.get_mut(self.fls_offset..fls_end) else {
            fail(b"reward: mint txn overflow");
        };
        for (d, s) in fls_dst.iter_mut().zip(fls.to_be_bytes().iter()) {
            *d = *s;
        }
        let lls_end = self.lls_offset.wrapping_add(4); // range end; see `push`'s overflow comment
        let Some(lls_dst) = self.buf.get_mut(self.lls_offset..lls_end) else {
            fail(b"reward: mint txn overflow");
        };
        for (d, s) in lls_dst.iter_mut().zip(lls.to_be_bytes().iter()) {
            *d = *s;
        }

        let Some(bytes) = self.buf.get(..self.len) else {
            fail(b"reward: mint txn overflow");
        };
        let fee = match etxn_fee_base(bytes) {
            Ok(f) => f,
            Err(_) => fail(b"reward: could not compute GenesisMint fee"),
        };
        let fee_end = self.fee_offset.wrapping_add(8); // range end; see `push`'s overflow comment
        let Some(fee_dst) = self.buf.get_mut(self.fee_offset..fee_end) else {
            fail(b"reward: mint txn overflow");
        };
        write_native_amount(fee_dst, fee);

        let Some(bytes) = self.buf.get(..self.len) else {
            fail(b"reward: mint txn overflow");
        };
        bytes
    }
}

impl Default for MintTxn {
    fn default() -> Self {
        Self::new()
    }
}
