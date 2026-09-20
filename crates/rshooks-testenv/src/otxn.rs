//! [`Otxn`]: builds the originating transaction a [`crate::TestEnv`] seeds
//! its invocations with.

use std::collections::HashMap;
use std::vec::Vec;

use rshooks::tx_type::TxType;
use rshooks::txn::codec::encode_native_amount_const;
use rshooks::txn::codec::sti::{STI_ACCOUNT, STI_VL};

/// The originating transaction a [`crate::TestEnv`] seeds its invocations
/// with — backs `otxn_field`/`otxn_type`/`otxn_id`/`otxn_param`. Every field
/// is stored as its **raw value bytes** (what a real `otxn_field` call
/// would write into a caller buffer — no STObject header, no VL length
/// prefix), keyed by the field's `sfXxx` code.
#[derive(Debug, Clone)]
pub struct Otxn {
    pub(crate) tx_type: TxType,
    pub(crate) fields: HashMap<u32, Vec<u8>>,
    pub(crate) params: HashMap<Vec<u8>, Vec<u8>>,
    pub(crate) id: [u8; 32],
}

impl Otxn {
    /// Starts a new originating transaction of type `tx_type`. Every field
    /// is absent (`otxn_field` returns `DOESNT_EXIST`) until set.
    #[must_use]
    pub fn new(tx_type: TxType) -> Self {
        Self {
            tx_type,
            fields: HashMap::new(),
            params: HashMap::new(),
            id: [0u8; 32],
        }
    }

    /// Sets `sfAccount` (the transaction's sender).
    #[must_use]
    pub fn account(mut self, acc: [u8; 20]) -> Self {
        self.fields
            .insert(rshooks::sfield::sfAccount.code(), acc.to_vec());
        self
    }

    /// Sets `sfDestination`.
    #[must_use]
    pub fn destination(mut self, acc: [u8; 20]) -> Self {
        self.fields
            .insert(rshooks::sfield::sfDestination.code(), acc.to_vec());
        self
    }

    /// Sets `sfAmount` to a native (XRP/XAH) amount of `drops`.
    ///
    /// # Panics
    ///
    /// Panics if `drops >=`[`rshooks::txn::codec::MAX_NATIVE_DROPS`] (via
    /// [`encode_native_amount_const`]) — a test-author bug to fix, not
    /// something worth threading a `Result` through every chainable `Otxn`
    /// method for.
    #[must_use]
    pub fn amount_drops(mut self, drops: u64) -> Self {
        self.fields.insert(
            rshooks::sfield::sfAmount.code(),
            encode_native_amount_const(drops).to_vec(),
        );
        self
    }

    /// Escape hatch: sets an arbitrary field's raw value bytes directly, by
    /// its `sfXxx` code (`rshooks::raw::sfcodes::sfXxx`, or
    /// `rshooks::sfield::sfXxx.code()`).
    #[must_use]
    pub fn field_raw(mut self, sfield: u32, bytes: &[u8]) -> Self {
        self.fields.insert(sfield, bytes.to_vec());
        self
    }

    /// Sets a Hook API parameter attached to this originating transaction
    /// (read back via `otxn_param`).
    #[must_use]
    pub fn param(mut self, name: &[u8], value: &[u8]) -> Self {
        self.params.insert(name.to_vec(), value.to_vec());
        self
    }

    /// Sets this transaction's ID (hash), returned by `otxn_id`.
    #[must_use]
    pub fn id(mut self, hash: [u8; 32]) -> Self {
        self.id = hash;
        self
    }
}

