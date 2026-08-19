//! Per-family scaffolding for the Phase 2 [`crate::backend::Backend`]
//! implementation (`.claude/design/TESTENV_PHASE2_DESIGN.md` §2, stage
//! P2-A).
//!
//! Each submodule here is where its family's `HostBackend` overrides land
//! in a later stage (P2-B..P2-E): `float.rs` (P2-B, landed), `util.rs`/
//! `keylet.rs` (P2-C, landed), `slots.rs`/`sto.rs` (P2-D, landed),
//! `control.rs` (P2-E). A landed family gains free functions the
//! `impl HostBackend for Backend` block in `backend.rs` delegates to in one
//! line each (see that file's own module doc comment for why the
//! delegating block itself must stay thin) and its submodule here is
//! `pub(crate)` so `backend.rs` can reach it; an unlanded family stays
//! private and empty (module doc comment only, `impl HostBackend for
//! Backend` keeps returning the trait's own `NOT_IMPLEMENTED` default for
//! its methods) — no dead-code warnings either way. `ledger_keylet`/
//! `trace_float`/`prepare`/`otxn_slot`/`meta_slot`/`xpop_slot`/`slot_set`
//! land directly in `backend.rs` itself, not (only) a submodule here — see
//! `host::keylet`/`host::control`/`host::slots`' own module doc comments
//! for why (each needs `World` access the rest of its own family's pure
//! functions don't; `slots.rs`'s `World`-needing functions still live in
//! `slots.rs`, taking `&World` as an explicit parameter, since they are
//! part of one cohesive family alongside its `World`-free functions —
//! `prepare` alone needs both `World` *and* other `Backend` methods
//! (`etxn_details`/`etxn_fee_base`), which only `backend.rs` itself has in
//! scope).

mod control;
pub(crate) mod float;
pub(crate) mod keylet;
pub(crate) mod slots;
pub(crate) mod sto;
pub(crate) mod util;
