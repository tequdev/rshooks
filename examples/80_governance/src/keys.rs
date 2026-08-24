//! Hook-state key layouts.
//!
//! `"MC"`/`"RR"`/`"RD"`/seat-forward/member-reverse are also declared as
//! `#[state(..)]` fields on [`crate::Governance`] (see that struct's doc
//! comment for the single-declaration story) — but the *high call-site-
//! density* code paths (`setup`, `action_seat`, `push_l1_seat_entries`)
//! read/write them through the raw functions/constants below instead of
//! the typed field accessors. This is a build-budget necessity, not a
//! style choice: each layer of the typed accessor chain (`State::at` ->
//! `StateEntry::get` -> `state::state_get` -> `decode_read` -> `res`) is
//! `#[inline(always)]`, and `rshooks-build`'s Guard-type pipeline force-
//! inlines every reachable function into one `hook()` body regardless of
//! Rust-level `#[inline(never)]` (see `docs/DESIGN.md` §6.2b) — so a
//! function with *many* typed-accessor call sites accumulates their
//! combined internal branching directly into its own measured nesting.
//! Measured on this crate: extracting `govern`'s four top-level typed
//! reads into their own `#[inline(never)]` helpers left nesting
//! unchanged (a single function's own call-site count is what compounds,
//! not cross-function fusion), and using typed accessors at
//! `action_seat`/`setup`/`push_l1_seat_entries`'s combined ~15 call sites
//! would push the `govern` entry's nesting to 63, over the 32-level
//! limit; the raw calls above keep it well under that limit. The
//! *declaration* (single key/value schema, one shared ABI, reused by both
//! hooks) is the consolidation's real payoff either way — see
//! [`crate::Governance`]'s doc comment; using raw calls at these call
//! sites costs nothing there, since they still use the field's own
//! declared key bytes.
//! `RR`/`RD` reads in `reward` (2 call sites total — `reward` is the only
//! caller) stay on the typed [`crate::Governance::reward_rate`]/
//! [`crate::Governance::reward_delay`] accessors: low enough call-site
//! density to fit comfortably, and the clearest demonstration of the
//! "one declaration, shared by both hooks" story. Governance's own setup
//! writes to the same keys still go through raw `state_set` (see
//! `setup_initial_reward_rate_and_delay` in `src/lib.rs`), for the same
//! call-site-density reason as the raw calls above.
//!
//! Vote/vote-count keys were never candidates for the typed field form at
//! all, regardless of budget: a vote-count key embeds the topic's own
//! *runtime-length* value (8, 20, or 32 bytes depending on topic type)
//! directly in its bytes, which has no single fixed `V` a `#[state(..)]`
//! field could express.

use rshooks::guard;
use rshooks::types::AccountId;

/// Current member count.
pub const MEMBER_COUNT: [u8; 2] = *b"MC";

/// Maps a seat to a member.
pub fn seat_forward_key(seat: u8) -> [u8; 1] {
    [seat]
}

/// Maps a member to a seat.
pub fn member_reverse_key(account: &AccountId) -> [u8; 20] {
    account.0
}

/// Builds a member vote key.
pub fn vote_key(topic_type: u8, topic_id: u8, layer: u8, voter: &AccountId) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[0] = b'V';
    k[1] = topic_type;
    k[2] = topic_id;
    k[3] = layer;
    let mut i = 0usize;
    while i < 20 {
        guard!(20);
        if let (Some(slot), Some(&b)) = (k.get_mut(12usize.wrapping_add(i)), voter.0.get(i)) {
            *slot = b;
        }
        i = i.wrapping_add(1);
    }
    k
}

/// Builds a vote-count key. `value` is always exactly 8, 20, or 32 bytes
/// (reward/hook/seat topics respectively — see the module doc comment);
/// dispatching on that closed set, one fixed-size `copy_from_slice` per
/// arm, is genuinely straight-line code — the same "compare/copy a
/// fixed-size buffer without a loop" idiom `rshooks::buf_eq`'s
/// `buf_eq_8`/`_20`/`_32` helpers use, applied to a copy instead of a
/// comparison. An out-of-the-closed-set length (unreachable for a
/// well-formed topic) leaves the value bytes zeroed rather than guessing a
/// placement.
///
/// Deliberately loop-free and `guard!`-free: this function is
/// force-inlined at all three of its call sites (govern's two direct
/// calls plus one inside [`super::garbage_collect_votes`]'s topic scan),
/// and `guard!`'s `(1 << 31) + line!()` id resolves to the source line it
/// is written on — so every inlined copy would share one runtime
/// iteration counter. With the scan calling this up to 64 times, a loop
/// here would charge well over a thousand iterations against that single
/// counter, far past any per-call `maxiter`.
pub fn vote_count_key(topic_type: u8, topic_id: u8, layer: u8, value: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 32];
    match value.len() {
        8 => k[24..32].copy_from_slice(value),
        20 => k[12..32].copy_from_slice(value),
        32 => k[0..32].copy_from_slice(value),
        _ => {}
    }
    k[0] = b'C';
    k[1] = topic_type;
    k[2] = topic_id;
    k[3] = layer;
    k
}
