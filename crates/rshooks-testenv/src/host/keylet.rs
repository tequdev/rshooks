//! `util_keylet` semantics — all 26 keylet types (P2-C —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "util_* + ledger_keylet",
//! stage plan §7). Consumes [`rshooks_core::backend::KeyletArg`]'s
//! already-resolved `Value`/`Bytes` slots — see that type's doc comment and
//! `crates/rshooks/testenv-call-sites.txt`'s header for why the six
//! arguments arrive pre-resolved rather than as raw pointers.
//!
//! Every keylet's byte layout below is a direct port of xahaud's own
//! implementation (`Xahau/xahaud`, branch `dev`), not the `hook-api`
//! skill's prose summary — sources consulted:
//!
//! - `src/libxrpl/protocol/Indexes.cpp` (`ripple::keylet::*`) — each type's
//!   own hash formula (which fields, in which order, canonical account
//!   ordering for `line`, the non-hashed `child`/`unchecked` pass-throughs,
//!   `quality`'s direct byte overwrite, `page`'s `index == 0` short-circuit,
//!   `cron`'s bespoke namespaced-prefix layout).
//! - `src/xrpld/app/hook/detail/applyHook.cpp`'s `util_keylet`
//!   `DEFINE_HOOK_FUNCTION` — exactly which of the six `a..f` slots each
//!   `keylet_type` consumes, in what shape (this is also independently
//!   cross-checked against `rshooks::api::keylet`'s 26 typed wrappers, which
//!   construct `KeyletArg`s in the identical shape — see each `match` arm's
//!   comment for the corresponding typed helper). Confirms `KEYLET_TICKET`
//!   (13) has **no case at all** in the real `switch` — every call for it
//!   falls through to the same `return INVALID_ARGUMENT;` an unrecognized
//!   `keylet_type` gets, matching `examples/13_keylets`' README-documented
//!   live finding (not a gap in this port).
//! - `include/xrpl/protocol/detail/ledger_entries.macro` /
//!   `include/xrpl/protocol/LedgerFormats.h` — each type's own 2-byte
//!   `ltXXX` ledger-entry-type code, which is **not always** the same byte
//!   as its `LedgerNameSpace` hash-space character (the `examples/13_keylets`
//!   README's documented quirk for `Cron`/`OwnerDir`/`Fees`, generalized here
//!   to every directory-shaped type: `Quality`/`EmittedDir`/`Page`/
//!   `HookStateDir`/`OwnerDir` all serialize as `ltDIR_NODE` (`0x0064`)
//!   regardless of their own distinct hash-space character, and `NftOffer`'s
//!   hash space (`'q'`) differs from its type code (`0x0037`) the same way —
//!   confirmed from source, not assumed, since only 13 of the 26 types were
//!   independently e2e-verified (see the README's "e2e verification scope").
//!
//! Per-type test vectors below reuse `examples/13_keylets`' own fixed test
//! inputs (`TEST_HASH`/`TEST_STATE_KEY`/`TEST_NAMESPACE`/`TEST_CURRENCY`/the
//! `*_SEQ` constants) and, for the 13 types independently verified in
//! `e2e/test/keylets.test.ts`, that file's own expected-hex vectors
//! (computed there via the `xahau` npm package's hash helpers or the same
//! `sha512Half(ledgerSpace ++ args)` pattern) — lifted verbatim as the test
//! oracle per this stage's task brief.

use std::vec::Vec;

use rshooks_core::INVALID_ARGUMENT;
use rshooks_core::backend::KeyletArg;
use rshooks_core::consts::{
    KEYLET_ACCOUNT, KEYLET_AMENDMENTS, KEYLET_CHECK, KEYLET_CHILD, KEYLET_CRON,
    KEYLET_DEPOSIT_PREAUTH, KEYLET_EMITTED, KEYLET_EMITTED_DIR, KEYLET_ESCROW, KEYLET_FEES,
    KEYLET_HOOK, KEYLET_HOOK_DEFINITION, KEYLET_HOOK_STATE, KEYLET_HOOK_STATE_DIR, KEYLET_LINE,
    KEYLET_NEGATIVE_UNL, KEYLET_NFT_OFFER, KEYLET_OFFER, KEYLET_OWNER_DIR, KEYLET_PAGE,
    KEYLET_PAYCHAN, KEYLET_QUALITY, KEYLET_SIGNERS, KEYLET_SKIP, KEYLET_TICKET, KEYLET_UNCHECKED,
};

use super::util::sha512_half;

