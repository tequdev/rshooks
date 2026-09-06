//! One typed helper per [`rshooks_core::consts`] `KEYLET_*` constant, built
//! on top of [`crate::api::util::util_keylet_buf`] — the untyped,
//! one-function-for-every-type escape hatch that takes `keylet_type` and up
//! to six raw `u32` components (`a`..`f`) and stays available for anything
//! not covered below (or a future protocol keylet type this crate hasn't
//! caught up with yet). Each also has a `keylet_xxx_into(out: &mut Keylet,
//! ...) -> Result<()>` out-param twin (see "`_into` twins" below).
//!
//! # Why typed helpers, and why one per type
//!
//! [`util_keylet`]/[`util_keylet_buf`] take
//! `a`..`f` as bare `u32`s — some are raw values (a sequence number, a
//! quality component), others are **pointers** into this hook's own linear
//! memory (an account ID, a hash, a currency code), and which is which, how
//! many of the six are used, and what they mean all depend silently on
//! `keylet_type`. Nothing at the type level stops passing an account
//! pointer where a sequence number was expected, or omitting a component a
//! given type requires. Get it wrong and the host either fails loudly
//! (`NO_SUCH_KEYLET`/`INVALID_ARGUMENT`) or, worse, silently resolves to
//! the wrong ledger entry — the typed helpers below exist to make that
//! mistake unrepresentable.
//!
//! Every function below instead takes exactly the fixed-size
//! `rshooks::types` newtype(s) and/or plain integer(s) its own keylet type
//! needs — [`keylet_account`] takes an `&AccountId` and nothing else,
//! [`keylet_line`] takes two `&AccountId`s and a `&CurrencyCode`,
//! [`keylet_offer`] takes an `&AccountId` and a `u32` sequence. Every one is
//! a thin, `#[inline(always)]` pass-through to [`util_keylet_buf`]
//! (computing each pointer/length pair via `.as_ptr()`/`.len()` on the
//! newtype argument, `0` for every unused `a`..`f` slot), so none of this
//! costs anything beyond the raw host call itself.
//!
//! # `_into` twins
//!
//! Every function above has a `keylet_xxx_into(out: &mut Keylet, ...) ->
//! Result<()>` twin below it that writes the computed `Keylet` straight
//! into caller-supplied storage via [`util_keylet`] instead of returning
//! one by value. Reach for it when the result is about to be borrowed into
//! another buffer-taking call right away: the by-value form's own scratch
//! buffer has its address taken by the host call, which stops the
//! optimizer from eliding the copy into the caller's actual destination on
//! return — so an extra ~34-byte copy survives even under
//! `#[inline(always)]`. Writing straight into the caller's own storage has
//! no such intermediate to copy from.
//!
//! **Each `_into` twin has its own independent implementation — it does
//! not call, and is not called by, its by-value sibling.** An inlined
//! delegation wrapper's own local `out` has the same address-taken problem
//! the `_into` twins exist to avoid, so routing the by-value form through
//! its `_into` twin buys nothing at a call site that only uses the
//! by-value API, and costs a small but measurable amount of extra
//! worst-case instructions from the added call-graph shape — hence the
//! two families duplicate the host-call plumbing instead of one calling
//! the other. `testenv_keylet` (below) is the one piece actually shared
//! between them (pure interception-side bookkeeping, no wasm-side cost).
//! [`keylet_fn`] is the macro that emits both bodies for every type below
//! that doesn't need type-specific branching (`keylet_skip`'s `Option`
//! handling is written out by hand instead).
//!
//! # Source of truth
//!
//! Every `KEYLET_*` constant this module covers comes from
//! [`rshooks_core::consts`] (generated from the vendored `hook/hookapi.h` —
//! see `rshooks-core`'s own module doc comment), and every function below
//! is named `keylet_xxx` for the constant `KEYLET_XXX` it wraps.
//! [`keylet_emitted`] is the corresponding helper for `KEYLET_EMITTED`.

