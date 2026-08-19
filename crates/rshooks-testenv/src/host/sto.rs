//! `sto_*` semantics (P2-D — `.claude/design/TESTENV_PHASE2_DESIGN.md` §4
//! "sto_*", stage plan §7): pure byte-level STObject surgery over caller
//! buffers, extending `crate::emit_walk`'s canonical field walker to expose
//! offsets rather than forking it. Empty scaffolding as of P2-A:
//! `crate::backend::Backend` does not yet override `sto_subfield`/
//! `sto_subarray`/`sto_validate`/`sto_emplace`/`sto_erase`, so every call
//! still falls through to the trait's own `NOT_IMPLEMENTED` default.