// ---------------------------------------------------------------------
// `LedgerNameSpace` hash-space characters (`Indexes.cpp`) — the byte fed
// into `indexHash`'s tagged-hash prefix. See the module doc comment: NOT
// always the same as the resulting keylet's own `ltXXX` type code.
// ---------------------------------------------------------------------
mod space {
    pub(super) const ACCOUNT: u16 = b'a' as u16;
    pub(super) const DIR_NODE: u16 = b'd' as u16;
    pub(super) const TRUST_LINE: u16 = b'r' as u16;
    pub(super) const OFFER: u16 = b'o' as u16;
    pub(super) const OWNER_DIR: u16 = b'O' as u16;
    pub(super) const SKIP_LIST: u16 = b's' as u16;
    pub(super) const ESCROW: u16 = b'u' as u16;
    pub(super) const AMENDMENTS: u16 = b'f' as u16;
    pub(super) const FEE_SETTINGS: u16 = b'e' as u16;
    pub(super) const SIGNER_LIST: u16 = b'S' as u16;
    pub(super) const PAYMENT_CHANNEL: u16 = b'x' as u16;
    pub(super) const CHECK: u16 = b'C' as u16;
    pub(super) const DEPOSIT_PREAUTH: u16 = b'p' as u16;
    pub(super) const NEGATIVE_UNL: u16 = b'N' as u16;
    pub(super) const HOOK: u16 = b'H' as u16;
    pub(super) const HOOK_STATE_DIR: u16 = b'J' as u16;
    pub(super) const HOOK_STATE: u16 = b'v' as u16;
    pub(super) const HOOK_DEFINITION: u16 = b'D' as u16;
    pub(super) const EMITTED_TXN: u16 = b'E' as u16;
    pub(super) const EMITTED_DIR: u16 = b'F' as u16;
    pub(super) const NFTOKEN_OFFER: u16 = b'q' as u16;
    pub(super) const CRON: u16 = b'L' as u16;
}

// ---------------------------------------------------------------------
// `ltXXX` ledger-entry type codes (`ledger_entries.macro`/`LedgerFormats.h`)
// — the keylet's own serialized 2-byte prefix.
// ---------------------------------------------------------------------
mod lt {
    pub(super) const ANY: u16 = 0x0000;
    pub(super) const NFTOKEN_OFFER: u16 = 0x0037;
    pub(super) const CRON: u16 = 0x0041; // LEDGER_ENTRY_DUPLICATE — distinct from hash-space 'L'
    pub(super) const CHECK: u16 = 0x0043;
    pub(super) const HOOK_DEFINITION: u16 = 0x0044;
    pub(super) const EMITTED_TXN: u16 = 0x0045;
    pub(super) const HOOK: u16 = 0x0048;
    pub(super) const NEGATIVE_UNL: u16 = 0x004e;
    pub(super) const SIGNER_LIST: u16 = 0x0053;
    pub(super) const ACCOUNT_ROOT: u16 = 0x0061;
    pub(super) const DIR_NODE: u16 = 0x0064;
    pub(super) const AMENDMENTS: u16 = 0x0066;
    pub(super) const LEDGER_HASHES: u16 = 0x0068;
    pub(super) const OFFER: u16 = 0x006f;
    pub(super) const DEPOSIT_PREAUTH: u16 = 0x0070; // LEDGER_ENTRY_DUPLICATE
    pub(super) const RIPPLE_STATE: u16 = 0x0072;
    pub(super) const FEE_SETTINGS: u16 = 0x0073;
    pub(super) const ESCROW: u16 = 0x0075;
    pub(super) const HOOK_STATE: u16 = 0x0076;
    pub(super) const PAYCHAN: u16 = 0x0078;
    pub(super) const CHILD: u16 = 0x1CD2;
}

/// `indexHash<Args...>(space, args...)` (`Indexes.cpp`): `sha512Half` of the
/// space's 2-byte big-endian code followed by each part's raw bytes, in
/// order. Reuses [`sha512_half`] (`super::util`) — the same primitive
/// `util_sha512h` exposes directly.
fn index_hash(space: u16, parts: &[&[u8]]) -> [u8; 32] {
    // `Vec::new()`, not a precomputed `with_capacity`: every part here is a
    // small (<= 34-byte) keylet argument, so the capacity hint isn't worth
    // the `+`/`.sum()` arithmetic (and the `clippy::arithmetic_side_effects`
    // it would need justifying).
    let mut buf = Vec::new();
    buf.extend_from_slice(&space.to_be_bytes());
    for p in parts {
        buf.extend_from_slice(p);
    }
    sha512_half(&buf)
}

/// Assembles the 34-byte wire form: 2-byte big-endian `ltXXX` type code,
/// then the 32-byte key verbatim.
fn keylet(ty: u16, key: [u8; 32]) -> [u8; 34] {
    let mut out = [0u8; 34];
    out[0..2].copy_from_slice(&ty.to_be_bytes());
    out[2..34].copy_from_slice(&key);
    out
}

/// A `KeyletArg::Bytes` slot of exactly `len` bytes, or `INVALID_ARGUMENT` —
/// mirrors `applyHook.cpp`'s own `read_len != N -> INVALID_ARGUMENT` checks
/// (a `KeyletArg::Value`/`Unused` here means the typed wrapper never
/// resolved real bytes for this slot, e.g. it arrived through the untyped
/// `util_keylet`/`util_keylet_buf` path — see that function's own doc
/// comment for why testenv fidelity is only guaranteed through a typed
/// helper).
fn bytes_exact<'a>(arg: &KeyletArg<'a>, len: usize) -> Result<&'a [u8], i64> {
    match *arg {
        KeyletArg::Bytes(b) if b.len() == len => Ok(b),
        _ => Err(INVALID_ARGUMENT),
    }
}

