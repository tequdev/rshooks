//! Encodes governance `Invoke` and `HookSet` emissions.

use rshooks::prelude::*;
use rshooks::static_cell::HookStatic;
use rshooks::txn::codec;
use rshooks::*;

/// Rolls back an emission failure.
#[inline(always)]
fn fail(msg: &[u8]) -> ! {
    rollback!(msg, -204);
}

const HDR_TRANSACTION_TYPE: ([u8; 3], usize) = codec::field_header(sfTransactionType);
const HDR_FLAGS: ([u8; 3], usize) = codec::field_header(sfFlags);
const HDR_SEQUENCE: ([u8; 3], usize) = codec::field_header(sfSequence);
const HDR_FIRST_LEDGER_SEQUENCE: ([u8; 3], usize) = codec::field_header(sfFirstLedgerSequence);
const HDR_LAST_LEDGER_SEQUENCE: ([u8; 3], usize) = codec::field_header(sfLastLedgerSequence);
const HDR_FEE: ([u8; 3], usize) = codec::field_header(sfFee);
const HDR_SIGNING_PUB_KEY: ([u8; 3], usize) = codec::field_header(sfSigningPubKey);
const HDR_ACCOUNT: ([u8; 3], usize) = codec::field_header(sfAccount);
const HDR_DESTINATION: ([u8; 3], usize) = codec::field_header(sfDestination);
const HDR_HOOKS: ([u8; 3], usize) = codec::field_header(sfHooks);
const HDR_HOOK: ([u8; 3], usize) = codec::field_header(sfHook);
const HDR_FLAGS_INNER: ([u8; 3], usize) = codec::field_header(sfFlags);
const HDR_CREATE_CODE: ([u8; 3], usize) = codec::field_header(sfCreateCode);
const HDR_HOOK_HASH: ([u8; 3], usize) = codec::field_header(sfHookHash);

// Compact field-header forms used by the encoders.
const H_TT: u8 = HDR_TRANSACTION_TYPE.0[0];
const H_FLAGS: u8 = HDR_FLAGS.0[0];
const H_SEQUENCE: u8 = HDR_SEQUENCE.0[0];
const H_FLS: [u8; 2] = [
    HDR_FIRST_LEDGER_SEQUENCE.0[0],
    HDR_FIRST_LEDGER_SEQUENCE.0[1],
];
const H_LLS: [u8; 2] = [HDR_LAST_LEDGER_SEQUENCE.0[0], HDR_LAST_LEDGER_SEQUENCE.0[1]];
const H_FEE: u8 = HDR_FEE.0[0];
const H_PUBKEY: u8 = HDR_SIGNING_PUB_KEY.0[0];
const H_ACCOUNT: u8 = HDR_ACCOUNT.0[0];
const H_DESTINATION: u8 = HDR_DESTINATION.0[0];
/// `tfCANONICAL`'s 4 big-endian bytes.
const TF_CANONICAL_BE: [u8; 4] = tfCANONICAL.to_be_bytes();

const OBJECT_END: u8 = 0xE1;
const ARRAY_END: u8 = 0xF1;
/// `hsfOVERRIDE`.
const HSF_OVERRIDE: u32 = 1;

/// A linear cursor-based transaction-bytes writer, shared by both
/// encoders in this module. See `examples/80_governance/src/mint_txn.rs`'s
/// `MintTxn` for the identical idiom and rationale.
struct TxnBuf {
    buf: [u8; 1024],
    len: usize,
}

impl TxnBuf {
    #[inline(always)]
    const fn new() -> Self {
        TxnBuf {
            buf: [0u8; 1024],
            len: 0,
        }
    }

    #[inline(always)]
    fn push<const N: usize>(&mut self, src: &[u8; N]) -> usize {
        let start = self.len;
        let Some(end) = start.checked_add(N) else {
            fail(b"govern: txn buffer overflow");
        };
        let Some(dst) = self.buf.get_mut(start..end) else {
            fail(b"govern: txn buffer overflow");
        };
        for (d, s) in dst.iter_mut().zip(src.iter()) {
            *d = *s;
        }
        self.len = end;
        start
    }

