//! Per-family scaffolding for the Phase 2 [`crate::backend::Backend`]
//! implementation (`.claude/design/TESTENV_PHASE2_DESIGN.md` §2, stage
//! P2-A).
//!
//! Each submodule holds one family's `HostBackend` overrides: `float.rs`,
//! `util.rs`/`keylet.rs`, `slots.rs`/`sto.rs`, `control.rs`. A submodule is
//! `pub(crate)` so the `impl HostBackend for Backend` block in `backend.rs`
//! can delegate to it in one line per function (see that file's module doc
//! comment for why the delegating block stays thin).
//!
//! `ledger_keylet`/`trace_float`/`prepare`/`otxn_slot`/`meta_slot`/
//! `xpop_slot`/`slot_set` live directly in `backend.rs`, not a submodule
//! here, because each needs `World` access the rest of its family's pure
//! functions don't (see `host::keylet`/`host::control`/`host::slots`'
//! module doc comments). `slots.rs`'s `World`-needing functions are the
//! exception: they stay in `slots.rs`, taking `&World` as an explicit
//! parameter, since they're part of one cohesive family with its
//! `World`-free functions. `prepare` needs both `World` *and* other
//! `Backend` methods (`etxn_details`/`etxn_fee_base`), which only
//! `backend.rs` has in scope.
//!
//! `host::control`'s functions (`hook_again`/`hook_skip`/
//! `hook_param_set`) need only `InvocationContext`; the merge into `World`
//! on `accept!` is `crate::env::TestEnv`'s job, not this module's.

pub(crate) mod control;
pub(crate) mod float;
pub(crate) mod keylet;
pub(crate) mod slots;
pub(crate) mod sto;
pub(crate) mod util;
