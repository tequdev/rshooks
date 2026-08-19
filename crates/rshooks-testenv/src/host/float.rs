//! `float_*`/`slot_float` semantics (P2-B —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "float_*", stage plan §7).
//! Empty scaffolding as of P2-A: `crate::backend::Backend` does not yet
//! override any `HostBackend` `float_*`/`float_sto`/`float_sto_set`/
//! `slot_float` method, so every one of those calls still falls through to
//! the trait's own `NOT_IMPLEMENTED` default.
