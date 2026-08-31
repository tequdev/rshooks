//! `hook_again`/`hook_skip`/`hook_param_set` semantics (P2-E —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "control leftovers", stage
//! plan §7). `etxn::prepare` and `trace_float` are pure/otxn-adjacent rather
//! than control-flow, and live in `crate::backend`'s own `impl HostBackend
//! for Backend` block directly, not here.
//!
//! Ported against `Xahau/xahaud`, branch `dev`,
//! `src/xrpld/app/hook/detail/HookAPI.cpp` (line numbers cited per function
//! below) plus `src/xrpld/app/hook/detail/applyHook.cpp`'s wasm-facing
//! wrappers (the fixed-length checks a caller's raw hash/name buffers must
//! satisfy before `HookAPI` itself is even called).
//!
//! # Commit timing: staged in [`InvocationContext`], merged into [`crate::world::World`] only on `accept!`
//!
//! All three functions here write into `InvocationContext` fields, never
//! directly into `World` — `crate::env::TestEnv`'s `run_entry` helper is the
//! only place that merges those fields into the persistent `World`, and
//! only in the `ExitType::Accept` arm (mirroring `pending_emissions` /
//! `committed_emissions`'s stage-then-merge pattern). This matches
//! upstream's own commit gate: `Transactor::executeHookChain`
//! (`src/xrpld/app/tx/detail/Transactor.cpp:1425-1468`) only reaches its
//! "gather skips" / "gather overrides" step when `hookResult.exitType ==
//! hook_api::ExitType::ACCEPT`; a hook that calls `hook_skip`/
//! `hook_param_set` and then rolls back never has those calls take effect.
//! `hook_again`'s real commit path is more involved (its
//! `executeAgainAsWeak` flag is gathered by the outer `Transactor::operator()`
//! irrespective of the *individual* hook's own exit type — `Transactor.cpp:2020-2030`
//! — since it drives a genuinely separate later weak re-execution this
//! harness does not model at all); tying it to the same accept-only commit
//! as `hook_skip`/`hook_param_set` here is a deliberate simplification for
//! the explicit-invocation model (no chain, no weak re-execution), chosen
//! for consistency with the other two and with `TestEnv::hook_again_requested()`'s
//! own "accessor reads the last **committed** invocation's flag" contract.
//!
//! # No chain model
//!
//! None of these three functions consult a seeded `SetHook` ledger object
//! (there is none in this harness): `hook_skip`'s upstream "is this hash
//! even in the chain" check (`HookAPI.cpp:1762-1777`) is skipped entirely —
//! every hash is accepted verbatim, per
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4's explicit "no chain model"
//! instruction.

use crate::invocation::{InvocationContext, MAX_PARAM_OVERRIDES};

/// `hook_again()`: ported from `HookAPI::hook_again`
/// (`HookAPI.cpp:1658-1670`):
///
/// ```text
/// if (hookCtx.result.executeAgainAsWeak) return Unexpected(ALREADY_SET);
/// if (hookCtx.result.isStrong) { hookCtx.result.executeAgainAsWeak = true; return 1ULL; }
/// return Unexpected(PREREQUISITE_NOT_MET);
/// ```
///
/// The `isStrong` branch is not reproducible here — this harness has no
/// notion of strong-vs-weak execution (`invoke` always runs a direct,
/// explicit entry call, design §2.4) — so every invocation is treated as
/// the `isStrong` case: `hook_again` succeeds the first time it is called
/// in an invocation and reports `ALREADY_SET` on a second call in the
/// *same* invocation. The `PREREQUISITE_NOT_MET` (weak-execution) branch
/// is unreachable in this model and never returned.
pub(crate) fn hook_again(ctx: &mut InvocationContext) -> i64 {
    if ctx.hook_again_called {
        return rshooks_core::ALREADY_SET;
    }
    ctx.hook_again_called = true;
    1
}

