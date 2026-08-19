//! Builds the exact `EmitDetails` STObject bytes `etxn_details()` hands
//! back — the same bytes [`crate::emit_walk`] later requires an emitted
//! blob's `EmitDetails` field to match byte-for-byte (design §5.6).
//!
//! Field layout (canonical `(type, field)` order, mirroring xahaud's real
//! `HookAPI::etxn_details`): `EmitGeneration` (UInt32), `EmitBurden`
//! (UInt64), `EmitParentTxnID`/`EmitNonce`/`EmitHookHash` (Hash256, in that
//! field order), and — only when this hook exports a `cbak` — `EmitCallback`
//! (AccountID). Every header is derived from the real, generated `sfXxx`
//! constants via [`rshooks::txn::codec::field_header`] rather than
//! hand-transcribed, so this stays correct if those codes ever change.
//! Total length matches [`rshooks::types::EMIT_DETAILS_MAX_LEN`]'s own
//! documented 116/138-byte split (see this module's tests).

use std::vec::Vec;

use rshooks::sfield::{
    sfEmitBurden, sfEmitCallback, sfEmitDetails, sfEmitGeneration, sfEmitHookHash, sfEmitNonce,
    sfEmitParentTxnID,
};
use rshooks::txn::codec::field_header;
use rshooks::types::SField;

/// The STObject terminator every nested object's serialization ends with
/// (`sfObjectEndMarker`'s wire byte: type 14, field 1, both `< 16` → a
/// single byte `(14 << 4) | 1`).
pub(crate) const OBJECT_END_MARKER: u8 = 0xE1;

/// The inputs this invocation's `etxn_details()` computes from — see
/// [`crate::backend::Backend::etxn_details`].
pub(crate) struct EmitDetailsInputs {
    pub(crate) generation: u32,
    pub(crate) burden: u64,
    pub(crate) parent_txn_id: [u8; 32],
    pub(crate) nonce: [u8; 32],
    pub(crate) hook_hash: [u8; 32],
    pub(crate) callback: Option<[u8; 20]>,
}

fn push_field_header<T>(buf: &mut Vec<u8>, f: SField<T>) {
    let (hdr, len) = field_header(f);
    if let Some(slice) = hdr.get(..len) {
        buf.extend_from_slice(slice);
    }
}

/// Builds the exact `EmitDetails` bytes for one `etxn_details()` call.
pub(crate) fn build_etxn_details(inputs: &EmitDetailsInputs) -> Vec<u8> {
    let mut inner = Vec::new();

    push_field_header(&mut inner, sfEmitGeneration);
    inner.extend_from_slice(&inputs.generation.to_be_bytes());

    push_field_header(&mut inner, sfEmitBurden);
    inner.extend_from_slice(&inputs.burden.to_be_bytes());

    push_field_header(&mut inner, sfEmitParentTxnID);
    inner.extend_from_slice(&inputs.parent_txn_id);

    push_field_header(&mut inner, sfEmitNonce);
    inner.extend_from_slice(&inputs.nonce);

    push_field_header(&mut inner, sfEmitHookHash);
    inner.extend_from_slice(&inputs.hook_hash);

    if let Some(callback) = inputs.callback {
        push_field_header(&mut inner, sfEmitCallback);
        // AccountID is VL-encoded: a 1-byte canonical length prefix (20 fits
        // the single-byte form) followed by the 20-byte payload.
        inner.push(20u8);
        inner.extend_from_slice(&callback);
    }

    let mut out = Vec::new();
    push_field_header(&mut out, sfEmitDetails);
    out.extend_from_slice(&inner);
    out.push(OBJECT_END_MARKER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshooks::types::EMIT_DETAILS_MAX_LEN;

    #[test]
    fn without_callback_matches_documented_116_bytes() {
        let bytes = build_etxn_details(&EmitDetailsInputs {
            generation: 1,
            burden: 1,
            parent_txn_id: [0u8; 32],
            nonce: [0u8; 32],
            hook_hash: [0u8; 32],
            callback: None,
        });
        assert_eq!(bytes.len(), 116);
        assert!(bytes.len() <= EMIT_DETAILS_MAX_LEN);
    }

    #[test]
    fn with_callback_matches_documented_138_bytes() {
        let bytes = build_etxn_details(&EmitDetailsInputs {
            generation: 1,
            burden: 1,
            parent_txn_id: [0u8; 32],
            nonce: [0u8; 32],
            hook_hash: [0u8; 32],
            callback: Some([0u8; 20]),
        });
        assert_eq!(bytes.len(), 138);
        assert_eq!(bytes.len(), EMIT_DETAILS_MAX_LEN);
    }

    #[test]
    fn ends_with_the_object_end_marker() {
        let bytes = build_etxn_details(&EmitDetailsInputs {
            generation: 0,
            burden: 0,
            parent_txn_id: [0u8; 32],
            nonce: [0u8; 32],
            hook_hash: [0u8; 32],
            callback: None,
        });
        assert_eq!(bytes.last(), Some(&OBJECT_END_MARKER));
    }

    #[test]
    fn distinct_inputs_produce_distinct_bytes() {
        let a = build_etxn_details(&EmitDetailsInputs {
            generation: 1,
            burden: 1,
            parent_txn_id: [1u8; 32],
            nonce: [2u8; 32],
            hook_hash: [3u8; 32],
            callback: None,
        });
        let b = build_etxn_details(&EmitDetailsInputs {
            generation: 2,
            burden: 1,
            parent_txn_id: [1u8; 32],
            nonce: [2u8; 32],
            hook_hash: [3u8; 32],
            callback: None,
        });
        assert_ne!(a, b);
    }
}
