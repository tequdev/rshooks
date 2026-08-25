//! A behavior-equivalent Rust port of xahaud's genesis governance/reward
//! chain — [`hook/genesis/govern.c`](https://raw.githubusercontent.com/Xahau/xahaud/dev/hook/genesis/govern.c)
//! and [`hook/genesis/reward.c`](https://raw.githubusercontent.com/Xahau/xahaud/dev/hook/genesis/reward.c)
//! — as **one crate declaring both hooks**, the flagship example of the
//! `#[hooks]` multi-hook chain model (see
//! `docs/MULTI_HOOK_STRUCT_DESIGN.md` §3.1/§8.1 and this repo's
//! `README.md`).
//!
//! On the real Xahau genesis account, `govern` and `reward` are installed
//! side by side, in that order (`Hooks[0] = govern`, `Hooks[1] = reward`):
//! governance sets the reward rate/delay and the L1 seat table; reward
//! reads both to compute and distribute `ClaimReward` payouts. The two
//! genuinely share part of their state layout — `"RR"`/`"RD"` (reward
//! rate/delay) and the seat/member forward-reverse mapping are written by
//! governance and read by *both* hooks (governance's own seat-change
//! bookkeeping, and reward's active-validator-seat lookup for L1
//! distribution). This crate declares each of those once, on
//! [`Governance`], and both `#[hook]` entries below reference the same
//! fields.
//!
//! `"MC"` (member count) and every vote/vote-count entry remain
//! governance-only. Vote/vote-count keys in particular stay on the raw,
//! untyped `state`/`state_set` API (see [`keys`]'s module doc comment):
//! a vote-count key embeds the topic's own *runtime-length* value (8, 20,
//! or 32 bytes depending on topic type) directly in its bytes, which has
//! no single fixed value type a declarative `#[state(..)]` field could
//! express.
//!
//! Build: `rshooks build --manifest-path examples/80_governance/Cargo.toml`
//! produces two wasm artifacts, `0.govern.wasm` and `1.reward.wasm`, plus a
//! `sethook.template.json` covering both chain positions — see this
//! example's `README.md`.

#![no_std]

mod keys;
mod message;
mod mint_txn;
mod raw;
mod txn;

use mint_txn::{L1_SEATS, MintTxn};
use rshooks::prelude::*;
use rshooks::slot_path;
use rshooks::static_cell::HookStatic;
use rshooks::*;

/// Network genesis account. Shared by both hooks: governance's L1-table
/// check (`hook_accid == GENESIS_ACCOUNT`) and reward's `GenesisMint`
/// `Account`/(vote-forwarding) `Destination`.
const GENESIS_ACCOUNT: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");

// --- governance-only constants ---

/// `SEAT_COUNT` in govern.c: the round table has 20 seats.
const SEAT_COUNT: u8 = 20;
/// `HOOK_MAX` in govern.c: hook topics `0..=10` are accepted (govern.c's
/// own `n > HOOK_MAX` check lets `n == 10` through too, even though only
/// slots `0..=9` exist on a 10-slot `Hooks` array — preserved exactly;
/// see the README's differences table).
const HOOK_MAX: u8 = 10;

// --- reward-only constants ---

/// Default reward rate, mirroring reward.c's default (~1/300 per claim).
const DEFAULT_REWARD_RATE: XFL = XFL!(0.003333333333333333);

/// Default reward delay: reward.c's own default of 2,600,000 seconds.
const DEFAULT_REWARD_DELAY: XFL = XFL!(2600000);

/// Maximum active validators in `UNLReport`.
const MAX_UNL: usize = 128;

/// Validator account offset within an `ActiveValidators` entry.
const AV_FIRST_KEY_OFFSET: usize = 39;

/// Size of an `ActiveValidators` entry.
const AV_ENTRY_STRIDE: usize = 60;

/// Maximum serialized `ActiveValidators` size.
const AV_ARRAY_LEN: usize = AV_ENTRY_STRIDE * MAX_UNL + 4;

/// `UNLReport` ledger keylet.
const UNLREPORT_KEYLET: [u8; 34] = [
    0x00, 0x52, 0x61, 0xE3, 0x2E, 0x7A, 0x24, 0xA2, 0x38, 0xF1, 0xC6, 0x19, 0xD5, 0xF9, 0xDD, 0xCC,
    0x41, 0xA9, 0x4B, 0x33, 0xB6, 0x6C, 0x01, 0x63, 0xF7, 0xEF, 0xCC, 0x8A, 0x19, 0xC9, 0xFD, 0x6F,
    0x28, 0xDC,
];

hook_errors! {
    /// `govern`'s rollback reasons. See [`RewardError`]'s doc comment for
    /// why these codes (not govern.c's `__LINE__`) are the stable ones,
    /// and why only `accept`/`rollback` outcome + message are the
    /// behavior-equivalence target.
    pub enum GovernError {
        /// A hook parameter or otxn parameter required by the current
        /// step was missing or malformed.
        BadParameter = -201,
        /// The invoking account is not a member of this table.
        NotAMember = -202,
        /// An invariant govern.c enforces with `ASSERT` failed —
        /// defensive; unreachable for well-formed state.
        AssertionFailed = -203,
        /// Building or submitting an emitted transaction failed.
        EmitFailed = -204,
    }
}

hook_errors! {
    /// `reward` rollback reasons.
    pub enum RewardError {
        /// Governance has disabled rewards (`RR`/`RD` state <= 0).
        RewardsDisabled = -101,
        /// `RR`/`RD` fail reward.c's sanity bounds (rate in `(0, 1]`,
        /// delay `>= 1` second).
        MisconfiguredReward = -102,
        /// An invariant reward.c enforces with `ASSERT`/an unchecked Hook
        /// API call failed — defensive; unreachable for a well-formed
        /// `ClaimReward` on an account already past its setup claim.
        AssertionFailed = -103,
        /// Building or submitting the `GenesisMint` emission failed.
        EmitFailed = -104,
    }
}

// Scratch buffers for transaction and state data (governance).
static TOPIC_DATA: HookStatic<[u8; 32]> = HookStatic::new([0u8; 32]);
static PREVIOUS_TOPIC_DATA: HookStatic<[u8; 32]> = HookStatic::new([0u8; 32]);
static PREVIOUS_MEMBER: HookStatic<[u8; 32]> = HookStatic::new([0u8; 32]);
static VOTE_VALUE: HookStatic<[u8; 32]> = HookStatic::new([0u8; 32]);