use crate::api::util::{util_keylet, util_keylet_buf};
use crate::error::Result;
use crate::types::{AccountId, CurrencyCode, Hash, IssuedAsset, Keylet, NameSpace, StateKey};
use rshooks_core::consts::{
    KEYLET_ACCOUNT, KEYLET_AMENDMENTS, KEYLET_CHECK, KEYLET_CHILD, KEYLET_CRON,
    KEYLET_DEPOSIT_PREAUTH, KEYLET_EMITTED, KEYLET_EMITTED_DIR, KEYLET_ESCROW, KEYLET_FEES,
    KEYLET_HOOK, KEYLET_HOOK_DEFINITION, KEYLET_HOOK_STATE, KEYLET_HOOK_STATE_DIR, KEYLET_LINE,
    KEYLET_NEGATIVE_UNL, KEYLET_NFT_OFFER, KEYLET_OFFER, KEYLET_OWNER_DIR, KEYLET_PAGE,
    KEYLET_PAYCHAN, KEYLET_QUALITY, KEYLET_SIGNERS, KEYLET_SKIP, KEYLET_TICKET, KEYLET_UNCHECKED,
};

#[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
use rshooks_core::backend::KeyletArg;

/// Testenv interception shared by every typed helper below (both families —
/// see the module doc comment's "`_into` twins" section for why the
/// by-value and `_into` forms don't call each other but do share this).
/// Unlike an `_exact`/`_typed` composing helper elsewhere in this crate,
/// neither `util_keylet_buf` nor `util_keylet` still has real slices by the
/// time a typed helper calls it, so interception happens here, one level
/// up, where `account`/`hash`/... are still real references (mirrors
/// `api::state`'s private `opt_in`/`foreign_target` helpers). The
/// wasm/no-backend fallback in every typed helper below still calls
/// `util_keylet_buf`/`util_keylet` unchanged.
#[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
#[inline(always)]
fn testenv_keylet(keylet_type: u32, args: [KeyletArg<'_>; 6]) -> Option<Result<Keylet>> {
    rshooks_core::backend::with_backend(|b| b.util_keylet(keylet_type, args))
        .map(crate::testenv_bridge::keylet_result)
}

/// Emits a `keylet_xxx`/`keylet_xxx_into` pair: the by-value form testenv-
/// intercepts then falls through to [`util_keylet_buf`], the `_into` twin
/// testenv-intercepts then falls through to [`util_keylet`] and writes
/// through `out` — see the module doc comment's "`_into` twins" section for
/// why the two bodies don't call each other.
macro_rules! keylet_fn {
    (
        $(#[$doc:meta])*
        fn $name:ident($($arg:ident: $ty:ty),* $(,)?), $into_name:ident => $konst:ident,
        [$($targ:expr),* $(,)?],
        ($($slot:expr),* $(,)?) $(,)?
    ) => {
        $(#[$doc])*
        #[inline(always)]
        pub fn $name($($arg: $ty),*) -> Result<Keylet> {
            #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
            if let Some(r) = testenv_keylet($konst, [$($targ),*]) {
                return r;
            }
            util_keylet_buf($konst, $($slot),*)
        }

        #[doc = concat!(
            "Out-param twin of [`",
            stringify!($name),
            "`] — see the module doc comment's `_into` twins section."
        )]
        #[inline(always)]
        pub fn $into_name(out: &mut Keylet, $($arg: $ty),*) -> Result<()> {
            #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
            if let Some(r) = testenv_keylet($konst, [$($targ),*]) {
                return r.map(|k| *out = k);
            }
            let _ = util_keylet(out, $konst, $($slot),*)?;
            Ok(())
        }
    };
}

keylet_fn! {
    /// `KEYLET_HOOK` (1): the keylet for `account`'s installed `Hook` ledger
    /// object (the object holding that account's chain of hooks — distinct
    /// from [`keylet_hook_definition`], which keys a single hook's own,
    /// account-independent definition object).
    fn keylet_hook(account: &AccountId), keylet_hook_into => KEYLET_HOOK,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_HOOK_STATE` (2): the keylet for one hook-state entry —
    /// `account`'s state keyed by `key`, inside `namespace`. This is an
    /// alternate route to the same state entry [`crate::state`]'s
    /// `state_get`/`state_set_loose` (+ `_foreign` twins) read/write directly
    /// by key; reach for this when a keylet (rather than a decoded value) is
    /// what's actually needed — e.g. to pass to [`crate::api::slot::slot_set`]
    /// or another Hook API that takes a keylet.
    fn keylet_hook_state(account: &AccountId, key: &StateKey, namespace: &NameSpace), keylet_hook_state_into => KEYLET_HOOK_STATE,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Bytes(key.as_ref()),
        KeyletArg::Bytes(namespace.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (
        account.as_ptr() as u32,
        account.len() as u32,
        key.as_ptr() as u32,
        key.len() as u32,
        namespace.as_ptr() as u32,
        namespace.len() as u32,
    ),
}

keylet_fn! {
    /// `KEYLET_ACCOUNT` (3): the keylet for `account`'s own `AccountRoot`
    /// ledger object.
    fn keylet_account(account: &AccountId), keylet_account_into => KEYLET_ACCOUNT,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_AMENDMENTS` (4): the keylet for the ledger's singleton
    /// `Amendments` object. Takes no arguments — every component the host
    /// call itself takes must be `0`.
    fn keylet_amendments(), keylet_amendments_into => KEYLET_AMENDMENTS,
    [
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (0, 0, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_CHILD` (5): a keylet derived from `parent`, one level down —
    /// the same "hash a parent index to get a pseudo-account's own index"
    /// pattern the protocol uses internally for a handful of derived ledger
    /// objects.
    fn keylet_child(parent: &Hash), keylet_child_into => KEYLET_CHILD,
    [
        KeyletArg::Bytes(parent.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (parent.as_ptr() as u32, parent.len() as u32, 0, 0, 0, 0),
}

/// `KEYLET_SKIP` (6): the keylet for a `SkipList` ledger object.
/// `ledger_index`: `None` for the current skip list (the common case, at
/// its fixed well-known index); `Some(seq)` for the skip list as of a
/// specific historical ledger sequence.
#[inline(always)]
pub fn keylet_skip(ledger_index: Option<u32>) -> Result<Keylet> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    {
        let args = match ledger_index {
            Some(seq) => [
                KeyletArg::Value(seq),
                KeyletArg::Value(1),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
            None => [
                KeyletArg::Value(0),
                KeyletArg::Value(0),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        };
        if let Some(r) = testenv_keylet(KEYLET_SKIP, args) {
            return r;
        }
    }
    match ledger_index {
        Some(seq) => util_keylet_buf(KEYLET_SKIP, seq, 1, 0, 0, 0, 0),
        None => util_keylet_buf(KEYLET_SKIP, 0, 0, 0, 0, 0, 0),
    }
}

/// Out-param twin of [`keylet_skip`] — see the module doc comment's `_into`
/// twins section.
#[inline(always)]
pub fn keylet_skip_into(out: &mut Keylet, ledger_index: Option<u32>) -> Result<()> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    {
        let args = match ledger_index {
            Some(seq) => [
                KeyletArg::Value(seq),
                KeyletArg::Value(1),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
            None => [
                KeyletArg::Value(0),
                KeyletArg::Value(0),
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
                KeyletArg::Unused,
            ],
        };
        if let Some(r) = testenv_keylet(KEYLET_SKIP, args) {
            return r.map(|k| *out = k);
        }
    }
    let _ = match ledger_index {
        Some(seq) => util_keylet(out, KEYLET_SKIP, seq, 1, 0, 0, 0, 0),
        None => util_keylet(out, KEYLET_SKIP, 0, 0, 0, 0, 0, 0),
    }?;
    Ok(())
}

keylet_fn! {
    /// `KEYLET_FEES` (7): the keylet for the ledger's singleton `FeeSettings`
    /// object. Takes no arguments — every component the host call itself
    /// takes must be `0`.
    fn keylet_fees(), keylet_fees_into => KEYLET_FEES,
    [
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (0, 0, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_NEGATIVE_UNL` (8): the keylet for the ledger's singleton
    /// `NegativeUNL` object. Takes no arguments — every component the host
    /// call itself takes must be `0`.
    fn keylet_negative_unl(), keylet_negative_unl_into => KEYLET_NEGATIVE_UNL,
    [
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (0, 0, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_LINE` (9): the keylet for the trust line (`RippleState` ledger
    /// object) between `account_a` and `account_b` in `currency` — order of
    /// `account_a`/`account_b` does not matter, a trust line has no fixed
    /// "side" (the protocol canonicalizes the two accounts internally when
    /// computing the index).
    fn keylet_line(account_a: &AccountId, account_b: &AccountId, currency: &CurrencyCode), keylet_line_into => KEYLET_LINE,
    [
        KeyletArg::Bytes(account_a.as_ref()),
        KeyletArg::Bytes(account_b.as_ref()),
        KeyletArg::Bytes(currency.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (
        account_a.as_ptr() as u32,
        account_a.len() as u32,
        account_b.as_ptr() as u32,
        account_b.len() as u32,
        currency.as_ptr() as u32,
        currency.len() as u32,
    ),
}

/// [`keylet_line`] taking an [`IssuedAsset`] in place of separate
/// currency/issuer arguments — the keylet for the trust line between
/// `account` and `asset.issuer` in `asset.currency`.
#[inline(always)]
pub fn keylet_line_for_asset(account: &AccountId, asset: &IssuedAsset) -> Result<Keylet> {
    keylet_line(account, &asset.issuer, &asset.currency)
}

keylet_fn! {
    /// `KEYLET_OFFER` (10): the keylet for `account`'s `Offer` ledger object
    /// created by the transaction at sequence `seq` (an `OfferCreate`'s own
    /// `Sequence`, or the ticket sequence that authorized it).
    fn keylet_offer(account: &AccountId, seq: u32), keylet_offer_into => KEYLET_OFFER,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Value(seq),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, seq, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_QUALITY` (11): the keylet for the order book directory page at
    /// exchange rate `quality_high`/`quality_low` (the top and bottom 32 bits
    /// of the 64-bit quality value), rooted at the order-book directory `dir`.
    fn keylet_quality(dir: &Keylet, quality_high: u32, quality_low: u32), keylet_quality_into => KEYLET_QUALITY,
    [
        KeyletArg::Bytes(dir.as_ref()),
        KeyletArg::Value(quality_high),
        KeyletArg::Value(quality_low),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (
        dir.as_ptr() as u32,
        dir.len() as u32,
        quality_high,
        quality_low,
        0,
        0,
    ),
}

keylet_fn! {
    /// `KEYLET_EMITTED_DIR` (12): the keylet for the ledger's singleton
    /// directory of currently-outstanding emitted transactions. Takes no
    /// arguments — every component the host call itself takes must be `0`.
    fn keylet_emitted_dir(), keylet_emitted_dir_into => KEYLET_EMITTED_DIR,
    [
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (0, 0, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_TICKET` (13): the keylet for `account`'s `Ticket` ledger object
    /// at ticket sequence `ticket_seq`.
    ///
    /// # Known host limitation
    ///
    /// On standalone `xahaud 2026.6.21-release+3350`, `util_keylet` returns an
    /// error for `KEYLET_TICKET` regardless of `ticket_seq`, even though the
    /// identical shape is accepted by that node's `ledger_entry` RPC and every
    /// structurally similar type (`KEYLET_OFFER`/`KEYLET_ESCROW`/
    /// `KEYLET_CHECK`/`KEYLET_SIGNERS`) succeeds — a host-side gap, not a bug in
    /// this wrapper's argument marshaling. `examples/13_keylets` does not
    /// exercise this call; see its README's "e2e verification scope" section.
    fn keylet_ticket(account: &AccountId, ticket_seq: u32), keylet_ticket_into => KEYLET_TICKET,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Value(ticket_seq),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, ticket_seq, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_SIGNERS` (14): the keylet for `account`'s `SignerList` ledger
    /// object.
    fn keylet_signers(account: &AccountId), keylet_signers_into => KEYLET_SIGNERS,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_CHECK` (15): the keylet for `account`'s `Check` ledger object
    /// created by the transaction at sequence `seq`.
    fn keylet_check(account: &AccountId, seq: u32), keylet_check_into => KEYLET_CHECK,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Value(seq),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, seq, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_DEPOSIT_PREAUTH` (16): the keylet for the `DepositPreauth`
    /// ledger object recording that `owner` has preauthorized `authorized`.
    fn keylet_deposit_preauth(owner: &AccountId, authorized: &AccountId), keylet_deposit_preauth_into => KEYLET_DEPOSIT_PREAUTH,
    [
        KeyletArg::Bytes(owner.as_ref()),
        KeyletArg::Bytes(authorized.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (
        owner.as_ptr() as u32,
        owner.len() as u32,
        authorized.as_ptr() as u32,
        authorized.len() as u32,
        0,
        0,
    ),
}

keylet_fn! {
    /// `KEYLET_UNCHECKED` (17): `hash` itself, reinterpreted directly as a
    /// keylet index with no type-prefix validation — an escape hatch for a
    /// ledger index already known to be correct (e.g. one read back from
    /// another ledger object's own fields), not a *computed* keylet.
    fn keylet_unchecked(hash: &Hash), keylet_unchecked_into => KEYLET_UNCHECKED,
    [
        KeyletArg::Bytes(hash.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (hash.as_ptr() as u32, hash.len() as u32, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_OWNER_DIR` (18): the keylet for `account`'s owner directory
    /// (the root page listing every ledger object `account` owns).
    fn keylet_owner_dir(account: &AccountId), keylet_owner_dir_into => KEYLET_OWNER_DIR,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_PAGE` (19): the keylet for directory page
    /// `index_high`/`index_low` (the top and bottom 32 bits of the page
    /// index) of the directory rooted at `root` (that root directory's own
    /// 32-byte ledger index — see [`keylet_owner_dir`]/[`keylet_quality`] for
    /// how to obtain one).
    fn keylet_page(root: &Hash, index_high: u32, index_low: u32), keylet_page_into => KEYLET_PAGE,
    [
        KeyletArg::Bytes(root.as_ref()),
        KeyletArg::Value(index_high),
        KeyletArg::Value(index_low),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (
        root.as_ptr() as u32,
        root.len() as u32,
        index_high,
        index_low,
        0,
        0,
    ),
}

keylet_fn! {
    /// `KEYLET_ESCROW` (20): the keylet for `account`'s `Escrow` ledger object
    /// created by the transaction at sequence `seq`.
    fn keylet_escrow(account: &AccountId, seq: u32), keylet_escrow_into => KEYLET_ESCROW,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Value(seq),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, seq, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_PAYCHAN` (21): the keylet for the `PayChannel` ledger object
    /// from `src` to `dst` created by the transaction at sequence `seq`.
    fn keylet_paychan(src: &AccountId, dst: &AccountId, seq: u32), keylet_paychan_into => KEYLET_PAYCHAN,
    [
        KeyletArg::Bytes(src.as_ref()),
        KeyletArg::Bytes(dst.as_ref()),
        KeyletArg::Value(seq),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (
        src.as_ptr() as u32,
        src.len() as u32,
        dst.as_ptr() as u32,
        dst.len() as u32,
        seq,
        0,
    ),
}

keylet_fn! {
    /// `KEYLET_EMITTED` (22): the keylet for the `EmittedTxn` bookkeeping
    /// object tracking the previously-emitted transaction identified by
    /// `hash`. Named for the constant it wraps (`rshooks_core::consts::
    /// KEYLET_EMITTED`, not `KEYLET_EMITTED_TXN`) — see this module's doc
    /// comment.
    fn keylet_emitted(hash: &Hash), keylet_emitted_into => KEYLET_EMITTED,
    [
        KeyletArg::Bytes(hash.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (hash.as_ptr() as u32, hash.len() as u32, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_NFT_OFFER` (23): the keylet for `account`'s `NFTokenOffer`
    /// ledger object created by the transaction at sequence `seq`.
    fn keylet_nft_offer(account: &AccountId, seq: u32), keylet_nft_offer_into => KEYLET_NFT_OFFER,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Value(seq),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, seq, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_HOOK_DEFINITION` (24): the keylet for the account-independent
    /// `HookDefinition` ledger object identified by `hash` (a hook's own wasm
    /// hash, the same value `SetHook`'s `sfHookHash`/`hook_hash` names) —
    /// distinct from [`keylet_hook`], which keys a specific *account's*
    /// installed hook chain.
    fn keylet_hook_definition(hash: &Hash), keylet_hook_definition_into => KEYLET_HOOK_DEFINITION,
    [
        KeyletArg::Bytes(hash.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (hash.as_ptr() as u32, hash.len() as u32, 0, 0, 0, 0),
}

keylet_fn! {
    /// `KEYLET_HOOK_STATE_DIR` (25): the keylet for the directory listing
    /// every hook-state entry `account` has stored under `namespace`.
    fn keylet_hook_state_dir(account: &AccountId, namespace: &NameSpace), keylet_hook_state_dir_into => KEYLET_HOOK_STATE_DIR,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Bytes(namespace.as_ref()),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (
        account.as_ptr() as u32,
        account.len() as u32,
        namespace.as_ptr() as u32,
        namespace.len() as u32,
        0,
        0,
    ),
}

keylet_fn! {
    /// `KEYLET_CRON` (26): the keylet for `account`'s `Cron` ledger object
    /// starting at `start_time` (a raw ledger-time value — a `Cron` entry is
    /// indexed by *when* it next fires, not by a per-account sequence
    /// counter, unlike every other `account`-keyed type above).
    fn keylet_cron(account: &AccountId, start_time: u32), keylet_cron_into => KEYLET_CRON,
    [
        KeyletArg::Bytes(account.as_ref()),
        KeyletArg::Value(start_time),
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
        KeyletArg::Unused,
    ],
    (account.as_ptr() as u32, account.len() as u32, start_time, 0, 0, 0),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        let account = AccountId::zeroed();
        let account_b = AccountId::zeroed();
        let hash = Hash::zeroed();
        let key = StateKey::zeroed();
        let namespace = NameSpace::zeroed();
        let currency = CurrencyCode::zeroed();
        let dir = Keylet::zeroed();

        assert_eq!(keylet_hook(&account), Err(HookError::NotImplemented));
        assert_eq!(
            keylet_hook_state(&account, &key, &namespace),
            Err(HookError::NotImplemented)
        );
        assert_eq!(keylet_account(&account), Err(HookError::NotImplemented));
        assert_eq!(keylet_amendments(), Err(HookError::NotImplemented));
        assert_eq!(keylet_child(&hash), Err(HookError::NotImplemented));
        assert_eq!(keylet_skip(None), Err(HookError::NotImplemented));
        assert_eq!(keylet_skip(Some(1)), Err(HookError::NotImplemented));
        assert_eq!(keylet_fees(), Err(HookError::NotImplemented));
        assert_eq!(keylet_negative_unl(), Err(HookError::NotImplemented));
        assert_eq!(
            keylet_line(&account, &account_b, &currency),
            Err(HookError::NotImplemented)
        );
        assert_eq!(keylet_offer(&account, 1), Err(HookError::NotImplemented));
        assert_eq!(keylet_quality(&dir, 1, 1), Err(HookError::NotImplemented));
        assert_eq!(keylet_emitted_dir(), Err(HookError::NotImplemented));
        assert_eq!(keylet_ticket(&account, 1), Err(HookError::NotImplemented));
        assert_eq!(keylet_signers(&account), Err(HookError::NotImplemented));
        assert_eq!(keylet_check(&account, 1), Err(HookError::NotImplemented));
        assert_eq!(
            keylet_deposit_preauth(&account, &account_b),
            Err(HookError::NotImplemented)
        );
        assert_eq!(keylet_unchecked(&hash), Err(HookError::NotImplemented));
        assert_eq!(keylet_owner_dir(&account), Err(HookError::NotImplemented));
        assert_eq!(keylet_page(&hash, 1, 1), Err(HookError::NotImplemented));
        assert_eq!(keylet_escrow(&account, 1), Err(HookError::NotImplemented));
        assert_eq!(
            keylet_paychan(&account, &account_b, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(keylet_emitted(&hash), Err(HookError::NotImplemented));
        assert_eq!(
            keylet_nft_offer(&account, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_hook_definition(&hash),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_hook_state_dir(&account, &namespace),
            Err(HookError::NotImplemented)
        );
        assert_eq!(keylet_cron(&account, 1), Err(HookError::NotImplemented));
    }
    #[test]
    fn smoke_into_not_implemented_on_host() {
        let account = AccountId::zeroed();
        let account_b = AccountId::zeroed();
        let hash = Hash::zeroed();
        let key = StateKey::zeroed();
        let namespace = NameSpace::zeroed();
        let currency = CurrencyCode::zeroed();
        let dir = Keylet::zeroed();
        let mut out = Keylet::zeroed();

        assert_eq!(
            keylet_hook_into(&mut out, &account),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_hook_state_into(&mut out, &account, &key, &namespace),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_account_into(&mut out, &account),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_amendments_into(&mut out),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_child_into(&mut out, &hash),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_skip_into(&mut out, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_skip_into(&mut out, Some(1)),
            Err(HookError::NotImplemented)
        );
        assert_eq!(keylet_fees_into(&mut out), Err(HookError::NotImplemented));
        assert_eq!(
            keylet_negative_unl_into(&mut out),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_line_into(&mut out, &account, &account_b, &currency),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_offer_into(&mut out, &account, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_quality_into(&mut out, &dir, 1, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_emitted_dir_into(&mut out),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_ticket_into(&mut out, &account, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_signers_into(&mut out, &account),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_check_into(&mut out, &account, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_deposit_preauth_into(&mut out, &account, &account_b),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_unchecked_into(&mut out, &hash),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_owner_dir_into(&mut out, &account),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_page_into(&mut out, &hash, 1, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_escrow_into(&mut out, &account, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_paychan_into(&mut out, &account, &account_b, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_emitted_into(&mut out, &hash),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_nft_offer_into(&mut out, &account, 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_hook_definition_into(&mut out, &hash),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_hook_state_dir_into(&mut out, &account, &namespace),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            keylet_cron_into(&mut out, &account, 1),
            Err(HookError::NotImplemented)
        );
    }
}
