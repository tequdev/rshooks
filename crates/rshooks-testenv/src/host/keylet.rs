//! `util_keylet` semantics — all 26 keylet types (P2-C —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "util_* + ledger_keylet",
//! stage plan §7), ported byte-for-byte from `examples/13_keylets`'
//! independently computed, e2e-verified layouts. Consumes
//! [`rshooks_core::backend::KeyletArg`]'s already-resolved `Value`/`Bytes`
//! slots — see that type's doc comment and
//! `crates/rshooks/testenv-call-sites.txt`'s header for why the six
//! arguments arrive pre-resolved rather than as raw pointers. Empty
//! scaffolding as of P2-A: `crate::backend::Backend` does not yet override
//! `util_keylet`, so every call still falls through to the trait's own
//! `NOT_IMPLEMENTED` default.