/// The `GenesisMint` transaction under construction (reward) — large (see
/// `mint_txn::MintTxn`), so it lives in a wasm data segment/BSS via
/// [`HookStatic`] rather than as a stack local (see `examples/README.md`,
/// "Statics for templates and large buffers").
static MINT_TXN: HookStatic<MintTxn> = HookStatic::new(MintTxn::new());

/// `UNLReport.ActiveValidators`'s worst-case serialized bytes (reward) —
/// also a `HookStatic` for the same reason.
static AV_BUF: HookStatic<[u8; AV_ARRAY_LEN]> = HookStatic::new([0u8; AV_ARRAY_LEN]);

/// Takes a scratch buffer.
#[inline(always)]
fn take_scratch<T: Clone>(cell: &'static HookStatic<T>) -> &'static mut T {
    let Some(buf) = cell.take() else {
        GovernError::AssertionFailed.nope(b"govern: scratch buffer already taken");
    };
    buf
}

impl GovernError {
    #[inline(always)]
    fn nope(self, msg: &[u8]) -> ! {
        rollback!(msg, self)
    }
}

impl RewardError {
    fn rollback(self, msg: &[u8]) -> ! {
        rollback!(msg, self)
    }
}

#[inline(always)]
fn done(msg: &[u8]) -> ! {
    accept!(msg, 0)
}

/// This chain's shared state/parameter schema. See the crate module doc
/// comment for the overall behavior each field participates in, and
/// [`keys`] for the vote/vote-count entries that stay outside this
/// declaration.
#[hooks(description = "20-seat L1/L2 governance and reward chain")]
pub struct Governance {
    /// Current member count (governance-only).
    #[state(key = b"MC")]
    member_count: State<u8>,

    /// L1 reward rate. Written by governance (L1 table only); read by
    /// reward, which falls back to its own compiled-in default when
    /// absent.
    #[state(key = b"RR")]
    reward_rate: State<XFL>,

    /// L1 reward delay (seconds). Written by governance (L1 table only);
    /// read by reward, which falls back to its own compiled-in default
    /// when absent.
    #[state(key = b"RD")]
    reward_delay: State<XFL>,

    /// Seat -> member forward mapping, keyed by the 1-byte seat number.
    /// Written by governance; read by both governance (seat-change
    /// bookkeeping) and reward (L1 seat reward distribution).
    #[state(key_by = [u8; 1])]
    seat_forward: State<AccountId>,

    /// Member -> seat reverse mapping, keyed by the 20-byte account.
    /// Written by governance; read by both governance
    /// (membership/seat-move checks) and reward (active-validator-seat
    /// lookup).
    #[state(key_by = [u8; 20])]
    member_reverse: State<u8>,

    /// Per-seat initial member, read once at setup (`IS{seat}`,
    /// governance-only).
    #[hook_param(name_by = [u8; 3], required)]
    initial_member: HookParam<AccountId>,

    /// Initial member count, read once at setup (`IMC`, governance-only).
    #[hook_param(name = b"IMC", required)]
    initial_member_count: HookParam<[u8; 1]>,

    /// Initial reward rate, read once at L1 setup (`IRR`, governance-only).
    #[hook_param(name = b"IRR", required)]
    initial_reward_rate: HookParam<XFL>,

    /// Initial reward delay, read once at L1 setup (`IRD`,
    /// governance-only).
    #[hook_param(name = b"IRD", required)]
    initial_reward_delay: HookParam<XFL>,

    /// Vote topic (`T`): `{topic_type, topic_id}` (governance-only).
    #[otxn_param(name = b"T", required)]
    topic: OtxnParam<[u8; 2]>,

    /// Vote layer (`L`): `1` or `2`, L2 tables only (governance-only).
    #[otxn_param(name = b"L", required)]
    layer: OtxnParam<[u8; 1]>,
}

#[hooks]
impl Governance {
    /// Governance entry point (`Hooks[0]` on the real genesis account).
    /// See the crate module doc comment for the full behavior.
    #[hook(0, on = [Invoke], can_emit = [Invoke, SetHook])]
    fn govern(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            GovernError::EmitFailed.nope(b"govern: etxn_reserve failed");
        }

        if otxn_type() != TxType::Invoke {
            done(b"Governance: Passing non-Invoke txn. HookOn should be changed to avoid this.");
        }

        let Ok(sender) = otxn_field_typed(sfAccount) else {
            GovernError::AssertionFailed.nope(b"govern: could not read otxn Account")
        };
        let Ok(hook_accid) = hook_account_buf() else {
            GovernError::AssertionFailed.nope(b"govern: could not read hook_account")
        };

        if buf_eq_20(&sender, &hook_accid) {
            if let Ok(dest) = otxn_field_typed(sfDestination) {
                if !buf_eq_20(&hook_accid, &dest) {
                    done(b"Goverance: Passing outgoing txn.");
                }
            }
        }

        let is_l1_table = buf_eq_20(&hook_accid, &GENESIS_ACCOUNT);

        // `member_count_or_setup`/`member_seat_of`/`read_topic`/
        // `read_layer_required` read `"MC"`/member-reverse/`T`/`L`
        // through the raw `state`/`otxn_param` API using
        // [`Governance::member_count`]/[`Governance::member_reverse`]/
        // [`Governance::topic`]/[`Governance::layer`]'s own declared key/
        // name bytes, rather than those fields' typed accessors — see
        // [`keys`]'s module doc comment for why (`action_seat`/`setup`/
        // `push_l1_seat_entries`'s combined call-site density would push
        // this entry's nesting from 22 to 63, over the 32-level limit).
        let member_count = member_count_or_setup(is_l1_table);
        let member_id = member_seat_of(&sender);
        if member_id < 0 {
            GovernError::NotAMember
                .nope(b"Governance: You are not currently a governance member at this table.");
        }

        let (topic_ok, topic) = read_topic();
        let t = topic[0];
        let n = topic[1];
        if !topic_ok || (t != b'S' && t != b'H' && t != b'R') {
            GovernError::BadParameter
                .nope(b"Governance: Valid TOPIC must be specified as otxn parameter.");
        }
        if t == b'S' && n > SEAT_COUNT.wrapping_sub(1) {
            GovernError::BadParameter.nope(b"Governance: Valid seat topics are 0 through 19.");
        }
        if t == b'H' && n > HOOK_MAX {
            GovernError::BadParameter.nope(b"Governance: Valid hook topics are 0 through 9.");
        }
        if t == b'R' && n != b'R' && n != b'D' {
            GovernError::BadParameter
                .nope(b"Governance: Valid reward topics are R (rate) and D (delay).");
        }

        let mut l = 1u8;
        if !is_l1_table {
            l = match read_layer_required() {
                Ok([v]) => v,
                Err(_) => GovernError::BadParameter
                    .nope(b"Governance: Missing L parameter. Which layer are you voting for?"),
            };
            if l != 1 && l != 2 {
                GovernError::BadParameter.nope(b"Governance: Layer parameter must be '1' or '2'.");
            }
        }
        if l == 2 && t == b'R' {
            GovernError::BadParameter
                .nope(b"Governance: L2s cannot vote on RR/RD at L2, did you mean to set L=1?");
        }

        let topic_size: usize = if t == b'H' {
            32
        } else if t == b'S' {
            20
        } else {
            8
        };
        let padding = 32usize.wrapping_sub(topic_size);

        let topic_data = take_scratch(&TOPIC_DATA);
        let vresult = {
            let Some(dst) = topic_data.get_mut(padding..) else {
                GovernError::AssertionFailed.nope(b"govern: bad topic padding");
            };
            otxn_param(dst, b"V")
        };
        if vresult != Ok(topic_size) {
            GovernError::BadParameter
                .nope(b"Governance: Missing or incorrect size of VOTE data for TOPIC type.");
        }
        let topic_data_zero = buf_eq_32(topic_data, &[0u8; 32]);

        let vk = keys::vote_key(t, n, l, &sender);
        let previous_topic_data = take_scratch(&PREVIOUS_TOPIC_DATA);
        let previous_topic_size = {
            let Some(dst) = previous_topic_data.get_mut(padding..) else {
                GovernError::AssertionFailed.nope(b"govern: bad topic padding");
            };
            state(dst, &vk).unwrap_or(0)
        };

        if previous_topic_size == topic_size && buf_eq_32(previous_topic_data, topic_data) {
            done(b"Governance: Your vote is already cast this way for this topic.");
        }

        {
            let Some(value) = topic_data.get(padding..) else {
                GovernError::AssertionFailed.nope(b"govern: bad topic padding");
            };
            if state_set(value, &vk) != Ok(topic_size) {
                GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
            }
        }

        if previous_topic_size > 0 {
            let Some(prev_value) = previous_topic_data.get(padding..) else {
                GovernError::AssertionFailed.nope(b"govern: bad topic padding");
            };
            let vck = keys::vote_count_key(t, n, l, prev_value);
            let mut votes = [0u8; 1];
            if state(&mut votes, &vck) != Ok(1) {
                GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
            }
            if votes[0] == 0 {
                GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
            }
            if votes[0] <= 1 {
                if state_set(&[], &vck).is_err() {
                    GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
                }
            } else {
                let dec = votes[0].wrapping_sub(1);
                if state_set(&[dec], &vck) != Ok(1) {
                    GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
                }
            }
        }

        let votes = {
            let Some(new_value) = topic_data.get(padding..) else {
                GovernError::AssertionFailed.nope(b"govern: bad topic padding");
            };
            let vck = keys::vote_count_key(t, n, l, new_value);
            let mut vc = [0u8; 1];
            let _ = state(&mut vc, &vck); // ignored on failure, matching govern.c
            let new_votes = vc[0].wrapping_add(1);
            if state_set(&[new_votes], &vck).is_err() {
                GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
            }
            new_votes
        };

        // govern.c computes these via `int64_t q80 = member_count * 0.8;`
        // (hardware `double` multiplication, truncated toward zero on
        // assignment) — reproduced here as exact rational integer
        // arithmetic (`* 4 / 5`, `* 51 / 100`) instead: the Hook API's
        // guard checker rejects wasm floating-point opcodes outright for
        // a Guard-type hook, and for every `member_count` this hook ever
        // sees (`0..=SEAT_COUNT`, i.e. `0..=20`) the two give identical
        // truncated results.
        let mut q80 = member_count.wrapping_mul(4).wrapping_div(5);
        let mut q51 = member_count.wrapping_mul(51).wrapping_div(100);
        if q80 < 2 {
            q80 = 2;
        }
        if q51 < 2 {
            q51 = 2;
        }

        if is_l1_table || l == 2 {
            let threshold = if t == b'S' { q80 } else { member_count };
            if i64::from(votes) < threshold {
                done(b"Governance: Vote record. Not yet enough votes to action.");
            }
        } else if i64::from(votes) < q51 {
            done(b"Governance: Not yet enough votes to action L1 vote...");
        }

        if l == 1 && !is_l1_table {
            let Some(value) = topic_data.get(padding..) else {
                GovernError::AssertionFailed.nope(b"govern: bad topic padding");
            };
            if txn::emit_l1_vote_forward(
                &hook_accid,
                &GENESIS_ACCOUNT,
                t,
                n,
                topic_size as u8,
                value,
            ) {
                done(b"Governance: Successfully emitted L1 vote.");
            }
            GovernError::EmitFailed.nope(b"Governance: L1 vote emission failed.");
        }

        if t == b'R' {
            action_reward(t, n, padding, topic_data);
        } else if t == b'H' {
            action_hook(&hook_accid, n, topic_data_zero, topic_data);
        } else {
            action_seat(n, topic_data_zero, topic_data);
        }
    }

    /// Reward entry point (`Hooks[1]` on the real genesis account). See
    /// the crate module doc comment for the full behavior.
    #[hook(1, on = [Invoke, ClaimReward], can_emit = [GenesisMint])]
    fn reward(&self) -> HookResult {
        if etxn_reserve(1).is_err() {
            RewardError::EmitFailed.rollback(b"reward: etxn_reserve failed");
        }

        // Only TxType::ClaimReward is of interest; everything else
        // (including this hook's own emitted GenesisMint settling, and any
        // other incoming transaction type on an account HookOn'd broadly)
        // passes through.
        if otxn_type() != TxType::ClaimReward {
            accept!(b"Reward: Passing non-claim txn", 0);
        }

        let Ok(sender) = otxn_field_typed(sfAccount) else {
            RewardError::AssertionFailed.rollback(b"reward: could not read otxn Account")
        };
        let Ok(hook_acc) = hook_account_buf() else {
            RewardError::AssertionFailed.rollback(b"reward: could not read hook_account")
        };
        // The hook's own emitted ClaimReward-adjacent traffic (there is
        // none today, but reward.c guards this unconditionally) passes
        // through.
        if buf_eq_20(&sender, &hook_acc) {
            accept!(b"Reward: Passing outgoing txn", 0);
        }

        let rr = self
            .reward_rate
            .get()
            .unwrap_or(None)
            .unwrap_or(DEFAULT_REWARD_RATE);
        let rd = self
            .reward_delay
            .get()
            .unwrap_or(None)
            .unwrap_or(DEFAULT_REWARD_DELAY);
        let rewards_disabled = (rr.raw_bits() <= 0) | (rd.raw_bits() <= 0);
        if rewards_disabled {
            RewardError::RewardsDisabled.rollback(b"Reward: Rewards are disabled by governance.");
        }

        // reward.c treats any of these four checks failing
        // (`required_delay < 0`, `rr` negative, `rr > 1`, `rd < 1`) as
        // "misconfigured," rolling back with the same message — and,
        // since a `float_*` host call returning a raw negative *error*
        // code would *also* read as "< 0" or "!= 0" in reward.c's C, an
        // XFL operation failing outright leads to exactly the same
        // rollback there too. See [`raw`]'s doc comment for why these
        // specific calls (and only these) bypass `XFL`.
        const MISCONFIGURED_MSG: &[u8] =
            b"Reward: Rewards incorrectly configured by governance or unrecoverable error.";
        let required_delay = raw::float_int(rd.raw_bits(), 0, 0);
        let misconfigured = (required_delay < 0)
            | (raw::float_sign(rr.raw_bits()) != 0)
            | (raw::float_compare(rr.raw_bits(), raw::float_one(), COMPARE_GREATER) != 0)
            | (raw::float_compare(rd.raw_bits(), raw::float_one(), COMPARE_LESS) != 0);
        if misconfigured {
            RewardError::MisconfiguredReward.rollback(MISCONFIGURED_MSG);
        }

        // Slot the sender's AccountRoot; `RewardAccumulator` only exists
        // once a prior ClaimReward has already run the protocol-level
        // reward setup on this account. The typed helper returns a full
        // 34-byte `Keylet` by construction, so no length check is needed.
        let Ok(kl) = keylet_account(&sender) else {
            RewardError::AssertionFailed.rollback(b"reward: could not build account keylet")
        };
        let fields = match read_reward_fields(&kl) {
            RewardFieldRead::Read(f) => f,
            RewardFieldRead::NoAccountSlot => {
                RewardError::AssertionFailed.rollback(b"reward: could not slot sender account")
            }
            RewardFieldRead::SetupTxn => accept!(b"Reward: Passing reward setup txn", 0),
            RewardFieldRead::MissingFields => {
                RewardError::AssertionFailed.rollback(b"reward: missing reward accounting fields")
            }
        };
        let last_claim_time = fields.last_claim_time;

        let time_elapsed = ledger_last_time().wrapping_sub(last_claim_time);
        let required_delay = required_delay as u64;
        if time_elapsed < required_delay {
            let remaining = required_delay.wrapping_sub(time_elapsed);
            rollback!(&message::wait_message(remaining), 0);
        }

        let accumulator = fields.accumulator;
        let first = fields.first;
        let last = fields.last;
        let raw_balance = fields.raw_balance;

        if (first <= 0) | (last <= 0) {
            RewardError::AssertionFailed.rollback(b"Reward: Assertion failure.");
        }

        let cur = i64::from(ledger_seq());
        let elapsed = cur.wrapping_sub(first);
        if elapsed <= 0 {
            RewardError::AssertionFailed.rollback(b"Reward: Assertion failure.");
        }
        let elapsed_since_last = cur.wrapping_sub(last);

        // `Balance`'s raw as-int64 form keeps the native-amount control
        // bits set; mask them off, then convert drops -> whole XAH
        // (reward.c's `bal &= ~0xE000000000000000ULL; bal /= 1000000LL`).
        let bal = (raw_balance & !0xE000_0000_0000_0000u64).wrapping_div(1_000_000);

        let accumulator = if bal > 0 && elapsed_since_last > 0 {
            accumulator.wrapping_add((bal as i64).wrapping_mul(elapsed_since_last))
        } else {
            accumulator
        };

        // reward.c's own reward-rate math `ASSERT`s only on the two
        // `float_set` results — the subsequent `float_divide`/
        // `float_multiply`/`float_int` calls are never separately checked
        // in reward.c either, so a host-level failure there flows through
        // as whatever raw value the failed call returns, same as
        // reward.c's own `uint64_t reward_drops = float_int(...)`.
        let xfl_accum = raw::float_set(0, accumulator);
        if xfl_accum <= 0 {
            RewardError::AssertionFailed.rollback(b"Reward: Assertion failure.");
        }
        let xfl_elapsed = raw::float_set(0, elapsed);
        if xfl_elapsed <= 0 {
            RewardError::AssertionFailed.rollback(b"Reward: Assertion failure.");
        }
        let per_ledger = raw::float_divide(xfl_accum, xfl_elapsed);
        let xfl_reward = raw::float_multiply(rr.raw_bits(), per_ledger);
        let base_reward_drops = raw::float_int(xfl_reward, 6, 1) as u64;
        // `L1_SEATS` (20) is a nonzero compile-time constant;
        // `wrapping_div`'s only panicking case is a zero divisor, which
        // this can never be — clippy just can't see across the
        // `mint_txn::L1_SEATS` import to prove it.
        #[allow(clippy::arithmetic_side_effects)]
        let l1_drops = base_reward_drops.wrapping_div(L1_SEATS as u64);

        let Ok(otxn) = SlotObject::from_otxn() else {
            RewardError::AssertionFailed.rollback(b"reward: could not slot otxn");
        };
        let Ok(fee_slot) = otxn.get(sfFee) else {
            RewardError::AssertionFailed.rollback(b"reward: could not slot otxn Fee");
        };
        // reward.c: `int64_t xfl_fee = slot_float(11);` — unchecked,
        // folding a failed call into the same `> 0` test the value needs
        // anyway (see [`raw`]'s doc comment). `as_xfl()` is the checked
        // spelling, so the fold happens here instead: a failure becomes
        // `0`, which fails the same test a negative error code did.
        let xfl_fee = fee_slot.as_xfl().map(XFL::raw_bits).unwrap_or(0);
        let rewardee_drops = if xfl_fee > 0 {
            let fee_drops = raw::float_int(xfl_fee, 6, 1);
            base_reward_drops.wrapping_add(fee_drops as u64)
        } else {
            base_reward_drops
        };

        let Some(txn) = MINT_TXN.take() else {
            RewardError::EmitFailed.rollback(b"reward: mint txn buffer already taken");
        };
        // `MintTxn`'s methods roll back internally on the (unreachable in
        // practice) failure paths instead of returning `Result` — see
        // `mint_txn`'s module doc comment for why.
        txn.start();
        txn.write_emit_details();
        txn.push_entry(rewardee_drops, &sender);

        push_l1_seat_entries(txn, l1_drops);

        let bytes = txn.finish(ledger_seq());

        match emit_buf(bytes) {
            Ok(_hash) => accept!(b"Reward: Emitted reward txn successfully.", 0),
            Err(_) => RewardError::EmitFailed.rollback(b"Reward: Emit loopback failed."),
        }
    }
}

