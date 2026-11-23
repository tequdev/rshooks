//! `dice`/`util_random` semantics (xahaud `featureRNG`), ported from
//! `HookAPI::rng_next`/`dice`/`util_random` (`src/xrpld/app/hook/detail/
//! HookAPI.cpp`, branch `release`) and `ApplyContext::discard`
//! (`src/xrpld/app/tx/detail/ApplyContext.cpp`).
//!
//! The ledger's `Random` object is modeled by [`RngObject`]: it exists once
//! seeded, and every successful draw replaces its state with
//! `sha512Half('RNGI', state)`. A draw's output block is
//! `sha512Half('RNGO', state, txid, hook account, hook hash, chain position,
//! strong, callback, wasm param)`, with `u32` fields big-endian. The state
//! is never part of the rollback snapshot: upstream's `discard` keeps the
//! advanced state, so a rolled-back invocation still consumed its draws.
//!
//! The ledger-sequence rule is the closed-ledger one (the object must be
//! seeded for exactly the current ledger); the open-ledger preview
//! allowance is not modeled.

use std::vec::Vec;

use rshooks_core::{INVALID_ARGUMENT, TOO_BIG, TOO_LITTLE_ENTROPY, TOO_MANY_NONCES, TOO_SMALL};

use super::util::sha512_half;
use crate::invocation::InvocationContext;
use crate::world::World;

/// `hook_api::max_rng_draws` (vendored `Enum.h`): successful draws per hook
/// execution, shared by `dice` and `util_random`.
pub(crate) const MAX_RNG_DRAWS: u32 = 256;
/// `util_random`'s maximum `write_len`.
const MAX_RANDOM_LEN: u32 = 512;

const OUTPUT_TAG: [u8; 4] = 0x524E_474Fu32.to_be_bytes(); // 'RNGO'
const STEP_TAG: [u8; 4] = 0x524E_4749u32.to_be_bytes(); // 'RNGI'

/// The `Random` ledger object's hook-visible state.
#[derive(Clone, Copy)]
pub(crate) struct RngObject {
    /// `sfRandomData`; `None` while no `Random` object exists.
    pub(crate) state: Option<[u8; 32]>,
    /// `sfLedgerSequence`; `None` follows the env's current ledger sequence.
    pub(crate) seeded_for: Option<u32>,
    /// `sfEntropyCount`.
    pub(crate) count: u16,
    /// `sfEntropyDenominator`.
    pub(crate) denominator: u16,
}

impl RngObject {
    pub(crate) const fn new() -> Self {
        Self {
            state: None,
            seeded_for: None,
            count: 1,
            denominator: 1,
        }
    }
}

fn output_block(state: &[u8; 32], w: &World, ctx: &InvocationContext) -> [u8; 32] {
    let mut pre = Vec::with_capacity(4 + 32 + 32 + 20 + 32 + 3 + 4);
    pre.extend_from_slice(&OUTPUT_TAG);
    pre.extend_from_slice(state);
    pre.extend_from_slice(&w.otxn.id);
    pre.extend_from_slice(&w.hook_account);
    pre.extend_from_slice(&w.current_hook_hash().unwrap_or([0u8; 32]));
    pre.push(w.hook_pos as u8);
    pre.push(u8::from(!ctx.is_callback));
    pre.push(u8::from(ctx.is_callback));
    pre.extend_from_slice(&ctx.wasm_param.to_be_bytes());
    sha512_half(&pre)
}

fn step(state: &[u8; 32]) -> [u8; 32] {
    let mut pre = [0u8; 36];
    pre[..4].copy_from_slice(&STEP_TAG);
    pre[4..].copy_from_slice(state);
    sha512_half(&pre)
}

