//! Blob builders shared by more than one module's `#[cfg(test)]` suite
//! (`crate::emit_walk`, `crate::host::sto`) — kept in one place instead of
//! copied per module.

use std::vec::Vec;

/// `sfSequence(2,4) = v`.
pub(crate) fn sf_sequence_bytes(v: u32) -> Vec<u8> {
    let mut out = vec![0x24]; // (type 2, field 4)
    out.extend_from_slice(&v.to_be_bytes());
    out
}

/// `sfFlags(2,2) = v`.
pub(crate) fn sf_flags_bytes(v: u32) -> Vec<u8> {
    let mut out = vec![0x22]; // (type 2, field 2)
    out.extend_from_slice(&v.to_be_bytes());
    out
}

/// A single field nested `depth` levels deep (an empty innermost object):
/// `depth` copies of the `(type 14, field 2)` header opening one level
/// each, followed by `depth` `0xE1` (`OBJECT_END_MARKER`) terminators
/// closing them back out. Matches real xahaud's `get_stobject_length`
/// recursion-depth convention (`HookAPI.cpp:2901`; see also
/// `crate::emit_walk::STO_MAX_RECURSION_DEPTH`'s doc comment).
pub(crate) fn nested_object_chain(depth: u32) -> Vec<u8> {
    let depth = depth as usize;
    let mut out = vec![0xE2u8; depth]; // (type 14, field 2), repeated
    out.extend(vec![0xE1u8; depth]); // OBJECT_END_MARKER, repeated
    out
}