/// Reads `"MC"` state, or runs [`setup`] if it doesn't exist yet.
///
/// Raw `state_u64` read, not [`Governance::member_count`]'s typed
/// accessor — see [`keys`]'s module doc comment. `Err(_)`, not
/// `Err(HookError::DoesntExist)`: `state_u64`, not `state_i64` (the host
/// packs whatever bytes *are* stored into the return value, regardless of
/// their actual length — the right match for `"MC"`'s actual 1-byte
/// stored value); and testing only `is_err()`-equivalent (never reading
/// which specific error occurred) matches govern.c's own `==
/// DOESNT_EXIST` check for every reachable case on this fixed 2-byte key.
fn member_count_or_setup(is_l1_table: bool) -> i64 {
    match state_u64(&keys::MEMBER_COUNT) {
        Ok(v) => v as i64,
        Err(_) => setup(is_l1_table),
    }
}

/// Looks up `sender`'s seat number, or `-1` if it doesn't currently hold
/// one. Raw `state_u64` read (see [`keys`]'s module doc comment); masks
/// every read failure (not just absence) to `-1`.
fn member_seat_of(sender: &AccountId) -> i64 {
    state_u64(&keys::member_reverse_key(sender))
        .map(|v| v as i64)
        .unwrap_or(-1)
}