/// A `KeyletArg::Value` slot — a bare scalar component (not a pointer).
fn require_value(arg: &KeyletArg<'_>) -> Result<u32, i64> {
    match *arg {
        KeyletArg::Value(v) => Ok(v),
        _ => Err(INVALID_ARGUMENT),
    }
}

/// A slot this keylet type does not use — `Unused` (the typed-wrapper
/// spelling) or `Value(0)` (the untyped raw-ABI spelling of "absent",
/// matching `applyHook.cpp`'s `a == 0`-style checks) are both accepted.
fn require_unused(arg: &KeyletArg<'_>) -> Result<(), i64> {
    match *arg {
        KeyletArg::Unused | KeyletArg::Value(0) => Ok(()),
        _ => Err(INVALID_ARGUMENT),
    }
}

/// The `UInt32or256` shape `OFFER`/`CHECK`/`ESCROW`/`NFT_OFFER`/`PAYCHAN`
/// take for their sequence component: a bare `u32` (hashed as 4 big-endian
/// bytes) or a 32-byte hash (hashed verbatim). Every typed wrapper in
/// `rshooks::api::keylet` only ever constructs the `u32` form (`seq: u32`
/// in each of their signatures) — the `Bytes(32)` arm exists for
/// completeness against the documented argument shape, but has no typed
/// caller and so no independent test vector.
fn seq_or_hash_bytes(arg: &KeyletArg<'_>) -> Result<Vec<u8>, i64> {
    match *arg {
        KeyletArg::Value(v) => Ok(v.to_be_bytes().to_vec()),
        KeyletArg::Bytes(b) if b.len() == 32 => Ok(b.to_vec()),
        _ => Err(INVALID_ARGUMENT),
    }
}