/// `HookAPI::rng_next`. The `featureRNG` rules check is not modeled: the
/// imports are not whitelisted without the amendment, so a hook that runs
/// has it enabled.
// The contribution fields are widened from `u16` to `u32` before the scaled
// comparison, so it cannot overflow; `rng_draws` is below `MAX_RNG_DRAWS`.
#[allow(clippy::arithmetic_side_effects)]
fn rng_next(w: &mut World, ctx: &mut InvocationContext) -> Result<[u8; 32], i64> {
    if ctx.rng_draws >= MAX_RNG_DRAWS {
        return Err(TOO_MANY_NONCES);
    }
    let obj = w.rng;
    let Some(state) = obj.state else {
        return Err(TOO_LITTLE_ENTROPY);
    };
    if obj.seeded_for.unwrap_or(w.ledger_seq) != w.ledger_seq {
        return Err(TOO_LITTLE_ENTROPY);
    }
    let (count, denominator) = (u32::from(obj.count), u32::from(obj.denominator));
    if count == 0 || count * 5 < denominator * 4 {
        return Err(TOO_LITTLE_ENTROPY);
    }
    let out = output_block(&state, w, ctx);
    w.rng.state = Some(step(&state));
    ctx.rng_draws = ctx.rng_draws.saturating_add(1);
    Ok(out)
}

/// Rejection sampling over the block's big-endian `u64` words, re-hashing
/// the block for more words, so every side is exactly equally likely.
// `sides >= 1` (`dice` rejects 0) and `excess < sides`, so no remainder
// divides by zero and `max - excess` cannot underflow.
#[allow(clippy::arithmetic_side_effects)]
fn dice_from_block(mut block: [u8; 32], sides: u32) -> u64 {
    let sides = u64::from(sides);
    let max = u64::MAX;
    let excess = (max % sides + 1) % sides;
    let mut word = 0u64;
    for _ in 0..16 {
        for chunk in block.chunks_exact(8) {
            let mut be = [0u8; 8];
            be.copy_from_slice(chunk);
            word = u64::from_be_bytes(be);
            if word <= max - excess {
                return word % sides;
            }
        }
        block = sha512_half(&block);
    }
    word % sides
}

/// `HookAPI::dice`: a draw in `[0, sides)`, or the error code.
pub(crate) fn dice(w: &mut World, ctx: &mut InvocationContext, sides: u32) -> i64 {
    if sides == 0 {
        return INVALID_ARGUMENT;
    }
    match rng_next(w, ctx) {
        Ok(block) => dice_from_block(block, sides) as i64,
        Err(e) => e,
    }
}