    #[inline(always)]
    fn push_header(&mut self, header: ([u8; 3], usize)) -> usize {
        let (hdr, hdr_len) = header;
        match hdr_len {
            1 => self.push(&[hdr[0]]),
            2 => self.push(&[hdr[0], hdr[1]]),
            _ => self.push(&[hdr[0], hdr[1], hdr[2]]),
        }
    }

    #[inline(always)]
    fn push_u32_field(&mut self, header: ([u8; 3], usize), value: u32) -> usize {
        let offset = self.push_header(header);
        self.push(&value.to_be_bytes());
        offset
    }

    /// Appends `src` (a topic's own vote data: always 8, 20, or 32 bytes
    /// — reward/hook/seat topics respectively). Dispatches to
    /// [`Self::push`]'s fixed-`N` monomorphizations by length instead of
    /// looping over a runtime-length slice directly: the same
    /// "monomorphize per size instead of one runtime-length copy loop"
    /// fix `push`/`push_header` already need (see [`Self::push`]'s doc
    /// comment) applies here too, since `src.len()` is a *value*, not a
    /// compile-time constant, at this call site even though it's always
    /// one of exactly three sizes.
    #[inline(always)]
    fn push_bytes(&mut self, src: &[u8]) {
        match src.len() {
            8 => {
                self.push(&src.first_chunk::<8>().copied().unwrap_or([0u8; 8]));
            }
            20 => {
                self.push(&src.first_chunk::<20>().copied().unwrap_or([0u8; 20]));
            }
            _ => {
                self.push(&src.first_chunk::<32>().copied().unwrap_or([0u8; 32]));
            }
        }
    }

    /// Writes the common header every emitted transaction in this module
    /// shares, up to and including `Account` (and `Destination`, if
    /// `destination` is `Some`) — `TransactionType`, `Flags =
    /// tfCANONICAL`, `Sequence = 0`, placeholder `FirstLedgerSequence`/
    /// `LastLedgerSequence`/`Fee`, an **empty** `SigningPubKey` (2-byte
    /// null-VL form — matching `macro.h`'s `ENCODE_SIGNING_PUBKEY_NULL`,
    /// which both of govern.c's emissions use, unlike reward.c's 35-byte
    /// zero-filled form).
    #[inline(always)]
    fn write_common_header(&mut self, tt: u16, cls: u32, account: &AccountId) -> usize {
        let tt_be = tt.to_be_bytes();
        // TransactionType + Flags(tfCANONICAL) + Sequence(0), combined
        // into one `push` (13 bytes: 3 + 5 + 5) instead of three.
        self.push(&[
            H_TT,
            tt_be[0],
            tt_be[1],
            H_FLAGS,
            TF_CANONICAL_BE[0],
            TF_CANONICAL_BE[1],
            TF_CANONICAL_BE[2],
            TF_CANONICAL_BE[3],
            H_SEQUENCE,
            0,
            0,
            0,
            0,
        ]);
        // FirstLedgerSequence + LastLedgerSequence, combined (12 bytes:
        // 6 + 6). Both are fully known already (`cls` is read before
        // this call), so — unlike `Fee` below — neither needs a
        // placeholder-then-patch round trip.
        let fls = cls.wrapping_add(1).to_be_bytes();
        let lls = cls.wrapping_add(5).to_be_bytes();
        self.push(&[
            H_FLS[0], H_FLS[1], fls[0], fls[1], fls[2], fls[3], H_LLS[0], H_LLS[1], lls[0], lls[1],
            lls[2], lls[3],
        ]);
        // Fee header + 8-byte zero placeholder (patched by `finish_fee`
        // once the total length is known), combined (9 bytes).
        let fee_start = self.push(&[H_FEE, 0, 0, 0, 0, 0, 0, 0, 0]);
        let fee_offset = fee_start.wrapping_add(1);
        // Empty SigningPubKey (2 bytes: header + zero-length VL).
        self.push(&[H_PUBKEY, 0]);
        // Account header + VL(20) + the 20-byte account, combined (22
        // bytes).
        let a = account.0;
        self.push(&[
            H_ACCOUNT, 20, a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8], a[9], a[10],
            a[11], a[12], a[13], a[14], a[15], a[16], a[17], a[18], a[19],
        ]);
        fee_offset
    }

    /// Appends a `Destination` field (header + VL(20) + the 20-byte
    /// account), combined into one `push` call — used only by the
    /// L1-vote-forward `Invoke` (a `HookSet` has no `Destination`).
    #[inline(always)]
    fn push_destination(&mut self, destination: &AccountId) {
        let d = destination.0;
        self.push(&[
            H_DESTINATION,
            20,
            d[0],
            d[1],
            d[2],
            d[3],
            d[4],
            d[5],
            d[6],
            d[7],
            d[8],
            d[9],
            d[10],
            d[11],
            d[12],
            d[13],
            d[14],
            d[15],
            d[16],
            d[17],
            d[18],
            d[19],
        ]);
    }

    /// Patches the placeholder `Fee` field with the actual fee for the
    /// bytes written so far, and returns the final slice.
    #[inline(always)]
    fn finish_fee(&mut self, fee_offset: usize) -> &[u8] {
        let Some(bytes) = self.buf.get(..self.len) else {
            fail(b"govern: txn buffer overflow");
        };
        let fee = match etxn_fee_base(bytes) {
            Ok(f) => f,
            Err(_) => fail(b"govern: could not compute emitted txn fee"),
        };
        let native = encode_native_amount(fee);
        let Some(fee_end) = fee_offset.checked_add(8) else {
            fail(b"govern: txn buffer overflow");
        };
        let Some(fee_dst) = self.buf.get_mut(fee_offset..fee_end) else {
            fail(b"govern: txn buffer overflow");
        };
        for (d, s) in fee_dst.iter_mut().zip(native.iter()) {
            *d = *s;
        }
        let Some(bytes) = self.buf.get(..self.len) else {
            fail(b"govern: txn buffer overflow");
        };
        bytes
    }
}

