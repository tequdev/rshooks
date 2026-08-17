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
//! not cross-function fusion), and `action_seat`/`setup`/
//! `push_l1_seat_entries`'s combined ~15 typed-accessor call sites alone
//! pushed the `govern` entry from 22 to 63 (limit: 32) before this
//! crate's own raw-call reversion for those paths. The *declaration*
//! (single key/value schema, one shared ABI, reused by both hooks) is the
//! consolidation's real payoff either way — see [`crate::Governance`]'s
//! doc comment; using raw calls at these call sites costs nothing there,
//! since they still use the field's own declared key bytes.
//! `RR`/`RD` (governance's setup writes, reward's reads — 4 call sites
//! total) stay on the typed [`crate::Governance::reward_rate`]/
//! [`crate::Governance::reward_delay`] accessors: low enough call-site
//! density to fit comfortably, and the clearest demonstration of the
//! "governance writes, reward reads, one declaration" story.
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

/// Builds a vote-count key.
pub fn vote_count_key(topic_type: u8, topic_id: u8, layer: u8, value: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 32];
    let start = 32usize.saturating_sub(value.len());
    let mut i = 0usize;
    while i < value.len() {
        guard!(32);
        if let (Some(slot), Some(&b)) = (k.get_mut(start.wrapping_add(i)), value.get(i)) {
            *slot = b;
        }
        i = i.wrapping_add(1);
    }
    k[0] = b'C';
    k[1] = topic_type;
    k[2] = topic_id;
    k[3] = layer;
    k
}