/// `HookAPI::util_keylet` dispatch (`applyHook.cpp`'s `util_keylet`
/// `DEFINE_HOOK_FUNCTION`, decoupled from wasm-memory concerns since `args`
/// already carries resolved bytes/values — see [`KeyletArg`]'s doc
/// comment). One match arm per `rshooks_core::consts::KEYLET_*` constant,
/// in the same numeric order as that module and `rshooks::api::keylet`'s 26
/// typed wrappers, each annotated with the typed helper whose `KeyletArg`
/// shape it expects.
#[allow(clippy::too_many_lines)]
// one mechanical arm per KEYLET_* type, each already factored to a single index_hash/keylet call — splitting further would only relocate, not reduce, the switch
#[allow(clippy::indexing_slicing)] // the only indexing/slicing in this function is `dir[0]`/`dir[1]`/`&dir[2..34]` in the QUALITY arm, reached only after `bytes_exact(&args[0], 34)?` has already established `dir.len() == 34`
pub(crate) fn util_keylet(keylet_type: u32, args: [KeyletArg<'_>; 6]) -> Result<[u8; 34], i64> {
    match keylet_type {
        // `keylet_hook`: [Bytes(account:20), Unused x5].
        KEYLET_HOOK => {
            let account = bytes_exact(&args[0], 20)?;
            for a in &args[1..6] {
                require_unused(a)?;
            }
            Ok(keylet(lt::HOOK, index_hash(space::HOOK, &[account])))
        }

        // `keylet_hook_state`: [Bytes(account:20), Bytes(key:32), Bytes(ns:32), Unused x3].
        KEYLET_HOOK_STATE => {
            let account = bytes_exact(&args[0], 20)?;
            let key = bytes_exact(&args[1], 32)?;
            let ns = bytes_exact(&args[2], 32)?;
            for a in &args[3..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::HOOK_STATE,
                index_hash(space::HOOK_STATE, &[account, key, ns]),
            ))
        }

        // `keylet_account`: [Bytes(account:20), Unused x5].
        KEYLET_ACCOUNT => {
            let account = bytes_exact(&args[0], 20)?;
            for a in &args[1..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::ACCOUNT_ROOT,
                index_hash(space::ACCOUNT, &[account]),
            ))
        }

        // `keylet_amendments`: [Unused x6].
        KEYLET_AMENDMENTS => {
            for a in &args {
                require_unused(a)?;
            }
            Ok(keylet(lt::AMENDMENTS, index_hash(space::AMENDMENTS, &[])))
        }

        // `keylet_child`: [Bytes(hash:32), Unused x5] — `key` passed through
        // verbatim, *not* hashed (`keylet::child`).
        KEYLET_CHILD => {
            let hash = bytes_exact(&args[0], 32)?;
            for a in &args[1..6] {
                require_unused(a)?;
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(hash);
            Ok(keylet(lt::CHILD, key))
        }

        // `keylet_skip`: `None` -> [Value(0), Value(0), Unused x4];
        // `Some(seq)` -> [Value(seq), Value(1), Unused x4].
        KEYLET_SKIP => {
            let ledger = require_value(&args[0])?;
            let flag = require_value(&args[1])?;
            for a in &args[2..6] {
                require_unused(a)?;
            }
            let key = match flag {
                0 => index_hash(space::SKIP_LIST, &[]),
                1 => {
                    let shifted = (ledger >> 16).to_be_bytes();
                    index_hash(space::SKIP_LIST, &[&shifted])
                }
                _ => return Err(INVALID_ARGUMENT),
            };
            Ok(keylet(lt::LEDGER_HASHES, key))
        }

        // `keylet_fees`: [Unused x6].
        KEYLET_FEES => {
            for a in &args {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::FEE_SETTINGS,
                index_hash(space::FEE_SETTINGS, &[]),
            ))
        }

        // `keylet_negative_unl`: [Unused x6].
        KEYLET_NEGATIVE_UNL => {
            for a in &args {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::NEGATIVE_UNL,
                index_hash(space::NEGATIVE_UNL, &[]),
            ))
        }

        // `keylet_line`: [Bytes(a:20), Bytes(b:20), Bytes(currency:20), Unused x3].
        // Accounts are hashed in canonical (byte-min, byte-max) order
        // (`keylet::line`'s `std::minmax`) regardless of caller order.
        KEYLET_LINE => {
            let a = bytes_exact(&args[0], 20)?;
            let b = bytes_exact(&args[1], 20)?;
            let currency = bytes_exact(&args[2], 20)?;
            for a in &args[3..6] {
                require_unused(a)?;
            }
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            Ok(keylet(
                lt::RIPPLE_STATE,
                index_hash(space::TRUST_LINE, &[lo, hi, currency]),
            ))
        }

        // `keylet_offer`: [Bytes(account:20), Value(seq), Unused x4].
        KEYLET_OFFER => {
            let account = bytes_exact(&args[0], 20)?;
            let seq = seq_or_hash_bytes(&args[1])?;
            for a in &args[2..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::OFFER,
                index_hash(space::OFFER, &[account, &seq]),
            ))
        }

        // `keylet_quality`: [Bytes(dir:34), Value(high), Value(low), Unused x3].
        // Not `indexHash`-based: overwrites the *existing* dir keylet's last
        // 8 key bytes with the big-endian `(high << 32) | low` quality
        // (`keylet::quality`); `dir` must itself already be a `ltDIR_NODE`
        // keylet.
        KEYLET_QUALITY => {
            let dir = bytes_exact(&args[0], 34)?;
            let high = require_value(&args[1])?;
            let low = require_value(&args[2])?;
            for a in &args[3..6] {
                require_unused(a)?;
            }
            if dir[0] != 0x00 || dir[1] != 0x64 {
                return Err(INVALID_ARGUMENT);
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&dir[2..34]);
            let quality = (u64::from(high) << 32) | u64::from(low);
            key[24..32].copy_from_slice(&quality.to_be_bytes());
            Ok(keylet(lt::DIR_NODE, key))
        }

        // `keylet_emitted_dir`: [Unused x6].
        KEYLET_EMITTED_DIR => {
            for a in &args {
                require_unused(a)?;
            }
            Ok(keylet(lt::DIR_NODE, index_hash(space::EMITTED_DIR, &[])))
        }

        // `keylet_ticket`: no case at all in the real `util_keylet` switch —
        // see the module doc comment. Always `INVALID_ARGUMENT`, matching
        // the live host limitation `examples/13_keylets`' README documents.
        KEYLET_TICKET => Err(INVALID_ARGUMENT),

        // `keylet_signers`: [Bytes(account:20), Unused x5]. `page` is always
        // `0` (`keylet::signers(account)` hardcodes it via the private
        // `signers(account, page)`).
        KEYLET_SIGNERS => {
            let account = bytes_exact(&args[0], 20)?;
            for a in &args[1..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::SIGNER_LIST,
                index_hash(space::SIGNER_LIST, &[account, &0u32.to_be_bytes()]),
            ))
        }

        // `keylet_check`: [Bytes(account:20), Value(seq), Unused x4].
        KEYLET_CHECK => {
            let account = bytes_exact(&args[0], 20)?;
            let seq = seq_or_hash_bytes(&args[1])?;
            for a in &args[2..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::CHECK,
                index_hash(space::CHECK, &[account, &seq]),
            ))
        }

        // `keylet_deposit_preauth`: [Bytes(owner:20), Bytes(authorized:20), Unused x4].
        KEYLET_DEPOSIT_PREAUTH => {
            let owner = bytes_exact(&args[0], 20)?;
            let authorized = bytes_exact(&args[1], 20)?;
            for a in &args[2..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::DEPOSIT_PREAUTH,
                index_hash(space::DEPOSIT_PREAUTH, &[owner, authorized]),
            ))
        }

        // `keylet_unchecked`: [Bytes(hash:32), Unused x5] — `key` passed
        // through verbatim, *not* hashed (`keylet::unchecked`); type `ltANY`
        // (`0`).
        KEYLET_UNCHECKED => {
            let hash = bytes_exact(&args[0], 32)?;
            for a in &args[1..6] {
                require_unused(a)?;
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(hash);
            Ok(keylet(lt::ANY, key))
        }

        // `keylet_owner_dir`: [Bytes(account:20), Unused x5].
        KEYLET_OWNER_DIR => {
            let account = bytes_exact(&args[0], 20)?;
            for a in &args[1..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::DIR_NODE,
                index_hash(space::OWNER_DIR, &[account]),
            ))
        }

        // `keylet_page`: [Bytes(root:32), Value(high), Value(low), Unused x3].
        // `index == 0` returns `root` itself, unhashed (`keylet::page`'s
        // short-circuit — the root directory page *is* its own keylet).
        KEYLET_PAGE => {
            let root = bytes_exact(&args[0], 32)?;
            let high = require_value(&args[1])?;
            let low = require_value(&args[2])?;
            for a in &args[3..6] {
                require_unused(a)?;
            }
            let index = (u64::from(high) << 32) | u64::from(low);
            let key = if index == 0 {
                let mut k = [0u8; 32];
                k.copy_from_slice(root);
                k
            } else {
                index_hash(space::DIR_NODE, &[root, &index.to_be_bytes()])
            };
            Ok(keylet(lt::DIR_NODE, key))
        }

        // `keylet_escrow`: [Bytes(account:20), Value(seq), Unused x4].
        KEYLET_ESCROW => {
            let account = bytes_exact(&args[0], 20)?;
            let seq = seq_or_hash_bytes(&args[1])?;
            for a in &args[2..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::ESCROW,
                index_hash(space::ESCROW, &[account, &seq]),
            ))
        }

        // `keylet_paychan`: [Bytes(src:20), Bytes(dst:20), Value(seq), Unused x3].
        // Upstream (`applyHook.cpp:2512`, the `PAYCHAN` switch arm) rejects
        // a zero `e` (the raw seq slot) unconditionally, before even
        // branching on whether it's a scalar or a 32-byte-hash pointer —
        // reproduced here for the scalar form, the only one any typed
        // `rshooks::api::keylet` caller ever constructs (see
        // `seq_or_hash_bytes`'s own doc comment). `OFFER`/`CHECK`/`ESCROW`/
        // `NFT_OFFER` below have no such check in their own upstream arms —
        // a zero seq is legal there, so this rejection is PAYCHAN-specific.
        KEYLET_PAYCHAN => {
            let src = bytes_exact(&args[0], 20)?;
            let dst = bytes_exact(&args[1], 20)?;
            if matches!(args[2], KeyletArg::Value(0)) {
                return Err(INVALID_ARGUMENT);
            }
            let seq = seq_or_hash_bytes(&args[2])?;
            for a in &args[3..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::PAYCHAN,
                index_hash(space::PAYMENT_CHANNEL, &[src, dst, &seq]),
            ))
        }

        // `keylet_emitted` (`KEYLET_EMITTED` = `EMITTED_TXN`):
        // [Bytes(hash:32), Unused x5].
        KEYLET_EMITTED => {
            let hash = bytes_exact(&args[0], 32)?;
            for a in &args[1..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::EMITTED_TXN,
                index_hash(space::EMITTED_TXN, &[hash]),
            ))
        }

        // `keylet_nft_offer`: [Bytes(account:20), Value(seq), Unused x4].
        KEYLET_NFT_OFFER => {
            let account = bytes_exact(&args[0], 20)?;
            let seq = seq_or_hash_bytes(&args[1])?;
            for a in &args[2..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::NFTOKEN_OFFER,
                index_hash(space::NFTOKEN_OFFER, &[account, &seq]),
            ))
        }

        // `keylet_hook_definition`: [Bytes(hash:32), Unused x5].
        KEYLET_HOOK_DEFINITION => {
            let hash = bytes_exact(&args[0], 32)?;
            for a in &args[1..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::HOOK_DEFINITION,
                index_hash(space::HOOK_DEFINITION, &[hash]),
            ))
        }

        // `keylet_hook_state_dir`: [Bytes(account:20), Bytes(ns:32), Unused x4].
        KEYLET_HOOK_STATE_DIR => {
            let account = bytes_exact(&args[0], 20)?;
            let ns = bytes_exact(&args[1], 32)?;
            for a in &args[2..6] {
                require_unused(a)?;
            }
            Ok(keylet(
                lt::DIR_NODE,
                index_hash(space::HOOK_STATE_DIR, &[account, ns]),
            ))
        }

        // `keylet_cron`: [Bytes(account:20), Value(start_time), Unused x4].
        // Not `indexHash`-based end to end: first 8 bytes of the key are the
        // first 8 bytes of `indexHash(CRON)` (no args); next 4 are
        // `start_time` big-endian; last 20 are the first 20 bytes of
        // `indexHash(CRON, start_time, account)` (`keylet::cron`).
        KEYLET_CRON => {
            let account = bytes_exact(&args[0], 20)?;
            let start_time = require_value(&args[1])?;
            for a in &args[2..6] {
                require_unused(a)?;
            }
            let ns = index_hash(space::CRON, &[]);
            let ts_be = start_time.to_be_bytes();
            let acc_hash = index_hash(space::CRON, &[&ts_be, account]);
            let mut key = [0u8; 32];
            key[0..8].copy_from_slice(&ns[0..8]);
            key[8..12].copy_from_slice(&ts_be);
            key[12..32].copy_from_slice(&acc_hash[0..20]);
            Ok(keylet(lt::CRON, key))
        }

        // Unknown/unhandled type — matches the real switch's fallthrough
        // (`return INVALID_ARGUMENT;` after the `try`/`catch`).
        _ => Err(INVALID_ARGUMENT),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt, docs/DESIGN.md §8

    use super::*;

    /// The 34-byte expected keylet for a `(type, index)` pair — the same
    /// "type prefix ++ index" shape `e2e/test/keylets.test.ts`'s
    /// `typedKeyletHex`/`keyletHex` helpers produce, but built directly from
    /// bytes (this crate has no `hex` dependency).
    fn expect(ty: u16, index: [u8; 32]) -> [u8; 34] {
        let mut out = [0u8; 34];
        out[0..2].copy_from_slice(&ty.to_be_bytes());
        out[2..34].copy_from_slice(&index);
        out
    }

    // Same fixed test inputs as `examples/13_keylets` (that example's own
    // `owner`/`dest` come from the invoking transaction at e2e time; a fixed
    // byte pattern here exercises the identical formula independent of any
    // one e2e run's account, per the module doc comment's "cross-checked
    // against the documented ledger-space/type-code formula directly" note).
    const OWNER: [u8; 20] = [0x11u8; 20];
    const TEST_HASH: [u8; 32] = [0xABu8; 32];
    const OFFER_SEQ: u32 = 1;
    const ESCROW_SEQ: u32 = 2;
    const CHECK_SEQ: u32 = 3;

    #[test]
    fn amendments_matches_e2e_formula() {
        // e2e: keyletHex('f', sha512Half(spacePrefixHex('f')))
        let got = util_keylet(
            KEYLET_AMENDMENTS,
            [
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&(b'f' as u16).to_be_bytes());
        assert_eq!(got, expect(0x0066, expected_index));
    }

    #[test]
    fn fees_type_code_differs_from_hash_space() {
        // e2e: typedKeyletHex('0073', sha512Half(spacePrefixHex('e')))
        let got = util_keylet(
            KEYLET_FEES,
            [
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&(b'e' as u16).to_be_bytes());
        assert_eq!(got, expect(0x0073, expected_index));
    }

    #[test]
    fn unchecked_is_type_zero_plus_raw_hash() {
        let got = util_keylet(
            KEYLET_UNCHECKED,
            [
                KeyletArg::Bytes(&TEST_HASH),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        assert_eq!(got, expect(0x0000, TEST_HASH));
    }

    #[test]
    fn account_matches_e2e_formula() {
        let got = util_keylet(
            KEYLET_ACCOUNT,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'a' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf
        });
        assert_eq!(got, expect(0x0061, expected_index));
    }

    #[test]
    fn check_matches_e2e_formula() {
        let got = util_keylet(
            KEYLET_CHECK,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Value(CHECK_SEQ),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'C' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf.extend_from_slice(&CHECK_SEQ.to_be_bytes());
            buf
        });
        assert_eq!(got, expect(0x0043, expected_index));
    }

    #[test]
    fn deposit_preauth_matches_e2e_formula() {
        let dest = [0x22u8; 20];
        let got = util_keylet(
            KEYLET_DEPOSIT_PREAUTH,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Bytes(&dest),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'p' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf.extend_from_slice(&dest);
            buf
        });
        assert_eq!(got, expect(0x0070, expected_index));
    }

    #[test]
    fn owner_dir_type_code_differs_from_hash_space() {
        let got = util_keylet(
            KEYLET_OWNER_DIR,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'O' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf
        });
        assert_eq!(got, expect(0x0064, expected_index));
    }

    #[test]
    fn offer_escrow_use_seq_not_hash() {
        let offer = util_keylet(
            KEYLET_OFFER,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Value(OFFER_SEQ),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'o' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf.extend_from_slice(&OFFER_SEQ.to_be_bytes());
            buf
        });
        assert_eq!(offer, expect(0x006f, expected_index));

        let escrow = util_keylet(
            KEYLET_ESCROW,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Value(ESCROW_SEQ),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_escrow_index = sha512_half(&{
            let mut buf = (b'u' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf.extend_from_slice(&ESCROW_SEQ.to_be_bytes());
            buf
        });
        assert_eq!(escrow, expect(0x0075, expected_escrow_index));
    }

    #[test]
    fn line_hashes_accounts_in_canonical_order_regardless_of_call_order() {
        let a = [0x01u8; 20];
        let b = [0x02u8; 20];
        let currency = [0u8; 20];
        let forward = util_keylet(
            KEYLET_LINE,
            [
                KeyletArg::Bytes(&a),
                KeyletArg::Bytes(&b),
                KeyletArg::Bytes(&currency),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let reversed = util_keylet(
            KEYLET_LINE,
            [
                KeyletArg::Bytes(&b),
                KeyletArg::Bytes(&a),
                KeyletArg::Bytes(&currency),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        assert_eq!(forward, reversed);
        let expected_index = sha512_half(&{
            let mut buf = (b'r' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&a);
            buf.extend_from_slice(&b);
            buf.extend_from_slice(&currency);
            buf
        });
        assert_eq!(forward, expect(0x0072, expected_index));
    }

    #[test]
    fn cron_layout_matches_indexes_cpp() {
        let start_time: u32 = 1_700_000_000;
        let got = util_keylet(
            KEYLET_CRON,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Value(start_time),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        assert_eq!(&got[0..2], &[0x00, 0x41]); // ltCRON, not hash-space 'L'

        let ns = sha512_half(&(b'L' as u16).to_be_bytes());
        assert_eq!(&got[2..10], &ns[0..8]);
        assert_eq!(&got[10..14], &start_time.to_be_bytes());
        let acc_hash = sha512_half(&{
            let mut buf = (b'L' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&start_time.to_be_bytes());
            buf.extend_from_slice(&OWNER);
            buf
        });
        assert_eq!(&got[14..34], &acc_hash[0..20]);
    }

    #[test]
    fn quality_overwrites_last_8_bytes_of_a_dir_keylet() {
        let dir = util_keylet(
            KEYLET_OWNER_DIR,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let got = util_keylet(
            KEYLET_QUALITY,
            [
                KeyletArg::Bytes(&dir),
                KeyletArg::Value(0x1122_3344),
                KeyletArg::Value(0x5566_7788),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        assert_eq!(&got[0..26], &dir[0..26]); // type + first 24 key bytes unchanged
        assert_eq!(
            &got[26..34],
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
        );
    }

    #[test]
    fn quality_rejects_a_non_dir_keylet() {
        let not_a_dir = [0u8; 34]; // type 0x0000, not 0x0064
        assert_eq!(
            util_keylet(
                KEYLET_QUALITY,
                [
                    KeyletArg::Bytes(&not_a_dir),
                    KeyletArg::Value(1),
                    KeyletArg::Value(1),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            ),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn page_index_zero_returns_root_unhashed() {
        let root = [0x77u8; 32];
        let got = util_keylet(
            KEYLET_PAGE,
            [
                KeyletArg::Bytes(&root),
                KeyletArg::Value(0),
                KeyletArg::Value(0),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        assert_eq!(got, expect(0x0064, root));
    }

    #[test]
    fn page_nonzero_index_hashes_root_and_index() {
        let root = [0x77u8; 32];
        let got = util_keylet(
            KEYLET_PAGE,
            [
                KeyletArg::Bytes(&root),
                KeyletArg::Value(0),
                KeyletArg::Value(1),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'd' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&root);
            buf.extend_from_slice(&1u64.to_be_bytes());
            buf
        });
        assert_eq!(got, expect(0x0064, expected_index));
    }

    #[test]
    fn child_and_unchecked_pass_the_hash_through_unhashed() {
        let child = util_keylet(
            KEYLET_CHILD,
            [
                KeyletArg::Bytes(&TEST_HASH),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        assert_eq!(&child[2..34], &TEST_HASH);
        assert_eq!(&child[0..2], &[0x1C, 0xD2]);
    }

    #[test]
    fn paychan_rejects_a_zero_seq() {
        assert_eq!(
            util_keylet(
                KEYLET_PAYCHAN,
                [
                    KeyletArg::Bytes(&OWNER),
                    KeyletArg::Bytes(&[0x22u8; 20]),
                    KeyletArg::Value(0),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            ),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn offer_check_escrow_nft_offer_accept_a_zero_seq() {
        // Unlike PAYCHAN, these four have no zero-seq rejection in their own
        // upstream `util_keylet` arms — a zero seq is legal.
        for ty in [KEYLET_OFFER, KEYLET_CHECK, KEYLET_ESCROW, KEYLET_NFT_OFFER] {
            assert!(
                util_keylet(
                    ty,
                    [
                        KeyletArg::Bytes(&OWNER),
                        KeyletArg::Value(0),
                        KeyletArg::Unused,
                        KeyletArg::Unused,
                        KeyletArg::Unused,
                        KeyletArg::Unused,
                    ],
                )
                .is_ok(),
                "keylet type {ty} unexpectedly rejected a zero seq"
            );
        }
    }

    #[test]
    fn ticket_is_always_invalid_argument() {
        assert_eq!(
            util_keylet(
                KEYLET_TICKET,
                [
                    KeyletArg::Bytes(&OWNER),
                    KeyletArg::Value(1),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            ),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn unknown_type_is_invalid_argument() {
        assert_eq!(
            util_keylet(
                9999,
                [
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            ),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn missing_required_bytes_slot_is_invalid_argument() {
        assert_eq!(
            util_keylet(
                KEYLET_ACCOUNT,
                [
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            ),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn signers_hashes_a_fixed_zero_page() {
        let got = util_keylet(
            KEYLET_SIGNERS,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'S' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf
        });
        assert_eq!(got, expect(0x0053, expected_index));
    }

    #[test]
    fn hook_matches_index_hash_formula() {
        let got = util_keylet(
            KEYLET_HOOK,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'H' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf
        });
        assert_eq!(got, expect(0x0048, expected_index));
    }

    #[test]
    fn hook_state_matches_index_hash_formula() {
        let key = [0x33u8; 32];
        let ns = [0x44u8; 32];
        let got = util_keylet(
            KEYLET_HOOK_STATE,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Bytes(&key),
                KeyletArg::Bytes(&ns),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'v' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf.extend_from_slice(&key);
            buf.extend_from_slice(&ns);
            buf
        });
        assert_eq!(got, expect(0x0076, expected_index));
    }

    #[test]
    fn hook_definition_matches_index_hash_formula() {
        let got = util_keylet(
            KEYLET_HOOK_DEFINITION,
            [
                KeyletArg::Bytes(&TEST_HASH),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'D' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&TEST_HASH);
            buf
        });
        assert_eq!(got, expect(0x0044, expected_index));
    }

    #[test]
    fn hook_state_dir_matches_index_hash_formula() {
        let ns = [0x55u8; 32];
        let got = util_keylet(
            KEYLET_HOOK_STATE_DIR,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Bytes(&ns),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'J' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf.extend_from_slice(&ns);
            buf
        });
        // `ltDIR_NODE` (0x0064), not hash-space 'J' — directory-shaped type.
        assert_eq!(got, expect(0x0064, expected_index));
    }

    #[test]
    fn emitted_matches_index_hash_formula() {
        let got = util_keylet(
            KEYLET_EMITTED,
            [
                KeyletArg::Bytes(&TEST_HASH),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'E' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&TEST_HASH);
            buf
        });
        assert_eq!(got, expect(0x0045, expected_index));
    }

    #[test]
    fn nft_offer_matches_index_hash_formula_and_type_code_differs_from_hash_space() {
        let seq: u32 = 6;
        let got = util_keylet(
            KEYLET_NFT_OFFER,
            [
                KeyletArg::Bytes(&OWNER),
                KeyletArg::Value(seq),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b'q' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&OWNER);
            buf.extend_from_slice(&seq.to_be_bytes());
            buf
        });
        // `ltNFTOKEN_OFFER` (0x0037) happens to differ from hash-space 'q'
        // only in numeric value, not in kind (both are single bytes here),
        // but pinned explicitly per this module's own doc comment.
        assert_eq!(got, expect(0x0037, expected_index));
    }

    #[test]
    fn skip_flag_zero_hashes_no_args() {
        let got = util_keylet(
            KEYLET_SKIP,
            [
                KeyletArg::Value(0),
                KeyletArg::Value(0),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&(b's' as u16).to_be_bytes());
        assert_eq!(got, expect(0x0068, expected_index)); // ltLEDGER_HASHES
    }

    #[test]
    fn skip_flag_one_hashes_ledger_shifted_right_16() {
        let ledger: u32 = 0x1234_5678;
        let got = util_keylet(
            KEYLET_SKIP,
            [
                KeyletArg::Value(ledger),
                KeyletArg::Value(1),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&{
            let mut buf = (b's' as u16).to_be_bytes().to_vec();
            buf.extend_from_slice(&(ledger >> 16).to_be_bytes());
            buf
        });
        assert_eq!(got, expect(0x0068, expected_index));
    }

    #[test]
    fn skip_bad_flag_is_invalid_argument() {
        assert_eq!(
            util_keylet(
                KEYLET_SKIP,
                [
                    KeyletArg::Value(0),
                    KeyletArg::Value(2),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            ),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn negative_unl_matches_index_hash_formula() {
        let got = util_keylet(
            KEYLET_NEGATIVE_UNL,
            [
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        )
        .unwrap();
        let expected_index = sha512_half(&(b'N' as u16).to_be_bytes());
        assert_eq!(got, expect(0x004e, expected_index));
    }

    #[test]
    fn hook_hook_state_hook_definition_hook_state_dir_and_emitted_well_formed() {
        for got in [
            util_keylet(
                KEYLET_HOOK,
                [
                    KeyletArg::Bytes(&OWNER),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            )
            .unwrap(),
            util_keylet(
                KEYLET_HOOK_STATE,
                [
                    KeyletArg::Bytes(&OWNER),
                    KeyletArg::Bytes(&TEST_HASH),
                    KeyletArg::Bytes(&TEST_HASH),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            )
            .unwrap(),
            util_keylet(
                KEYLET_HOOK_DEFINITION,
                [
                    KeyletArg::Bytes(&TEST_HASH),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            )
            .unwrap(),
            util_keylet(
                KEYLET_HOOK_STATE_DIR,
                [
                    KeyletArg::Bytes(&OWNER),
                    KeyletArg::Bytes(&TEST_HASH),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            )
            .unwrap(),
            util_keylet(
                KEYLET_EMITTED,
                [
                    KeyletArg::Bytes(&TEST_HASH),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            )
            .unwrap(),
            util_keylet(
                KEYLET_NFT_OFFER,
                [
                    KeyletArg::Bytes(&OWNER),
                    KeyletArg::Value(6),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            )
            .unwrap(),
            util_keylet(
                KEYLET_PAYCHAN,
                [
                    KeyletArg::Bytes(&OWNER),
                    KeyletArg::Bytes(&[0x22u8; 20]),
                    KeyletArg::Value(5),
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                    KeyletArg::Unused,
                ],
            )
            .unwrap(),
        ] {
            assert_eq!(got.len(), 34);
            assert_ne!(got, [0u8; 34]);
        }
    }
}
