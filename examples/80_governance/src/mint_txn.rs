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

/// Upper bound on the whole encoded transaction: [`PREFIX_LEN`], the
/// worst-case (with-callback) `EmitDetails` region, the `GenesisMints`
/// array header, every possible entry, and the array's own end marker.
const MAX_LEN: usize = PREFIX_LEN
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

/// Length of the fixed emit-plumbing prefix [`PREFIX`] bakes:
/// `TransactionType` through `Account`.
const PREFIX_LEN: usize = codec::transaction_type_field_size(sfTransactionType)
    + codec::u32_field_size(sfFlags)
    + codec::u32_field_size(sfSequence)
    + codec::u32_field_size(sfFirstLedgerSequence)
    + codec::u32_field_size(sfLastLedgerSequence)
    + codec::native_amount_field_size(sfFee)
    + PUBKEY_FIELD_LEN
    + codec::account_id_field_size(sfAccount);

/// Offsets [`MintTxn::finish`] patches, baked alongside [`PREFIX`]'s image
/// in the same `const` block. A named struct, not a bare `(usize, usize,
/// usize)` tuple: three same-typed positional fields would let two of them
/// transpose silently, with nothing to catch it at compile time — see
/// `docs/TXN_TEMPLATE_FIELDS_DESIGN.md` §7.3.
struct PrefixOffsets {
    first_ledger_sequence_off: usize,
    last_ledger_sequence_off: usize,
    fee_off: usize,
}

/// The fixed emit-plumbing prefix (`TransactionType = ttGENESIS_MINT`,
/// `Flags = tfCANONICAL`, `Sequence = 0`, `FirstLedgerSequence`/
/// `LastLedgerSequence`/`Fee` placeholders, zero-filled `SigningPubKey`,
/// `Account = GENESIS_ACCOUNT`) and the offsets [`MintTxn::finish`]
/// patches, baked together in one `const` block over [`codec`]'s writers
/// so the offsets cannot drift from the image — see
/// `docs/TXN_TEMPLATE_FIELDS_DESIGN.md` §7.
const PREFIX: ([u8; PREFIX_LEN], PrefixOffsets) = {
    let mut buf = [0u8; PREFIX_LEN];
    let mut off = 0usize;

    codec::write_field_header(&mut buf, off, sfTransactionType);
    off = off.wrapping_add(HDR_TRANSACTION_TYPE.1);
    codec::write_uint_be(&mut buf, off, 2, ttGENESIS_MINT as u64);
    off = off.wrapping_add(2);

    codec::write_field_header(&mut buf, off, sfFlags);
    off = off.wrapping_add(HDR_FLAGS.1);
    codec::write_uint_be(&mut buf, off, 4, tfCANONICAL as u64);
    off = off.wrapping_add(4);

    codec::write_field_header(&mut buf, off, sfSequence);
    off = off.wrapping_add(HDR_SEQUENCE.1);
    off = off.wrapping_add(4); // Sequence = 0

    codec::write_field_header(&mut buf, off, sfFirstLedgerSequence);
    off = off.wrapping_add(HDR_FIRST_LEDGER_SEQUENCE.1);
    let fls_off = off;
    off = off.wrapping_add(4);

    codec::write_field_header(&mut buf, off, sfLastLedgerSequence);
    off = off.wrapping_add(HDR_LAST_LEDGER_SEQUENCE.1);
    let lls_off = off;
    off = off.wrapping_add(4);

    codec::write_field_header(&mut buf, off, sfFee);
    off = off.wrapping_add(HDR_FEE.1);
    let fee_off = off;
    codec::write_const_bytes(&mut buf, off, &codec::encode_native_amount_const(0));
    off = off.wrapping_add(8); // Fee = 0, patched by `finish`

    codec::write_field_header(&mut buf, off, sfSigningPubKey);
    off = off.wrapping_add(HDR_SIGNING_PUB_KEY.1);
    codec::write_const_bytes(&mut buf, off, &[33u8]); // VL length prefix
    off = off.wrapping_add(1).wrapping_add(33); // all-zero pubkey payload

    codec::write_field_header(&mut buf, off, sfAccount);
    off = off.wrapping_add(HDR_ACCOUNT.1);
    codec::write_const_bytes(&mut buf, off, &[20u8]); // VL length prefix
    off = off.wrapping_add(1);
    codec::write_const_bytes(&mut buf, off, &GENESIS_ACCOUNT.0);
    off = off.wrapping_add(ACC_ID_LEN);
    assert!(
        off == PREFIX_LEN,
        "PREFIX builder under/over-ran PREFIX_LEN"
    );

    (
        buf,
        PrefixOffsets {
            first_ledger_sequence_off: fls_off,
            last_ledger_sequence_off: lls_off,
            fee_off,
        },
    )
};