/// Reads the `T` otxn parameter: `(was it present and well-formed, the
/// value or `[0, 0]` on failure)`. Raw `otxn_param` read, not
/// [`Governance::topic`]'s typed accessor — see [`keys`]'s module doc
/// comment.
fn read_topic() -> (bool, [u8; 2]) {
    let mut buf = [0u8; 2];
    let ok = otxn_param(&mut buf, b"T") == Ok(2);
    (ok, if ok { buf } else { [0, 0] })
}

/// Reads the `L` otxn parameter (L2 tables only). Raw `otxn_param` read,
/// not [`Governance::layer`]'s typed accessor — see [`keys`]'s module doc
/// comment.
fn read_layer_required() -> core::result::Result<[u8; 1], ()> {
    let mut buf = [0u8; 1];
    if otxn_param(&mut buf, b"L") == Ok(1) {
        Ok(buf)
    } else {
        Err(())
    }
}

// The setup-only hook parameters below are read through the raw
// `hook_param_exact` API — the same bytes `Governance`'s own declared
// fields (`initial_member`/`initial_member_count`/`initial_reward_rate`/
// `initial_reward_delay`) use — instead of those fields' typed accessors:
// see [`keys`]'s module doc comment. Going through the typed accessors here
// was measured to push `setup`'s compiled nesting from 22 to 63 of the Hook
// API's 32-level guard-checker limit, so this stays on the raw API exactly
// like the rest of the setup path.