/// Builds the canonical serialized field sequence [`crate::host::slots::otxn_slot`]
/// loads into a root slot (P2-D, `.claude/design/TESTENV_PHASE2_DESIGN.md`
/// §4 "slot family"): every seeded field in `otxn.fields`, plus a
/// synthesized `sfTransactionType` from `otxn.tx_type` unless `field_raw`
/// already set an override, in canonical `(type, field)` order. A VL
/// length-prefix is added for `STI_VL`(7)/`STI_ACCOUNT`(8) fields since
/// [`Otxn::fields`] stores value-only bytes; every other type is written
/// as-is. No wrapping header or terminator (a root object's own shape, see
/// `crate::host::slots`' module doc comment). [`deserialize`] is the
/// inverse.
pub(crate) fn serialize(otxn: &Otxn) -> Vec<u8> {
    let mut all: Vec<(u32, Vec<u8>)> = otxn.fields.iter().map(|(k, v)| (*k, v.clone())).collect();
    let tt_code = rshooks::sfield::sfTransactionType.code();
    if !otxn.fields.contains_key(&tt_code) {
        all.push((tt_code, otxn.tx_type.code().to_be_bytes().to_vec()));
    }
    all.sort_by_key(|(code, _)| *code);

    let mut out = Vec::new();
    for (code, value) in all {
        write_field(&mut out, code, &value);
    }
    out
}

/// Writes the 1/2/3-byte STObject field header for `(type, field)`
/// (mirrors `rshooks::txn::codec::field_header`'s 4-case grammar;
/// duplicated here because that function needs a typed `SField<T>` and this
/// serializer works from raw stored codes) — also the header-only case
/// `crate::host::float::write_field_header` wraps for `HookAPI::float_sto`'s
/// identical layout, adding only its native/"short" no-header sentinels.
pub(crate) fn write_field_header(out: &mut Vec<u8>, ty: u32, field: u32) {
    if ty < 16 && field < 16 {
        out.push(((ty << 4) | field) as u8);
    } else if ty < 16 {
        out.push((ty << 4) as u8);
        out.push(field as u8);
    } else if field < 16 {
        out.push((field << 4) as u8);
        out.push(ty as u8);
    } else {
        out.push(0);
        out.push(ty as u8);
        out.push(field as u8);
    }
}

/// Writes one field's header plus its wire value. Also reused by
/// `crate::backend::Backend::prepare` (P2-D) to build field bytes
/// `crate::host::sto::sto_emplace` needs for `Sequence`/`SigningPubKey`/
/// `Account`/`FirstLedgerSequence`/`LastLedgerSequence`/`Fee`.
pub(crate) fn write_field(out: &mut Vec<u8>, code: u32, value: &[u8]) {
    let ty = code >> 16;
    let field = code & 0xFFFF;
    write_field_header(out, ty, field);
    if ty == STI_VL || ty == STI_ACCOUNT {
        write_vl_len(out, value.len());
    }
    out.extend_from_slice(value);
}

/// Encodes a VL length prefix — the inverse of
/// `crate::emit_walk::decode_vl_len`'s three-case grammar.
fn write_vl_len(out: &mut Vec<u8>, len: usize) {
    if len <= 192 {
        out.push(len as u8);
    } else if len <= 12480 {
        let adj = len.wrapping_sub(193);
        out.push(193usize.wrapping_add(adj / 256) as u8);
        out.push((adj % 256) as u8);
    } else {
        let adj = len.wrapping_sub(12481);
        out.push(241usize.wrapping_add(adj / 65536) as u8);
        out.push(((adj / 256) % 256) as u8);
        out.push((adj % 256) as u8);
    }
}

/// The wire bytes a `slot()`/`otxn_field` write-out returns for one field's
/// stored value-only bytes (`Otxn::fields`' and a numbered slot's
/// `SlotEntry::bytes` share this convention). Real xahaud's wrapper
/// serializes the field's own value with a bare `add(s)` and then, only for
/// `STI_ACCOUNT`(8), skips that call's leading VL byte before writing it out
/// (`applyHook.cpp:1873-1878` for `otxn_field`, `:1915-1920` for `slot`, both
/// driven by the same `WRITE_WASM_MEMORY_OR_RETURN_AS_INT64` macro's final
/// `getSType() == STI_ACCOUNT` argument) — so an `STI_ACCOUNT` value's
/// already-prefix-free stored bytes come back unchanged, while an
/// `STI_VL`(7)/Blob value's `add(s)` keeps its length prefix
/// (`STBlob::add`), which is added back here since the stored bytes are
/// value-only. Every other type has no VL concept and is returned as-is.
pub(crate) fn value_wire_bytes(field_code: u32, value: &[u8]) -> Vec<u8> {
    if field_code >> 16 == STI_VL {
        let mut out = Vec::with_capacity(value.len().wrapping_add(3));
        write_vl_len(&mut out, value.len());
        out.extend_from_slice(value);
        out
    } else {
        value.to_vec()
    }
}