/// Checks, at compile time, that `off` is exactly the byte position right
/// after `header`'s own bytes within `image` — proof an offset names a
/// field's value, never the byte its own header starts at.
///
/// # Panics (compile-time only)
///
/// Never: every index is proven in-bounds by the `off < hdr_len` guard and
/// the `while i < hdr_len` loop bound before use, and this is only ever
/// called from a `const` context.
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // in-bounds by construction; const-only, see the Panics note
const fn offset_follows_header<const N: usize>(
    image: &[u8; N],
    header: ([u8; 3], usize),
    off: usize,
) -> bool {
    let (hdr, hdr_len) = header;
    if off < hdr_len {
        return false;
    }
    let base = off.wrapping_sub(hdr_len);
    let mut i = 0;
    while i < hdr_len {
        if image[base.wrapping_add(i)] != hdr[i] {
            return false;
        }
        i = i.wrapping_add(1);
    }
    true
}

// `examples/80_governance` is `crate-type = ["cdylib"]` with `[lib] test =
// false`, so it cannot run a `#[test]`; these compile-time checks are the
// coverage it can afford, and run on every build rather than only when
// someone remembers to run `cargo test`.
const _: () = {
    assert!(
        offset_follows_header(
            &PREFIX.0,
            HDR_FIRST_LEDGER_SEQUENCE,
            PREFIX.1.first_ledger_sequence_off
        ),
        "MintTxn: FirstLedgerSequence's recorded offset does not follow its own header"
    );
    assert!(
        offset_follows_header(
            &PREFIX.0,
            HDR_LAST_LEDGER_SEQUENCE,
            PREFIX.1.last_ledger_sequence_off
        ),
        "MintTxn: LastLedgerSequence's recorded offset does not follow its own header"
    );
    assert!(
        offset_follows_header(&PREFIX.0, HDR_FEE, PREFIX.1.fee_off),
        "MintTxn: Fee's recorded offset does not follow its own header"
    );
};

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
    /// Starts a new, empty builder, its buffer's first [`PREFIX_LEN`] bytes
    /// already baked to [`PREFIX`]'s image — a wasm data segment, filled
    /// once at compile time, never touched at runtime. Call [`Self::start`]
    /// before adding entries.
    #[must_use]
    pub const fn new() -> Self {
        let mut buf = [0u8; MAX_LEN];
        codec::write_const_bytes(&mut buf, 0, &PREFIX.0);
        MintTxn {
            buf,
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
        // did. This whole `push`/`push_field_header` chain gets
        // force-inlined at every call site inside
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
    /// [`codec::field_header`] — this covers ordinary fields and the
    /// `GenesisMint`/`GenesisMints` object/array-start markers uniformly,
    /// since all three are just a header whose `(type, field)` fall in
    /// those ranges). Takes the header already computed (the `HDR_*`
    /// constants below) rather than deriving one from an `sfXxx` constant
    /// at runtime: `field_header`'s range checks are compile-time
    /// assertions in a `const` context, but at runtime compile to a
    /// genuine (unreachable-in-practice) panic path whose message
    /// formatting blows this hook's nesting budget once inlined. Every
    /// header this module writes is for a compile-time-constant field, so
    /// precomputing costs nothing either way.
    #[inline(always)]
    fn push_field_header(&mut self, header: ([u8; 3], usize)) -> usize {
        let (hdr, hdr_len) = header;
        match hdr_len {
            1 => self.push(&[hdr[0]]),
            2 => self.push(&[hdr[0], hdr[1]]),
            _ => self.push(&[hdr[0], hdr[1], hdr[2]]),
        }
    }

    /// Positions the cursor past the transaction's fixed-shape header, up
    /// to and including `Account` — already baked into `buf` by
    /// [`Self::new`] (see [`PREFIX`]), so nothing is written here: the
    /// placeholder `FirstLedgerSequence`/`LastLedgerSequence`/`Fee` offsets
    /// [`Self::finish`] patches are [`PREFIX`]'s own offsets, recorded
    /// alongside the image so they cannot drift from it.
    pub fn start(&mut self) {
        self.len = PREFIX_LEN;
        self.fls_offset = PREFIX.1.first_ledger_sequence_off;
        self.lls_offset = PREFIX.1.last_ledger_sequence_off;
        self.fee_offset = PREFIX.1.fee_off;
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