/// Reads `IRR`/`IRD` (L1-table-only setup) and writes `"RR"`/`"RD"` state.
/// Kept in its own `#[inline(never)]` function: reading both inline inside
/// `setup` pushes `setup`'s own compiled nesting close to the 32-level
/// limit — this function boundary keeps it well under. Raw `state_set`
/// writes, not [`Governance::reward_rate`]/
/// [`Governance::reward_delay`]'s typed `.set()` — see [`keys`]'s module
/// doc comment.
#[inline(never)]
fn setup_initial_reward_rate_and_delay() {
    let Ok(irr) = hook_param_exact::<XFL>(b"IRR") else {
        GovernError::BadParameter.nope(b"Governance: Initial Reward Rate Parameter missing (IRR).")
    };
    let Ok(ird) = hook_param_exact::<XFL>(b"IRD") else {
        GovernError::BadParameter.nope(b"Governance: Initial Reward Delay Parameter miss (IRD).")
    };
    if ird.raw_bits() == 0 {
        GovernError::BadParameter.nope(b"Governance: Initial Reward Delay must be > 0.");
    }
    if state_set(&irr.raw_bits().to_le_bytes(), b"RR").is_err() {
        GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
    }
    if state_set(&ird.raw_bits().to_le_bytes(), b"RD").is_err() {
        GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
    }
}