/// `hook_skip(hash, flags)`: ported from `HookAPI::hook_skip`
/// (`HookAPI.cpp:1745-1782`) plus its wasm wrapper's own fixed-length gate
/// (`applyHook.cpp:3874-3894`: `read_len != 32` -> `INVALID_ARGUMENT`,
/// checked *before* `HookAPI::hook_skip` is even called — reproduced here
/// since [`HostBackend::hook_skip`](rshooks_core::backend::HostBackend::hook_skip)
/// takes `hash` as an unsized `&[u8]`, not a fixed array).
///
/// `flags` other than `0`/`1` -> `INVALID_ARGUMENT`. `flags == 1` (delete):
/// removes a hash **this same invocation** already added — `DOESNT_EXIST`
/// if it was never added this invocation (upstream's `hookCtx.result.hookSkips`
/// is fresh per hook execution, `HookAPI.h`'s `HookResult`, so a delete can
/// only ever target *this invocation's own* earlier adds — no persistent
/// "already skipped from a prior invocation" set exists, consistent with
/// this harness's no-chain model). `flags == 0` (add): a hash already added
/// this invocation is a no-op success (upstream:
/// `if (skips.find(hash) != skips.end()) return 1;`, without re-adding or
/// re-checking chain membership); otherwise the chain-membership check
/// (`HookAPI.cpp:1762-1777`) is skipped per this module's doc comment — the
/// add always succeeds. Every successful call (add or delete, no-op or not)
/// is appended to [`InvocationContext::skip_directives`] verbatim, in call
/// order (design §4: "recorded verbatim").
pub(crate) fn hook_skip(ctx: &mut InvocationContext, hash: &[u8], flags: u32) -> i64 {
    if flags != 0 && flags != 1 {
        return rshooks_core::INVALID_ARGUMENT;
    }
    let Ok(hash32) = <[u8; 32]>::try_from(hash) else {
        return rshooks_core::INVALID_ARGUMENT;
    };

    if flags == 1 {
        if !skip_currently_active(&ctx.skip_directives, &hash32) {
            return rshooks_core::DOESNT_EXIST;
        }
        ctx.skip_directives.push((hash32, 1));
        return 1;
    }

    // flags == 0 (add): already-active is a no-op success but is still
    // recorded verbatim (see doc comment above).
    ctx.skip_directives.push((hash32, 0));
    1
}

/// Whether `hash` is currently in this invocation's skip set, derived by
/// replaying `directives` rather than tracking a separate `HashSet` — the
/// last directive touching `hash` decides: `flags == 0` means currently
/// skipped, `flags == 1` (or no directive at all) means not.
fn skip_currently_active(directives: &[([u8; 32], u32)], hash: &[u8; 32]) -> bool {
    directives
        .iter()
        .rev()
        .find(|(h, _)| h == hash)
        .is_some_and(|(_, flags)| *flags == 0)
}

