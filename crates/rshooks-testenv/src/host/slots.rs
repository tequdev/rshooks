//! `slot_*`/`otxn_slot`/`meta_slot`/`xpop_slot` semantics (P2-D —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "slot family", stage plan
//! §7). Empty scaffolding as of P2-A: `crate::backend::Backend` does not
//! yet override any of these `HostBackend` methods, so every call still
//! falls through to the trait's own `NOT_IMPLEMENTED` default. The
//! per-invocation `slots: [Option<SlotEntry>; 256]` array this family will
//! read/write already lives on [`crate::invocation::InvocationContext`]
//! (design §3) — this module is where the navigation logic over it lands.