/// First-ever `Invoke` on this table: reads `IMC` (+`IRR`/`IRD` on L1)
/// and `IS0..IS{imc-1}` hook parameters and populates the initial seat
/// table. Diverges (`accept!`/`rollback!`) — govern.c's setup path never
/// falls through to normal voting.
#[inline(never)]
fn setup(is_l1_table: bool) -> ! {
    let Ok(imc) = hook_param_exact::<[u8; 1]>(b"IMC") else {
        GovernError::BadParameter.nope(b"Governance: Initial Member Count Parameter missing (IMC).")
    };
    if state_set(&imc, &keys::MEMBER_COUNT).is_err() {
        GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
    }
    let member_count = imc[0];
    if member_count == 0 {
        GovernError::BadParameter.nope(b"Governance: Initial Member Count must be > 0.");
    }
    if member_count > SEAT_COUNT {
        GovernError::BadParameter
            .nope(b"Governance: Initial Member Count must be <= Seat Count (20).");
    }

    if is_l1_table {
        setup_initial_reward_rate_and_delay();
    }

    let mut i = 0u8;
    while i < member_count {
        guard!(u32::from(SEAT_COUNT));
        let this_seat = i;
        i = i.wrapping_add(1);
        let member_name = [b'I', b'S', this_seat];
        let Ok(member_acc) = hook_param_exact::<AccountId>(&member_name) else {
            GovernError::BadParameter
                .nope(b"Governance: One or more initial member account ID's is missing")
        };
        if state_set(member_acc.as_ref(), &keys::seat_forward_key(this_seat)) != Ok(20) {
            GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
        }
        if state_set(&[this_seat], &keys::member_reverse_key(&member_acc)) != Ok(1) {
            GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
        }
    }

    done(b"Governance: Setup completed successfully.");
}

/// Actions a reward-rate/delay topic (`t == 'R'`): writes the voted value
/// directly under the `"RR"`/`"RD"` key. Diverges.
///
/// Deliberately bypasses [`Governance::reward_rate`]/
/// [`Governance::reward_delay`]'s typed `.set()` and writes the raw voted
/// bytes straight through [`rshooks::api::state::state_set`] instead: the
/// voted value is arbitrary member-supplied bytes with no format check
/// anywhere in this path (matching govern.c exactly — it never validates a
/// voted reward-rate/delay value either), and this call site should not
/// pick up a dependency on `XFL`'s decode step just because it happens not
/// to validate today. The key bytes (`b"RR"`/`b"RD"`) are exactly
/// [`Governance::reward_rate`]/[`Governance::reward_delay`]'s own declared
/// keys, so this hits the identical ledger slot either way.
#[inline(never)]
fn action_reward(_t: u8, n: u8, padding: usize, topic_data: &[u8; 32]) -> ! {
    let Some(value) = topic_data.get(padding..) else {
        GovernError::AssertionFailed.nope(b"govern: bad topic padding");
    };
    let key: [u8; 2] = if n == b'R' { *b"RR" } else { *b"RD" };
    if state_set(value, &key).is_err() {
        GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
    }
    if n == b'R' {
        done(b"Governance: Reward rate change actioned!");
    }
    done(b"Governance: Reward delay change actioned!");
}

/// Actions a hook topic (`t == 'H'`): installs or deletes hook slot `n`
/// via an emitted `HookSet`. Diverges.
#[inline(never)]
fn action_hook(hook_accid: &AccountId, n: u8, topic_data_zero: bool, topic_data: &[u8; 32]) -> ! {
    let Ok(keylet) = keylet_hook(hook_accid) else {
        GovernError::AssertionFailed.nope(b"govern: could not build hook keylet");
    };
    // The already-installed hook's hash, if there is one: the hook
    // account's `Hooks` array, element `n`, its `HookHash`. `slot_path!`
    // clears each intermediate as soon as its child exists, so this costs
    // one live slot rather than three — and it flattens to a single `if
    // let` here, which matters in a hook this close to the nesting
    // ceiling.
    //
    // Missing hook data skips the comparison.
    if let Ok(hook_acc) = SlotObject::from_keylet(&keylet) {
        if let Ok(hash_slot) = slot_path!(hook_acc[sfHooks][u32::from(n)][sfHookHash]) {
            let Ok(existing_hook) = hash_slot.value() else {
                GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
            };
            if buf_eq_32(&existing_hook, topic_data) {
                done(b"Goverance: Target hook is already the same as actioned hook.");
            }
        }
        let _ = hook_acc.clear();
    }

    if !topic_data_zero {
        let Ok(hdef_keylet) = keylet_hook_definition(&Hash::from(*topic_data)) else {
            GovernError::AssertionFailed.nope(b"govern: could not build hook definition keylet");
        };
        // Existence check only — the handle is dropped without reading,
        // and the slot lives until the hook ends (the C cost model; see
        // `rshooks::slot_obj`).
        if SlotObject::from_keylet(&hdef_keylet).is_err() {
            GovernError::BadParameter
                .nope(b"Goverance: Hook Hash doesn't exist on ledger while actioning hook.");
        }
    }

    let hash_opt = if topic_data_zero {
        None
    } else {
        Some(topic_data)
    };
    if txn::emit_hookset(hook_accid, n, hash_opt) {
        done(b"Governance: Hook actioned.");
    }
    GovernError::EmitFailed.nope(b"Governance: Emit failed during hook actioning.");
}