/// Shared scratch space for whichever one of [`emit_l1_vote_forward`]/
/// [`emit_hookset`] this hook invocation calls (never both — `govern`
/// takes exactly one action per `Invoke`) — a `HookStatic` rather than a
/// stack local for the same reason `examples/80_governance`'s `MINT_TXN`
/// is: a 1024-byte stack buffer is exactly the kind of thing
/// `wasm32v1-none`'s codegen lowers to unguarded `memset`/`memcpy`-style
/// loops (see `examples/README.md`'s "Statics for templates and large
/// buffers").
static TXN_BUF: HookStatic<TxnBuf> = HookStatic::new(TxnBuf::new());

/// Takes [`TXN_BUF`], rolling back if it was somehow already taken (never
/// happens in practice — at most one of this module's two emit functions
/// runs per hook execution).
#[inline(always)]
fn take_txn_buf() -> &'static mut TxnBuf {
    let Some(txn) = TXN_BUF.take() else {
        fail(b"govern: txn buffer already taken");
    };
    txn
}

/// Encodes `drops` as an 8-byte native (XAH) amount — see
/// `examples/80_governance/src/mint_txn.rs::write_native_amount`'s doc
/// comment for why this is hand-written instead of calling
/// `codec::encode_native_amount` directly (that function's own
/// `copy_from_slice` keeps an unreachable-in-practice panic path linked
/// in that this crate's nesting budget can't afford).
#[inline(always)]
fn encode_native_amount(drops: u64) -> [u8; 8] {
    let bytes = drops.to_be_bytes();
    [
        0x40 | (bytes[0] & 0x3F),
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
    ]
}

