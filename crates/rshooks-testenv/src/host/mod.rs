//! Per-family scaffolding for the Phase 2 [`crate::backend::Backend`]
//! implementation (`.claude/design/TESTENV_PHASE2_DESIGN.md` §2, stage
//! P2-A).
//!
//! Each submodule here is where its family's `HostBackend` overrides land
//! in a later stage (P2-B..P2-E): `float.rs` (P2-B), `util.rs`/`keylet.rs`
//! (P2-C), `slots.rs`/`sto.rs` (P2-D), `control.rs` (P2-E). P2-A only
//! creates the module skeleton — `impl HostBackend for
//! crate::backend::Backend` (`crates/rshooks-testenv/src/backend.rs`)
//! keeps returning the trait's own `NOT_IMPLEMENTED` defaults for every
//! Phase 2 method; nothing here is wired into that `impl` block yet, so
//! every module below is currently empty (module doc comment only) and
//! produces no dead-code warnings. Once a family lands, its module gains
//! free functions the `impl HostBackend for Backend` block in
//! `backend.rs` delegates to in one line each — see that file's own module
//! doc comment for why the delegating block itself must stay thin.

mod control;
mod float;
mod keylet;
mod slots;
mod sto;
mod util;
