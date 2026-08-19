//! Per-family scaffolding for the Phase 2 [`crate::backend::Backend`]
//! implementation (`.claude/design/TESTENV_PHASE2_DESIGN.md` §2, stage
//! P2-A).
//!
//! Each submodule here is where its family's `HostBackend` overrides land
//! in a later stage (P2-B..P2-E): `float.rs` (P2-B, landed), `util.rs`/
//! `keylet.rs` (P2-C), `slots.rs`/`sto.rs` (P2-D), `control.rs` (P2-E). A
//! landed family gains free functions the `impl HostBackend for Backend`
//! block in `backend.rs` delegates to in one line each (see that file's own
//! module doc comment for why the delegating block itself must stay thin)
//! and its submodule here is `pub(crate)` so `backend.rs` can reach it; an
//! unlanded family stays private and empty (module doc comment only,
//! `impl HostBackend for Backend` keeps returning the trait's own
//! `NOT_IMPLEMENTED` default for its methods) — no dead-code warnings
//! either way.

mod control;
pub(crate) mod float;
mod keylet;
mod slots;
mod sto;
mod util;