/// Actions a seat topic (`t == 'S'`): adds, moves, or removes a member —
/// govern.c's full 8-case `E|Z|M` logic table, including the vote
/// garbage-collection double loop over the outgoing member's votes.
/// Diverges.
#[inline(never)]
fn action_seat(n: u8, topic_data_zero: bool, topic_data: &[u8; 32]) -> ! {
    let Some(new_account) = topic_data.get(12..32) else {
        GovernError::AssertionFailed.nope(b"govern: bad topic data");
    };

    // `Governance.seat_forward`/`member_reverse`'s own declared key bytes
    // (`keys::seat_forward_key`/`keys::member_reverse_key`), read/written
    // through the raw `state`/`state_set` API rather than those fields'
    // typed accessors — see [`keys`]'s module doc comment.
    let previous_member = take_scratch(&PREVIOUS_MEMBER);
    let previous_present = {
        let Some(dst) = previous_member.get_mut(12..32) else {
            GovernError::AssertionFailed.nope(b"govern: bad buffer");
        };
        state(dst, &keys::seat_forward_key(n)) == Ok(20)
    };

    if previous_present {
        let Some(prev_account) = previous_member.get(12..32) else {
            GovernError::AssertionFailed.nope(b"govern: bad buffer");
        };
        if bytes_eq(prev_account, new_account) {
            done(b"Governance: Actioning seat change, but seat already contains the new member.");
        }
    }

    // Masks every read failure (not just absence) to "not currently
    // holding a seat."
    let existing_member = state_u64(new_account).map(|v| v as i64).unwrap_or(-1);
    let existing_member_moving = existing_member >= 0;

    let op: u8 = (u8::from(!previous_present) << 2)
        | (u8::from(topic_data_zero) << 1)
        | u8::from(existing_member_moving);
    if op == 0b011 || op == 0b111 {
        GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
    }

    let mut member_count = state_u64(&keys::MEMBER_COUNT)
        .map(|v| v as i64)
        .unwrap_or(0);
    if op == 0b001 || op == 0b010 {
        member_count = member_count.wrapping_sub(1);
    } else if op == 0b100 {
        member_count = member_count.wrapping_add(1);
    }
    if member_count <= 1 {
        GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
    }
    let mc = member_count as u8;
    if state_set(&[mc], &keys::MEMBER_COUNT) != Ok(1) {
        GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
    }

    if existing_member_moving {
        let m = existing_member as u8;
        if state_set(&[], &keys::seat_forward_key(m)).is_err() {
            GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
        }
        if state_set(&[], new_account).is_err() {
            GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
        }
    }

    if previous_present {
        previous_member[0] = b'V';
        let vote_value = take_scratch(&VOTE_VALUE);
        let mut tbl = 1u8;
        // `no_unroll`: without it, `opt-level = 3` fully unrolls this
        // 2-iteration loop, physically duplicating
        // `garbage_collect_votes`'s `guard!(66)` scan and multiplying
        // (rather than amortizing) its worst-case instruction count — see
        // `no_unroll`'s own doc comment for the full mechanism.
        while no_unroll(tbl) <= 2 {
            guard!(2);
            let this_tbl = tbl;
            tbl = tbl.wrapping_add(1);
            garbage_collect_votes(previous_member, vote_value, this_tbl);
        }

        if state_set(&[], &keys::seat_forward_key(n)).is_err() {
            GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
        }
        let Some(prev_account) = previous_member.get(12..32) else {
            GovernError::AssertionFailed.nope(b"govern: bad buffer");
        };
        if state_set(&[], prev_account).is_err() {
            GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
        }
    }

    if !topic_data_zero {
        if state_set(new_account, &keys::seat_forward_key(n)) != Ok(20) {
            GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
        }
        if state_set(&[n], new_account) != Ok(1) {
            GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
        }
    }

    done(b"Governance: Action member change.");
}

/// The inner loop of `action_seat`'s vote garbage collection: for table
/// `tbl` (1 or 2), scans all 32 topic slots (2 reward + 10 hook + 20
/// seat) the outgoing member (`previous_member[12..32)`) may have voted
/// on, deleting the vote and decrementing/deleting the vote-count entry.
/// `previous_member[0]` must already be `'V'`; this function overwrites
/// `[1..4)` on every iteration (`previous_member` doubles as the vote key
/// throughout, matching govern.c's in-place reuse of the same buffer).
/// Stays on the raw `state`/`state_set` API — see [`keys`]'s module doc
/// comment for why vote entries aren't declarative `#[state(..)]` fields.
#[inline(never)]
fn garbage_collect_votes(previous_member: &mut [u8; 32], vote_value: &mut [u8; 32], tbl: u8) {
    let mut i = 0u8;
    while i < 32 {
        // `66`, not `32`: the Hook API's guard-iteration counter is
        // scoped to the *static* `_g` call site (keyed by source line),
        // not to each dynamic loop entry — this function runs twice per
        // `action_seat` call (`tbl` 1 and 2), so the two calls'
        // iteration counts *accumulate* against this one guard site
        // within a single hook execution. `66` is govern.c's own
        // `GUARD(66)` bound for this exact loop (`2 * 32` plus slack).
        guard!(66);
        let this_i = i;
        i = i.wrapping_add(1);

        let topic_type = if this_i < 2 {
            b'R'
        } else if this_i < 12 {
            b'H'
        } else {
            b'S'
        };
        let topic_id = if this_i == 0 {
            b'R'
        } else if this_i == 1 {
            b'D'
        } else if this_i < 12 {
            this_i.wrapping_sub(2)
        } else {
            this_i.wrapping_sub(12)
        };
        previous_member[1] = topic_type;
        previous_member[2] = topic_id;
        previous_member[3] = tbl;

        let topic_size: usize = if topic_type == b'H' {
            32
        } else if topic_type == b'S' {
            20
        } else {
            8
        };
        let padding = 32usize.wrapping_sub(topic_size);

        let read = {
            let Some(dst) = vote_value.get_mut(padding..) else {
                continue;
            };
            state(dst, previous_member.as_ref())
        };
        if read != Ok(topic_size) {
            continue;
        }

        let Some(value) = vote_value.get(padding..) else {
            continue;
        };
        let vck = keys::vote_count_key(topic_type, topic_id, tbl, value);
        let mut vote_count = [0u8; 1];
        if state(&mut vote_count, &vck) == Ok(1) {
            if vote_count[0] <= 1 {
                let _ = state_set(&[], &vck);
            } else {
                let dec = vote_count[0].wrapping_sub(1);
                let _ = state_set(&[dec], &vck);
            }
        }
        let _ = state_set(&[], previous_member.as_ref());
    }
}

/// Element-wise byte-slice equality, no `copy_from_slice`/length-mismatch
/// panic path (see `examples/80_governance/src/mint_txn.rs::MintTxn::push`'s
/// doc comment for why that matters for this crate's nesting budget) —
/// used for the handful of runtime-length (not fixed-`N`) comparisons
/// `buf_eq_20`/`buf_eq_32` can't cover.
#[inline(always)]
fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0usize;
    let mut eq = true;
    while i < a.len() {
        guard!(32);
        if a.get(i) != b.get(i) {
            eq = false;
        }
        i = i.wrapping_add(1);
    }
    eq
}

/// The five account-root fields `reward` reads, and which of the three
/// "cannot proceed" outcomes applies instead.
enum RewardFieldRead {
    /// Every field was present.
    Read(RewardFields),
    /// The sender's account root could not be loaded at all.
    NoAccountSlot,
    /// No `sfRewardAccumulator`: this is a reward *setup* transaction,
    /// which the hook passes rather than rejects.
    SetupTxn,
    /// The accumulator is there but the rest of the accounting is not.
    MissingFields,
}