/// `HookAPI::util_random`: `len` bytes, 32 per block, the block re-hashed
/// for more.
pub(crate) fn util_random(
    w: &mut World,
    ctx: &mut InvocationContext,
    len: u32,
) -> Result<Vec<u8>, i64> {
    if len == 0 {
        return Err(TOO_SMALL);
    }
    if len > MAX_RANDOM_LEN {
        return Err(TOO_BIG);
    }
    let mut block = rng_next(w, ctx)?;
    let mut out = Vec::with_capacity(len as usize);
    while out.len() < len as usize {
        let n = (len as usize).saturating_sub(out.len()).min(32);
        out.extend(block.iter().take(n));
        block = sha512_half(&block);
    }
    Ok(out)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    type Edit = dyn Fn(&mut World, &mut InvocationContext);

    const SEED: [u8; 32] = [0x11; 32];

    fn world_seeded() -> World {
        let mut w = World::new();
        w.rng.state = Some(SEED);
        w.otxn.id = [0x22; 32];
        w.hook_account = [0x33; 20];
        w.hook_hashes.insert(0, [0x44; 32]);
        w
    }

    fn hex(b: &[u8]) -> std::string::String {
        use std::fmt::Write;
        b.iter().fold(std::string::String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
    }

    // Vectors below were computed with an independent SHA-512 (Python
    // `hashlib`) over the preimage layout documented in the module header.
    const BLOCK_HEX: &str = "ef88f077eeace15839dc14df61cb43166dd7516853c56f96c35b8d4580de3e09";
    const NEXT_STATE_HEX: &str = "c65eb638f06a418755662ee2fee423f29c735e6d61e771a84c09b24b796d92a1";
    const REHASHED_BLOCK_HEX: &str =
        "d7f5f62ac4c1e3da7597b32c121657caa10ed517c797d67e0f98211ab50bd477";

    #[test]
    fn output_block_matches_independent_vector() {
        let mut w = world_seeded();
        let mut ctx = InvocationContext::new(0);
        let block = rng_next(&mut w, &mut ctx).unwrap();
        assert_eq!(hex(&block), BLOCK_HEX);
        assert_eq!(hex(&w.rng.state.unwrap()), NEXT_STATE_HEX);
    }

    #[test]
    fn failure_callback_block_matches_independent_vector() {
        let mut w = world_seeded();
        let mut ctx = InvocationContext::new(0);
        ctx.is_callback = true;
        ctx.wasm_param = 1;
        let block = rng_next(&mut w, &mut ctx).unwrap();
        assert_eq!(
            hex(&block),
            "fb921bd3c09bc07bb47b99f56c9166eadc865b065a1a4f195a4ccf7641ba4f26"
        );
    }

    #[test]
    fn dice_matches_independent_vector() {
        // Word 0 of `BLOCK_HEX` is 17260309969793507672; 17260309969793507672 % 6 == 2.
        let mut w = world_seeded();
        let mut ctx = InvocationContext::new(0);
        assert_eq!(dice(&mut w, &mut ctx, 6), 2);
    }

    #[test]
    fn util_random_matches_independent_vector() {
        let mut w = world_seeded();
        let mut ctx = InvocationContext::new(0);
        let out = util_random(&mut w, &mut ctx, 64).unwrap();
        assert_eq!(hex(&out[..32]), BLOCK_HEX);
        assert_eq!(hex(&out[32..]), REHASHED_BLOCK_HEX);
    }

    #[test]
    fn util_random_is_prefix_consistent_and_costs_one_draw() {
        let mut ctx_a = InvocationContext::new(0);
        let a = util_random(&mut world_seeded(), &mut ctx_a, 512).unwrap();
        let mut ctx_b = InvocationContext::new(0);
        let b = util_random(&mut world_seeded(), &mut ctx_b, 5).unwrap();
        assert_eq!(a.len(), 512);
        assert_eq!(&a[..5], &b[..]);
        assert_eq!((ctx_a.rng_draws, ctx_b.rng_draws), (1, 1));
    }

    #[test]
    fn dice_rejection_skips_words_above_the_unbiased_range() {
        // sides == 3: 2^64 mod 3 == 1, so u64::MAX is rejected and the next
        // word decides.
        let mut block = [0u8; 32];
        block[..8].copy_from_slice(&u64::MAX.to_be_bytes());
        block[8..16].copy_from_slice(&7u64.to_be_bytes());
        assert_eq!(dice_from_block(block, 3), 7 % 3);
        // A power-of-two modulus never rejects.
        assert_eq!(dice_from_block(block, 4), u64::MAX % 4);
    }

    #[test]
    fn dice_stays_in_range() {
        let mut w = world_seeded();
        let mut ctx = InvocationContext::new(0);
        for sides in [1u32, 2, 6, 1000, u32::MAX] {
            let v = dice(&mut w, &mut ctx, sides);
            assert!((0..i64::from(sides)).contains(&v), "dice({sides}) = {v}");
        }
    }

    #[test]
    fn argument_checks_precede_entropy_and_consume_no_draw() {
        let mut w = World::new();
        let mut ctx = InvocationContext::new(0);
        assert_eq!(dice(&mut w, &mut ctx, 0), INVALID_ARGUMENT);
        assert_eq!(util_random(&mut w, &mut ctx, 0), Err(TOO_SMALL));
        assert_eq!(util_random(&mut w, &mut ctx, 513), Err(TOO_BIG));
        assert_eq!(ctx.rng_draws, 0);
    }

    #[test]
    fn unseeded_is_too_little_entropy() {
        let mut w = World::new();
        let mut ctx = InvocationContext::new(0);
        assert_eq!(dice(&mut w, &mut ctx, 6), TOO_LITTLE_ENTROPY);
        assert_eq!(util_random(&mut w, &mut ctx, 1), Err(TOO_LITTLE_ENTROPY));
        assert_eq!(ctx.rng_draws, 0);
    }

    #[test]
    fn stale_seed_sequence_is_too_little_entropy() {
        let mut w = world_seeded();
        w.ledger_seq = 10;
        w.rng.seeded_for = Some(9);
        assert_eq!(
            dice(&mut w, &mut InvocationContext::new(0), 6),
            TOO_LITTLE_ENTROPY
        );
        w.rng.seeded_for = Some(10);
        assert!(dice(&mut w, &mut InvocationContext::new(0), 6) >= 0);
    }

    #[test]
    fn entropy_contribution_threshold_is_eighty_percent() {
        let mut w = world_seeded();
        for (count, den, ok) in [
            (0u16, 0u16, false),
            (0, 5, false),
            (3, 5, false),
            (4, 5, true),
            (1, 0, true),
            (5, 5, true),
            (79, 100, false),
            (80, 100, true),
            // Widened comparison: no 16-bit overflow at the field limits.
            (u16::MAX, u16::MAX, true),
        ] {
            w.rng.count = count;
            w.rng.denominator = den;
            let r = dice(&mut w, &mut InvocationContext::new(0), 6);
            assert_eq!(r >= 0, ok, "count={count} denominator={den} -> {r}");
            if !ok {
                assert_eq!(r, TOO_LITTLE_ENTROPY);
            }
        }
    }

    #[test]
    fn budget_is_shared_per_execution_and_failures_do_not_count() {
        let mut w = world_seeded();
        let mut ctx = InvocationContext::new(0);
        for i in 0..MAX_RNG_DRAWS {
            // Failed calls in between consume nothing.
            assert_eq!(dice(&mut w, &mut ctx, 0), INVALID_ARGUMENT);
            let ok = if i % 2 == 0 {
                dice(&mut w, &mut ctx, 6) >= 0
            } else {
                util_random(&mut w, &mut ctx, 512).is_ok()
            };
            assert!(ok, "draw {i}");
        }
        assert_eq!(dice(&mut w, &mut ctx, 6), TOO_MANY_NONCES);
        assert_eq!(util_random(&mut w, &mut ctx, 1), Err(TOO_MANY_NONCES));
        // A fresh execution has a fresh budget.
        assert!(dice(&mut w, &mut InvocationContext::new(1), 6) >= 0);
    }

    #[test]
    fn budget_check_precedes_entropy_check() {
        let mut w = World::new();
        let mut ctx = InvocationContext::new(0);
        ctx.rng_draws = MAX_RNG_DRAWS;
        assert_eq!(dice(&mut w, &mut ctx, 6), TOO_MANY_NONCES);
    }

    #[test]
    fn state_steps_between_draws() {
        let mut w = world_seeded();
        let mut ctx = InvocationContext::new(0);
        let a = util_random(&mut w, &mut ctx, 32).unwrap();
        assert_eq!(w.rng.state, Some(step(&SEED)));
        let b = util_random(&mut w, &mut ctx, 32).unwrap();
        assert_eq!(w.rng.state, Some(step(&step(&SEED))));
        assert_ne!(a, b);
    }

    #[test]
    fn identical_inputs_are_deterministic_and_each_input_changes_the_output() {
        let draw = |edit: &Edit| {
            let mut w = world_seeded();
            let mut ctx = InvocationContext::new(0);
            edit(&mut w, &mut ctx);
            util_random(&mut w, &mut ctx, 32).unwrap()
        };
        let base = draw(&|_, _| {});
        assert_eq!(base, draw(&|_, _| {}));
        let variants: [&Edit; 7] = [
            &|w, _| w.otxn.id = [0x23; 32],
            &|w, _| w.hook_account = [0x34; 20],
            &|w, _| {
                w.hook_hashes.insert(0, [0x45; 32]);
            },
            &|w, _| {
                w.hook_hashes.insert(1, [0x44; 32]);
                w.hook_pos = 1;
            },
            &|_, c| c.is_callback = true,
            &|_, c| c.wasm_param = 1,
            &|w, _| w.rng.state = Some([0x12; 32]),
        ];
        for (i, v) in variants.iter().enumerate() {
            assert_ne!(base, draw(v), "variant {i}");
        }
    }
}