/// The full `entry->add(s)` length `HookAPI::slot_size` reports for one
/// field's stored value-only bytes (`HookAPI.cpp:2143-2156`): unlike
/// [`value_wire_bytes`], this is never stripped for `STI_ACCOUNT`, since
/// `slot_size` computes `add(s)`'s length directly and has no
/// `STI_ACCOUNT`-skipping macro in its path — `value_len` plus a VL
/// length-prefix's own byte count for both `STI_VL`(7) and `STI_ACCOUNT`(8)
/// (both wire types are `addVL`-serialized), plain `value_len` for every
/// other type.
pub(crate) fn wire_add_len(field_code: u32, value_len: usize) -> usize {
    match field_code >> 16 {
        ty if ty == STI_VL || ty == STI_ACCOUNT => {
            let mut prefix = Vec::new();
            write_vl_len(&mut prefix, value_len);
            prefix.len().wrapping_add(value_len)
        }
        _ => value_len,
    }
}

/// Parses a root field sequence (as [`serialize`] produces, any other
/// well-formed root slot's content, or a raw NOP-padded blob a test
/// supplies directly) back into a field map — the inverse of [`serialize`].
/// `None` on any parse failure. `pub(crate)` rather than an `Otxn`
/// constructor: P2-E's `cbak` harness (design §4 "cbak execution")
/// reconstructs an `Otxn`-shaped field map for the emitted transaction
/// passed into a callback.
///
/// [`crate::emit_walk::canonicalize`]s `bytes` first, so every stored
/// field value — including a nested `STI_OBJECT`(14)/`STI_ARRAY`(15)
/// field's own bytes, e.g. an incoming `Remit`'s `sfAmounts` array — is
/// NOP-free at every depth by the time it lands in the returned map. This
/// matters even when [`bytes`] itself already came from a NOP-free
/// [`crate::world::EmittedTxn::blob`] (`crate::backend::Backend::emit`
/// stores canonically too): keeping the canonicalization here as well
/// means this function's own contract — "the returned field values are
/// always safe to feed to the strict `sto_*`/`slot_*` family" — does not
/// silently depend on every caller already having canonicalized upstream.
pub(crate) fn deserialize(bytes: &[u8]) -> Option<std::collections::HashMap<u32, Vec<u8>>> {
    let canonical = crate::emit_walk::canonicalize(bytes).ok()?;
    let fields =
        crate::emit_walk::walk_top_level_fields(&canonical, crate::emit_walk::NopMode::Strict)
            .ok()?;
    let mut map = std::collections::HashMap::new();
    for f in &fields {
        let (start, end) = crate::emit_walk::field_value_payload(&canonical, f).ok()?;
        let value = canonical.get(start..end)?.to_vec();
        map.insert(f.code as u32, value);
    }
    Some(map)
}

