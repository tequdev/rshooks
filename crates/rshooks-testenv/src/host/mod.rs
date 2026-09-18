//! Per-family scaffolding for the Phase 2 [`crate::backend::Backend`]
//! implementation (`.claude/design/TESTENV_PHASE2_DESIGN.md` §2, stage
//! P2-A). Each submodule holds one family's `HostBackend` overrides,
//! `pub(crate)` so `backend.rs`'s `impl HostBackend for Backend` can
//! delegate to it in one line per function. A function needing `World` (or
//! other `Backend` methods) beyond what its family's pure functions need
//! lives directly in `backend.rs` instead — `slots.rs` is the exception,
//! taking `&World` as an explicit parameter since it's one cohesive family
//! either way (see `host::keylet`/`host::control`/`host::slots`'s own
//! module docs for the per-family split).

pub(crate) mod control;
pub(crate) mod float;
pub(crate) mod keylet;
pub(crate) mod slots;
pub(crate) mod sto;
pub(crate) mod util;
