//! `util_sha512h`/`util_accid`/`util_raddr`/`util_verify`/`ledger_keylet`
//! semantics (P2-C — `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "util_*
//! and ledger_keylet", stage plan §7). `util_keylet` itself lives in
//! [`super::keylet`], not here (§0's family table lists it under
//! "util (5)", but the implementation is large enough on its own,
//! 26 keylet types plus [`rshooks_core::backend::KeyletArg`] resolution,
//! to warrant its own module). Empty scaffolding as of P2-A:
//! `crate::backend::Backend` does not yet override any of these
//! `HostBackend` methods, so every call still falls through to the
//! trait's own `NOT_IMPLEMENTED` default.