/// Builds and emits an `Invoke` from this (L2) table to the L1 (genesis)
/// table, forwarding a member's vote on an L1 topic — govern.c's inline
/// hex-constant builder (L401-472). `topic_data` is exactly `topic_size`
/// bytes (the topic's own value, not front-padded).
///
/// # Errors
///
/// Returns `Err(())` if `emit` itself fails (`emit_result != 32`); the
/// caller rolls back with govern.c's own message
/// ("Governance: L1 vote emission failed.").
pub fn emit_l1_vote_forward(
    hook_accid: &AccountId,
    genesis: &AccountId,
    topic_type: u8,
    topic_id: u8,
    topic_size: u8,
    topic_data: &[u8],
) -> bool {
    let txn = take_txn_buf();
    let cls = ledger_seq();
    let fee_offset = txn.write_common_header(ttINVOKE, cls, hook_accid);
    txn.push_destination(genesis);

    // `Parameters`: two `HookParameter`-shaped objects, `T` (2-byte
    // topic type+id) and `V` (the topic's own data) — transcribed
    // verbatim from govern.c's hardcoded hex constants (the exact
    // STObject template for an `Invoke`'s `Parameters` array), not
    // rebuilt from `sfcodes` field-by-field: govern.c itself hand-wrote
    // these bytes rather than using a generic encoder, and this is the
    // most faithful way to reproduce them exactly. Combined into two
    // `push` calls (14 + 10 bytes) rather than one per literal fragment,
    // for the same call-count reason as `write_common_header`.
    txn.push(&[
        0xF0u8, 0x13, 0xE0, 0x17, 0x70, 0x18, 0x01, 0x54, 0x70, 0x19, 0x02, 0x00, topic_type,
        topic_id,
    ]);
    txn.push(&[
        0xE1u8, 0xE0, 0x17, 0x70, 0x18, 0x01, 0x56, 0x70, 0x19, topic_size,
    ]);
    txn.push_bytes(topic_data);
    txn.push(&[OBJECT_END, ARRAY_END]);

    let bytes = txn.finish_fee(fee_offset);
    emit_buf(bytes).is_ok()
}

/// Builds and emits a `HookSet` actioning a hook-topic vote — govern.c's
/// `PREPARE_HOOKSET` macro (`macro.h`). `hash` is `None` for a delete
/// operation (empty `CreateCode`), `Some(hash)` to install that hook
/// definition at position `slot_index` (0..=9); every other position is a
/// no-op (`hsfOVERRIDE` unset, matching `_0E_0E_ENCODE_HOOKOBJ`'s `hook0
/// == 0` branch).
pub fn emit_hookset(hook_accid: &AccountId, slot_index: u8, hash: Option<&[u8; 32]>) -> bool {
    let txn = take_txn_buf();
    let cls = ledger_seq();
    let fee_offset = txn.write_common_header(ttHOOK_SET, cls, hook_accid);

    // `EmitDetails`, reserved worst-case then trimmed to the actually
    // written length — see `mint_txn::MintTxn::write_emit_details`'s
    // twin logic.
    let start = txn.len;
    let Some(end) = start.checked_add(EMIT_DETAILS_MAX_LEN) else {
        fail(b"govern: txn buffer overflow");
    };
    let Some(region) = txn.buf.get_mut(start..end) else {
        fail(b"govern: txn buffer overflow");
    };
    let written = match etxn_details(region) {
        Ok(n) => n,
        Err(_) => fail(b"govern: could not write EmitDetails"),
    };
    let Some(new_len) = start.checked_add(written) else {
        fail(b"govern: txn buffer overflow");
    };
    txn.len = new_len;

    txn.push_header(HDR_HOOKS);
    let mut i = 0u8;
    while i < 10 {
        guard!(10);
        txn.push_header(HDR_HOOK);
        if i == slot_index {
            txn.push_u32_field(HDR_FLAGS_INNER, HSF_OVERRIDE);
            match hash {
                None => {
                    txn.push_header(HDR_CREATE_CODE);
                    txn.push(&[0u8]); // empty VL: delete operation
                }
                Some(h) => {
                    txn.push_header(HDR_HOOK_HASH);
                    txn.push(h);
                }
            }
        }
        txn.push(&[OBJECT_END]);
        i = i.wrapping_add(1);
    }
    txn.push(&[ARRAY_END]);

    let bytes = txn.finish_fee(fee_offset);
    emit_buf(bytes).is_ok()
}