/// Reconstructs the originating-transaction view a `#[cbak]` execution sees
/// (P2-E, design §4 "cbak execution"): `blob` (an emitted transaction,
/// already validated by the emission walker before becoming an
/// [`crate::world::EmittedTxn`]) becomes the callback's otxn, and its
/// `EmitDetails.EmitGeneration`/`EmitBurden` fields become the
/// `(burden, generation)` pair `crate::env::TestEnv::invoke_cbak` seeds
/// `World::otxn_emitted` with. This matches `HookAPI::otxn_burden`/
/// `HookAPI::otxn_generation` (`Xahau/xahaud` `dev`,
/// `src/xrpld/app/hook/detail/HookAPI.cpp:1465-1520`), which read those two
/// `EmitDetails` fields directly off `hookCtx.applyCtx.tx` — during
/// `Transactor::doHookCallback` (`src/xrpld/app/tx/detail/Transactor.cpp:1483-1614`)
/// *is* the emitted transaction itself — rather than incrementing anything,
/// unlike `etxn_burden`/`etxn_generation`'s own `× reserved`/`+ 1`
/// derivation for the *next* emission (`crate::backend::Backend::compute_etxn_burden`/
/// `compute_etxn_generation`, unchanged by this function).
///
/// `id(hash)` is set to `hash` (the emit-returned hash): a real callback's
/// `otxn_id` returns `getTransactionID()` by default (`HookAPI.cpp:1545-1551`)
/// — the emitted transaction's own hash, exactly what `emit` returned.
///
/// `None` on any parse failure (malformed field sequence, missing
/// `TransactionType`, missing/malformed `EmitDetails` or its
/// `EmitGeneration`/`EmitBurden`/`EmitHookHash` sub-fields); a caller maps
/// that to a clear panic. Should not occur for any blob from
/// `crate::TestEnv::emitted()`, since those already passed the emission
/// walker's own `EmitDetails` well-formedness checks
/// ([`crate::emit_walk::validate_emit_blob`]).
pub(crate) struct EmittedOtxn {
    pub(crate) otxn: Otxn,
    pub(crate) burden: u64,
    pub(crate) generation: u32,
    /// `EmitDetails.EmitHookHash` — the hash of the hook that performed the
    /// emit.
    pub(crate) hook_hash: [u8; 32],
    /// `EmitDetails.EmitCallback`, if present — the account
    /// `Transactor::doHookCallback` (`Xahau/xahaud` `dev`,
    /// `src/xrpld/app/tx/detail/Transactor.cpp:1483-1614`) looks up a
    /// callback hook on. `None` means the emitting hook declared no
    /// `#[cbak]` body, so on-chain this transaction never triggers a
    /// callback at all.
    pub(crate) callback_account: Option<[u8; 20]>,
}