/// The account-root values the reward calculation needs.
struct RewardFields {
    accumulator: i64,
    first: i64,
    last: i64,
    raw_balance: u64,
    last_claim_time: u64,
}

/// Reads the sender's reward accounting off their account root.
///
/// `#[inline(never)]`, and that is load-bearing rather than stylistic:
/// extracting these reads out of the caller keeps its compiled nesting
/// depth under the Hook API's guard-checker limit — the same escape hatch
/// [`action_hook`] uses for governance's dense read paths.
#[inline(never)]
fn read_reward_fields(keylet: &Keylet) -> RewardFieldRead {
    let Ok(account) = SlotObject::from_keylet(keylet) else {
        return RewardFieldRead::NoAccountSlot;
    };
    let Ok(accumulator_slot) = account.get(sfRewardAccumulator) else {
        return RewardFieldRead::SetupTxn;
    };
    // Four sequential `let else`s rather than one tuple pattern: a 4-way
    // tuple `let (Ok(..), Ok(..), Ok(..), Ok(..)) = .. else` lowers to
    // nested matches, and this hook has no nesting to spare.
    let Ok(first_slot) = account.get(sfRewardLgrFirst) else {
        return RewardFieldRead::MissingFields;
    };
    let Ok(last_slot) = account.get(sfRewardLgrLast) else {
        return RewardFieldRead::MissingFields;
    };
    let Ok(balance_slot) = account.get(sfBalance) else {
        return RewardFieldRead::MissingFields;
    };
    let Ok(time_slot) = account.get(sfRewardTime) else {
        return RewardFieldRead::MissingFields;
    };

    RewardFieldRead::Read(RewardFields {
        accumulator: accumulator_slot.value().unwrap_or(0) as i64,
        first: i64::from(first_slot.value().unwrap_or(0)),
        last: i64::from(last_slot.value().unwrap_or(0)),
        // Read the native amount's raw wire encoding.
        raw_balance: balance_slot.assume_type::<u64>().value().unwrap_or(0),
        last_claim_time: u64::from(time_slot.value().unwrap_or(0)),
    })
}

/// Appends one `GenesisMint` entry per L1 seat currently occupied by an
/// active validator (reward.c's `can_reward`/seat-iteration loops). Any
/// failure here is non-fatal to the overall claim (matching reward.c,
/// which treats the whole `UNLReport`-driven L1 distribution as
/// best-effort and always proceeds to emit at least the rewardee entry).
///
/// Reads the seat/member state [`Governance::member_reverse`]/
/// [`Governance::seat_forward`] declare — the two fields governance
/// itself writes — through the raw `state` API rather than those fields'
/// typed accessors; see [`keys`]'s module doc comment and the crate
/// module doc comment.
fn push_l1_seat_entries(txn: &mut MintTxn, l1_drops: u64) {
    let Ok(unl_report) = SlotObject::from_keylet(&Keylet(UNLREPORT_KEYLET)) else {
        return;
    };
    // `sfActiveValidators` is an `STArray`, so the handle knows it has a
    // `count()`.
    let Ok(validators) = unl_report.get(sfActiveValidators) else {
        return;
    };
    let Some(av_buf) = AV_BUF.take() else {
        return;
    };
    // `count()` borrows, so it must come before the consuming `raw()`
    // read — which is exactly the ordering the borrowing pre-checks exist
    // for.
    let Ok(count) = validators.count() else {
        return;
    };
    let Ok(read_len) = validators.raw(av_buf) else {
        return;
    };
    if read_len == 0 {
        return;
    }
    let av_size = (count as usize).min(MAX_UNL);

    // Flattened with `let-else` + `continue` throughout (rather than
    // nested `if let`) so each loop body stays a single level deep — the
    // Hook API's static guard checker rejects wasm control flow nested
    // past 32 levels, and a chain of nested `if let`s here compounds with
    // this function's own inlining into `reward()`.
    let mut can_reward = [false; L1_SEATS];
    let mut i = 0usize;
    while i < av_size {
        guard!(MAX_UNL as u32);
        let offset = AV_FIRST_KEY_OFFSET.wrapping_add(AV_ENTRY_STRIDE.wrapping_mul(i));
        i = i.wrapping_add(1);

        let Some(key) = av_buf.get(offset..offset.wrapping_add(ACC_ID_LEN)) else {
            continue;
        };
        // C's off-by-one (`seat > L1SEATS` rather than `>=`, so a stored
        // seat byte of exactly 20 is let through) is preserved here too,
        // but `can_reward.get_mut` turns the resulting out-of-range index
        // into a safe no-op instead of C's out-of-bounds array write.
        // `seat` values only ever come from `govern`, which never
        // assigns 20, so this is unreachable in practice.
        let mut seat = [0u8; 1];
        if state(&mut seat, key) != Ok(1) {
            continue;
        }
        let Some(&seat) = seat.first() else {
            continue;
        };
        if seat as usize > L1_SEATS {
            continue;
        }
        let Some(slot) = can_reward.get_mut(seat as usize) else {
            continue;
        };
        *slot = true;
    }

    // A manual `while`, not `for seat in 0u8..(L1_SEATS as u8)`: a `for`
    // loop's `Iterator::next()` bookkeeping compiles to real instructions
    // *before* the loop body, which pushes `guard!`'s `_g` call out of
    // the required first-three-instructions position the checker looks
    // for.
    let mut seat = 0u8;
    while seat < L1_SEATS as u8 {
        guard!(L1_SEATS as u32);
        let this_seat = seat;
        seat = seat.wrapping_add(1);
        if !can_reward.get(this_seat as usize).copied().unwrap_or(false) {
            continue;
        }
        let mut destination = AccountId::zeroed();
        if state(destination.as_mut(), core::slice::from_ref(&this_seat)) != Ok(ACC_ID_LEN) {
            continue;
        }
        // At most `L1_SEATS` seat entries are ever pushed here (see
        // `mint_txn::MAX_ENTRIES`), so `push_entry` never actually hits
        // its internal overflow check in practice.
        txn.push_entry(l1_drops, &destination);
    }
}