/// `hook_param_set(hash, name, value)`: ported from `HookAPI::hook_param_set`
/// (`HookAPI.cpp:1712-1744`) plus its wasm wrapper's own redundant checks
/// (`applyHook.cpp:3844-3858`, including `hread_len != 32` ->
/// `INVALID_ARGUMENT`, reproduced here for the same reason as
/// [`hook_skip`]'s fixed-length gate above). The wrapper's check order is
/// reproduced exactly (`hook_hash` length checked after the name checks
/// but *before* the value-size check):
///
/// - `name` empty -> `TOO_SMALL`; `name.len() > 32` (`hook::maxHookParameterKeySize()`,
///   vendored `Enum.h:60-64`) -> `TOO_BIG`.
/// - `hook_hash` must be exactly 32 bytes -> `INVALID_ARGUMENT` otherwise.
/// - `value.len() > 256` (`hook::maxHookParameterValueSize()`, vendored
///   `Enum.h:66-70`) -> `TOO_BIG`. An empty `value` is **not** special-cased
///   at write time — it is stored as-is (upstream: `overrides[hash][name] =
///   value` unconditionally); `Backend::hook_param`'s read side treats a
///   stored empty override as "deleted" (`DOESNT_EXIST`), matching
///   `HookAPI::hook_param`'s own `if (param.size() == 0) return
///   Unexpected(DOESNT_EXIST);` (`HookAPI.cpp:1685-1687`).
/// - `overrideCount >= hook_api::max_params` (16, vendored `Enum.h:401`) ->
///   `TOO_MANY_PARAMS`, counted per call (not per distinct key), fresh each
///   invocation.
///
/// Later calls in the same invocation for the same `(hash, name)` key
/// overwrite earlier ones (`InvocationContext::pending_param_overrides` is a
/// `HashMap`) — matches upstream's `overrides[hash][name] = value`
/// assignment exactly.
pub(crate) fn hook_param_set(
    ctx: &mut InvocationContext,
    value: &[u8],
    name: &[u8],
    hook_hash: &[u8],
) -> i64 {
    if name.is_empty() {
        return rshooks_core::TOO_SMALL;
    }
    if name.len() > 32 {
        return rshooks_core::TOO_BIG;
    }
    let Ok(hash32) = <[u8; 32]>::try_from(hook_hash) else {
        return rshooks_core::INVALID_ARGUMENT;
    };
    if value.len() > 256 {
        return rshooks_core::TOO_BIG;
    }
    if ctx.param_override_count >= MAX_PARAM_OVERRIDES {
        return rshooks_core::TOO_MANY_PARAMS;
    }
    ctx.param_override_count = ctx.param_override_count.saturating_add(1);
    ctx.pending_param_overrides
        .insert((hash32, name.to_vec()), value.to_vec());
    value.len() as i64
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    fn ctx() -> InvocationContext {
        InvocationContext::new(0)
    }

    // -- hook_again --

    #[test]
    fn hook_again_succeeds_once_then_already_set() {
        let mut c = ctx();
        assert_eq!(hook_again(&mut c), 1);
        assert!(c.hook_again_called);
        assert_eq!(hook_again(&mut c), rshooks_core::ALREADY_SET);
    }

    // -- hook_skip --

    #[test]
    fn hook_skip_add_then_duplicate_add_is_a_no_op_success() {
        let mut c = ctx();
        let hash = [1u8; 32];
        assert_eq!(hook_skip(&mut c, &hash, 0), 1);
        assert_eq!(hook_skip(&mut c, &hash, 0), 1);
        assert_eq!(c.skip_directives.len(), 2);
    }

    #[test]
    fn hook_skip_delete_of_never_added_hash_is_doesnt_exist() {
        let mut c = ctx();
        assert_eq!(hook_skip(&mut c, &[2u8; 32], 1), rshooks_core::DOESNT_EXIST);
        assert!(c.skip_directives.is_empty());
    }

    #[test]
    fn hook_skip_add_then_delete_then_delete_again_is_doesnt_exist() {
        let mut c = ctx();
        let hash = [3u8; 32];
        assert_eq!(hook_skip(&mut c, &hash, 0), 1);
        assert_eq!(hook_skip(&mut c, &hash, 1), 1);
        assert_eq!(hook_skip(&mut c, &hash, 1), rshooks_core::DOESNT_EXIST);
    }

    #[test]
    fn hook_skip_rejects_bad_flags_and_bad_hash_length() {
        let mut c = ctx();
        assert_eq!(
            hook_skip(&mut c, &[0u8; 32], 2),
            rshooks_core::INVALID_ARGUMENT
        );
        assert_eq!(
            hook_skip(&mut c, &[0u8; 10], 0),
            rshooks_core::INVALID_ARGUMENT
        );
    }

    // -- hook_param_set --

    #[test]
    fn hook_param_set_rejects_size_limits() {
        let mut c = ctx();
        assert_eq!(
            hook_param_set(&mut c, b"v", b"", &[0u8; 32]),
            rshooks_core::TOO_SMALL
        );
        let long_name = vec![b'x'; 33];
        assert_eq!(
            hook_param_set(&mut c, b"v", &long_name, &[0u8; 32]),
            rshooks_core::TOO_BIG
        );
        let long_value = vec![b'v'; 257];
        assert_eq!(
            hook_param_set(&mut c, &long_value, b"n", &[0u8; 32]),
            rshooks_core::TOO_BIG
        );
        assert_eq!(
            hook_param_set(&mut c, b"v", b"n", &[0u8; 10]),
            rshooks_core::INVALID_ARGUMENT
        );
    }

    #[test]
    fn hook_param_set_checks_hash_length_before_value_size() {
        // Bad-length hash + over-limit value: upstream checks `hread_len
        // != 32` before the value-size check, so this is `INVALID_ARGUMENT`,
        // not `TOO_BIG`.
        let mut c = ctx();
        let long_value = vec![b'v'; 257];
        assert_eq!(
            hook_param_set(&mut c, &long_value, b"n", &[0u8; 10]),
            rshooks_core::INVALID_ARGUMENT
        );
    }

    #[test]
    fn hook_param_set_records_the_override_and_returns_value_len() {
        let mut c = ctx();
        assert_eq!(hook_param_set(&mut c, b"val", b"K", &[9u8; 32]), 3);
        assert_eq!(
            c.pending_param_overrides.get(&([9u8; 32], b"K".to_vec())),
            Some(&b"val".to_vec())
        );
    }

    #[test]
    fn hook_param_set_overwrites_the_same_key_within_one_invocation() {
        let mut c = ctx();
        let _ = hook_param_set(&mut c, b"first", b"K", &[9u8; 32]);
        let _ = hook_param_set(&mut c, b"second", b"K", &[9u8; 32]);
        assert_eq!(
            c.pending_param_overrides.get(&([9u8; 32], b"K".to_vec())),
            Some(&b"second".to_vec())
        );
        assert_eq!(c.param_override_count, 2);
    }

    #[test]
    fn hook_param_set_enforces_the_max_params_budget() {
        let mut c = ctx();
        for i in 0..16u8 {
            assert_eq!(hook_param_set(&mut c, b"v", &[i], &[0u8; 32]), 1);
        }
        assert_eq!(
            hook_param_set(&mut c, b"v", b"one-too-many", &[0u8; 32]),
            rshooks_core::TOO_MANY_PARAMS
        );
    }
}