pub(crate) fn from_emitted(blob: &[u8], hash: [u8; 32]) -> Option<EmittedOtxn> {
    let map = deserialize(blob)?;

    let tt_bytes = map.get(&rshooks::sfield::sfTransactionType.code())?;
    let tt_code = u16::from_be_bytes(tt_bytes.as_slice().try_into().ok()?);
    let mut otxn = Otxn::new(TxType::from(tt_code)).id(hash);
    for (code, value) in &map {
        otxn = otxn.field_raw(*code, value);
    }

    let ed_bytes = map.get(&rshooks::sfield::sfEmitDetails.code())?;
    // `ed_bytes` came out of `deserialize`'s canonicalized map, so it is
    // already NOP-free — `Strict` here is both correct and a defensive
    // assertion of that invariant (see `crate::emit_walk::NopMode::Strict`'s
    // doc comment).
    let ed_fields = crate::emit_walk::walk_top_level_fields_or_object(
        ed_bytes,
        true,
        crate::emit_walk::NopMode::Strict,
    )
    .ok()?;

    let generation_code = u64::from(rshooks::sfield::sfEmitGeneration.code());
    let burden_code = u64::from(rshooks::sfield::sfEmitBurden.code());
    let hook_hash_code = u64::from(rshooks::sfield::sfEmitHookHash.code());
    let callback_code = u64::from(rshooks::sfield::sfEmitCallback.code());

    let generation_field = ed_fields.iter().find(|f| f.code == generation_code)?;
    let burden_field = ed_fields.iter().find(|f| f.code == burden_code)?;
    let hook_hash_field = ed_fields.iter().find(|f| f.code == hook_hash_code)?;
    let (gs, ge) = crate::emit_walk::field_value_payload(ed_bytes, generation_field).ok()?;
    let (bs, be) = crate::emit_walk::field_value_payload(ed_bytes, burden_field).ok()?;
    let (hs, he) = crate::emit_walk::field_value_payload(ed_bytes, hook_hash_field).ok()?;
    let generation = u32::from_be_bytes(ed_bytes.get(gs..ge)?.try_into().ok()?);
    let burden = u64::from_be_bytes(ed_bytes.get(bs..be)?.try_into().ok()?);
    let hook_hash: [u8; 32] = ed_bytes.get(hs..he)?.try_into().ok()?;

    let callback_account = match ed_fields.iter().find(|f| f.code == callback_code) {
        Some(callback_field) => {
            let (cs, ce) = crate::emit_walk::field_value_payload(ed_bytes, callback_field).ok()?;
            let acc: [u8; 20] = ed_bytes.get(cs..ce)?.try_into().ok()?;
            Some(acc)
        }
        None => None,
    };

    Some(EmittedOtxn {
        otxn,
        burden,
        generation,
        hook_hash,
        callback_account,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    #[test]
    fn account_sets_raw_field_bytes() {
        let otxn = Otxn::new(TxType::Payment).account([7u8; 20]);
        assert_eq!(
            otxn.fields.get(&rshooks::sfield::sfAccount.code()),
            Some(&[7u8; 20].to_vec())
        );
    }

    #[test]
    fn amount_drops_sets_native_amount_encoding() {
        let otxn = Otxn::new(TxType::Payment).amount_drops(1);
        let bytes = otxn.fields.get(&rshooks::sfield::sfAmount.code()).unwrap();
        assert_eq!(bytes, &vec![0x40, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn field_raw_is_a_general_escape_hatch() {
        let otxn =
            Otxn::new(TxType::Payment).field_raw(rshooks::sfield::sfSequence.code(), &[0, 0, 0, 5]);
        assert_eq!(
            otxn.fields.get(&rshooks::sfield::sfSequence.code()),
            Some(&vec![0, 0, 0, 5])
        );
    }

    #[test]
    fn param_and_id_are_stored() {
        let otxn = Otxn::new(TxType::Payment).param(b"K", b"V").id([9u8; 32]);
        assert_eq!(otxn.params.get(b"K".as_slice()), Some(&b"V".to_vec()));
        assert_eq!(otxn.id, [9u8; 32]);
    }

    #[test]
    #[should_panic(expected = "native_amount default does not fit in 62 bits")]
    fn amount_drops_panics_at_or_above_max_native_drops() {
        let _ = Otxn::new(TxType::Payment).amount_drops(1u64 << 62);
    }

    // -- serialize/deserialize (P2-D) --

    #[test]
    fn serialize_is_a_bare_canonical_field_sequence() {
        let otxn = Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .destination([1u8; 20])
            .amount_drops(1_000_000);
        let bytes = serialize(&otxn);
        // No wrapping header/terminator: walkable as a top-level field
        // sequence, and the walk must consume every byte.
        let fields =
            crate::emit_walk::walk_top_level_fields(&bytes, crate::emit_walk::NopMode::Tolerant)
                .unwrap();
        // TransactionType (synthesized) + Amount + Account + Destination.
        assert_eq!(fields.len(), 4);
        // Canonical (type, field) order: TransactionType(1,2) < Amount(6,1)
        // < Account(8,1) < Destination(8,3).
        let codes: Vec<u64> = fields.iter().map(|f| f.code).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        assert_eq!(codes, sorted);
        assert!(codes.is_sorted());
    }

    #[test]
    fn serialize_synthesizes_transaction_type_unless_overridden() {
        let otxn = Otxn::new(TxType::Payment);
        let bytes = serialize(&otxn);
        let map = deserialize(&bytes).unwrap();
        let tt = map.get(&rshooks::sfield::sfTransactionType.code()).unwrap();
        assert_eq!(tt, &TxType::Payment.code().to_be_bytes().to_vec());

        // An explicit `field_raw` override wins over the synthesized value.
        let overridden = Otxn::new(TxType::Payment)
            .field_raw(rshooks::sfield::sfTransactionType.code(), &[0, 8]);
        let bytes2 = serialize(&overridden);
        let map2 = deserialize(&bytes2).unwrap();
        assert_eq!(
            map2.get(&rshooks::sfield::sfTransactionType.code()),
            Some(&vec![0u8, 8])
        );
    }

    #[test]
    fn serialize_then_deserialize_round_trips_every_seeded_field() {
        let otxn = Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .destination([1u8; 20])
            .amount_drops(42);
        let bytes = serialize(&otxn);
        let map = deserialize(&bytes).unwrap();

        assert_eq!(
            map.get(&rshooks::sfield::sfAccount.code()),
            Some(&vec![2u8; 20])
        );
        assert_eq!(
            map.get(&rshooks::sfield::sfDestination.code()),
            Some(&vec![1u8; 20])
        );
        assert_eq!(
            map.get(&rshooks::sfield::sfAmount.code()),
            otxn.fields.get(&rshooks::sfield::sfAmount.code())
        );
    }

    #[test]
    fn serialize_vl_field_round_trips_through_field_raw() {
        // A VL-typed field (sfSigningPubKey, type 7) seeded with 3 raw
        // value bytes must come back with the VL length-prefix added on
        // the wire, then stripped again by `deserialize`.
        let otxn = Otxn::new(TxType::Payment)
            .field_raw(rshooks::sfield::sfSigningPubKey.code(), &[9, 9, 9]);
        let bytes = serialize(&otxn);
        // The field's own value bytes on the wire are `<len=3><9,9,9>`.
        let fields =
            crate::emit_walk::walk_top_level_fields(&bytes, crate::emit_walk::NopMode::Tolerant)
                .unwrap();
        let spk = fields
            .iter()
            .find(|f| f.code == u64::from(rshooks::sfield::sfSigningPubKey.code()))
            .unwrap();
        assert_eq!(
            bytes.get(spk.value_range.0..spk.value_range.1).unwrap(),
            &[3, 9, 9, 9]
        );
        let map = deserialize(&bytes).unwrap();
        assert_eq!(
            map.get(&rshooks::sfield::sfSigningPubKey.code()),
            Some(&vec![9u8, 9, 9])
        );
    }

    #[test]
    fn deserialize_rejects_malformed_bytes() {
        assert_eq!(deserialize(&[0xE2]), None); // truncated
    }

    #[test]
    fn deserialize_canonicalizes_a_nested_nop_supplied_directly() {
        // A top-level field sequence — sfTransactionType(0), then a
        // nested STObject field (type 14, field 2) whose own body is one
        // scalar field followed by a NOP, before its own `0xE1` — built
        // by hand rather than through `serialize`/`Backend::emit`: exactly
        // the "raw NOP-padded blob a test supplies directly" case
        // `deserialize`'s own canonicalization (independent of
        // `Backend::emit`'s) exists for.
        let mut bytes = vec![0x12, 0x00, 0x00]; // sfTransactionType = 0
        bytes.push(0xE2); // (type 14, field 2) nested object header
        bytes.extend_from_slice(&[0x24, 0, 0, 0, 7]); // sfSequence = 7
        bytes.push(0x99); // NOP inside the nested object, before its own 0xE1
        bytes.push(0xE1); // nested object terminator

        let map = deserialize(&bytes).expect("tolerant parse of a NOP-padded blob");
        let nested_code = (14u32 << 16) | 2;
        let nested_value = map.get(&nested_code).expect("nested field present");
        // The map's entry is the nested field's *value* (payload,
        // terminator included — `field_value_payload`'s STI_OBJECT
        // convention): it must be exactly the NOP-free bytes, proving the
        // NOP genuinely present in `bytes` did not survive into the map
        // `deserialize` returns.
        assert_eq!(nested_value, &vec![0x24, 0, 0, 0, 7, 0xE1]);
    }
}
