//! `hook_again`/`hook_skip`/`hook_param_set` semantics (P2-E —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "control leftovers", stage
//! plan §7), plus `invoke_cbak` support. Empty scaffolding as of P2-A:
//! `crate::backend::Backend` does not yet override `hook_again`/
//! `hook_skip`/`hook_param_set`, so every call still falls through to the
//! trait's own `NOT_IMPLEMENTED` default. `etxn::prepare` and
//! `trace_float` (also listed under "control leftovers" in the design's
//! stage plan) are pure/otxn-adjacent rather than control-flow, and land
//! in `crate::backend`'s own `impl HostBackend for Backend` block directly
//! once implemented, not in this submodule.
