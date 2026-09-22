//! One typed helper per [`rshooks_core::consts`] `KEYLET_*` constant, built
//! on top of [`crate::api::util::util_keylet`] — the untyped,
//! one-function-for-every-type escape hatch that takes `keylet_type` and up
//! to six raw `u32` components (`a`..`f`) and stays available for anything
//! not covered below (or a future protocol keylet type this crate hasn't
//! caught up with yet). Each also has a `keylet_xxx_into(out: &mut Keylet,
//! ...) -> Result<()>` out-param twin (see "`_into` twins" below).
//!
//! # Why typed helpers, and why one per type
//!
//! [`util_keylet`]/[`util_keylet_into`] take
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
//! a thin, `#[inline(always)]` pass-through to [`util_keylet`]
//! (computing each pointer/length pair via `.as_ptr()`/`.len()` on the
//! newtype argument, `0` for every unused `a`..`f` slot), so none of this
//! costs anything beyond the raw host call itself.
//!
//! # `_into` twins
//!
//! Every function above has a `keylet_xxx_into(out: &mut Keylet, ...) ->
//! Result<()>` twin below it that writes the computed `Keylet` straight
//! into caller-supplied storage via [`util_keylet_into`] instead of returning
//! one by value. Reach for it when the result is about to be borrowed into
//! another buffer-taking call right away: the by-value form's own scratch
//! buffer has its address taken by the host call, which stops the
//! optimizer from eliding the copy into the caller's actual destination on
//! return — so an extra ~34-byte copy survives even under
//! `#[inline(always)]`. Writing straight into the caller's own storage has
//! no such intermediate to copy from.
//!
//! **Each `_into` twin in this module has its own independent
//! implementation — it does not call, and is not called by, its by-value
//! sibling.** An inlined delegation wrapper's own local `out` has the same
//! address-taken problem the `_into` twins exist to avoid, so routing the
//! by-value form through its `_into` twin buys nothing at a call site that
//! only uses the by-value API, and for the keylet family measured a small
//! amount of extra worst-case instructions from the added call-graph shape
//! (`examples/13_keylets`) — hence the two families here duplicate the
//! host-call plumbing instead of one calling the other. This is a per-family
//! measurement, not a rule: `super::fixed_buf_fn!`'s by-value forms do
//! delegate to their twins and measured byte-identical on every by-value
//! caller. `keylet_intercept` (below) is the one piece actually shared
//! between them (pure interception-side bookkeeping, no wasm-side cost).
//! [`keylet_fn`] is the macro that emits both bodies for every type below
//! that doesn't need type-specific branching (`keylet_skip`'s `Option`
//! handling is written out by hand instead). The by-value bodies call
//! [`util_keylet`], the `_into` bodies call [`util_keylet_into`] — two
//! independent public entry points on `util.rs`'s side too.
//!
//! # Source of truth
//!
//! Every `KEYLET_*` constant this module covers comes from
//! [`rshooks_core::consts`] (generated from the vendored `hook/hookapi.h` —
//! see `rshooks-core`'s own module doc comment), and every function below
//! is named `keylet_xxx` for the constant `KEYLET_XXX` it wraps.
//! [`keylet_emitted`] is the corresponding helper for `KEYLET_EMITTED`.

use crate::api::util::{util_keylet, util_keylet_into};
use crate::error::Result;
use crate::types::{AccountId, CurrencyCode, Hash, IssuedAsset, Keylet, NameSpace, StateKey};
use rshooks_core::consts::{
    KEYLET_ACCOUNT, KEYLET_AMENDMENTS, KEYLET_CHECK, KEYLET_CHILD, KEYLET_CRON,
    KEYLET_DEPOSIT_PREAUTH, KEYLET_EMITTED, KEYLET_EMITTED_DIR, KEYLET_ESCROW, KEYLET_FEES,
    KEYLET_HOOK, KEYLET_HOOK_DEFINITION, KEYLET_HOOK_STATE, KEYLET_HOOK_STATE_DIR, KEYLET_LINE,
    KEYLET_NEGATIVE_UNL, KEYLET_NFT_OFFER, KEYLET_OFFER, KEYLET_OWNER_DIR, KEYLET_PAGE,
    KEYLET_PAYCHAN, KEYLET_QUALITY, KEYLET_SIGNERS, KEYLET_SKIP, KEYLET_TICKET, KEYLET_UNCHECKED,
};

/// Emits a `keylet_xxx`/`keylet_xxx_into` pair from a bare argument list —
/// the by-value form testenv-intercepts then falls through to
/// [`util_keylet`], the `_into` twin testenv-intercepts then falls
/// through to [`util_keylet_into`] and writes through `out` (see the module
/// doc comment's "`_into` twins" section for why the two bodies don't call
/// each other). Each argument must be `$ident: &$ty` (a pointer/length component,
/// contributing `KeyletArg::Bytes`/two `u32` slots) or `$ident: u32` (a raw
/// component, contributing `KeyletArg::Value`/one `u32` slot); the muncher
/// below pads both the `[KeyletArg; 6]` array and the six-slot call to
/// `util_keylet`/`util_keylet_into` out to their fixed width, one recursion
/// step per real argument, then one step per padding slot.
macro_rules! keylet_fn {
    (
        $(#[$doc:meta])*
        fn $name:ident($($args:tt)*), $into_name:ident => $konst:ident $(,)?
    ) => {
        // The two `[() () () () () ()]` lists are remaining-slot counters,
        // one `()` per unfilled array slot / per unfilled call slot: `@args`
        // consumes one from each per real argument (two from the call-slot
        // counter for a `&$ty` argument), then `@pad_targ`/`@pad_slot`
        // consume the rest one at a time, appending `KeyletArg::Unused`/`0`.
        keylet_fn!(@args
            [$(#[$doc])*] $name $into_name $konst
            [() () () () () ()] [() () () () () ()]
            [] [] []
            $($args)*
        );
    };

    // `&$ty` argument: one array slot, two call slots (ptr, len).
    (@args [$($doc:tt)*] $name:ident $into_name:ident $konst:ident
        [$_ta:tt $($pad_targ:tt)*] [$_sa:tt $_sb:tt $($pad_slot:tt)*]
        [$($sig:tt)*] [$($targ:expr),*] [$($slot:expr),*]
        $arg:ident : & $ty:ty $(, $($rest:tt)*)?
    ) => {
        keylet_fn!(@args [$($doc)*] $name $into_name $konst
            [$($pad_targ)*] [$($pad_slot)*]
            [$($sig)* $arg: &$ty,]
            [$($targ,)* KeyletArg::Bytes($arg.as_ref())]
            [$($slot,)* $arg.as_ptr() as u32, $arg.len() as u32]
            $($($rest)*)?
        );
    };

    // `u32` argument: one array slot, one call slot.
    (@args [$($doc:tt)*] $name:ident $into_name:ident $konst:ident
        [$_ta:tt $($pad_targ:tt)*] [$_sa:tt $($pad_slot:tt)*]
        [$($sig:tt)*] [$($targ:expr),*] [$($slot:expr),*]
        $arg:ident : u32 $(, $($rest:tt)*)?
    ) => {
        keylet_fn!(@args [$($doc)*] $name $into_name $konst
            [$($pad_targ)*] [$($pad_slot)*]
            [$($sig)* $arg: u32,]
            [$($targ,)* KeyletArg::Value($arg)]
            [$($slot,)* $arg]
            $($($rest)*)?
        );
    };

    // Arguments exhausted: pad the `KeyletArg` array to six.
    (@args [$($doc:tt)*] $name:ident $into_name:ident $konst:ident
        [$($pad_targ:tt)*] [$($pad_slot:tt)*]
        [$($sig:tt)*] [$($targ:expr),*] [$($slot:expr),*]
    ) => {
        keylet_fn!(@pad_targ [$($doc)*] $name $into_name $konst
            [$($pad_targ)*] [$($pad_slot)*]
            [$($sig)*] [$($targ),*] [$($slot),*]
        );
    };
    (@pad_targ [$($doc:tt)*] $name:ident $into_name:ident $konst:ident
        [$_h:tt $($pad_targ:tt)*] [$($pad_slot:tt)*]
        [$($sig:tt)*] [$($targ:expr),*] [$($slot:expr),*]
    ) => {
        keylet_fn!(@pad_targ [$($doc)*] $name $into_name $konst
            [$($pad_targ)*] [$($pad_slot)*]
            [$($sig)*] [$($targ,)* KeyletArg::Unused] [$($slot),*]
        );
    };

    // `KeyletArg` array padded: pad the call's six `u32` slots.
    (@pad_targ [$($doc:tt)*] $name:ident $into_name:ident $konst:ident
        [] [$($pad_slot:tt)*]
        [$($sig:tt)*] [$($targ:expr),*] [$($slot:expr),*]
    ) => {
        keylet_fn!(@pad_slot [$($doc)*] $name $into_name $konst
            [$($pad_slot)*]
            [$($sig)*] [$($targ),*] [$($slot),*]
        );
    };
    (@pad_slot [$($doc:tt)*] $name:ident $into_name:ident $konst:ident
        [$_h:tt $($pad_slot:tt)*]
        [$($sig:tt)*] [$($targ:expr),*] [$($slot:expr),*]
    ) => {
        keylet_fn!(@pad_slot [$($doc)*] $name $into_name $konst
            [$($pad_slot)*]
            [$($sig)*] [$($targ),*] [$($slot,)* 0]
        );
    };

    // Both padded: emit the by-value/`_into` pair.
    (@pad_slot [$($doc:tt)*] $name:ident $into_name:ident $konst:ident
        []
        [$($sig:tt)*] [$($targ:expr),*] [$($slot:expr),*]
    ) => {
        $($doc)*
        #[inline(always)]
        pub fn $name($($sig)*) -> Result<Keylet> {
            #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
            {
                use crate::testenv_bridge::{KeyletArg, keylet_intercept};
                if let Some(r) = keylet_intercept($konst, [$($targ),*]) {
                    return r;
                }
            }
            util_keylet($konst, $($slot),*)
        }

        #[doc = concat!(
            "Out-param twin of [`",
            stringify!($name),
            "`] — see the module doc comment's `_into` twins section."
        )]
        #[inline(always)]
        pub fn $into_name(out: &mut Keylet, $($sig)*) -> Result<()> {
            #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
            {
                use crate::testenv_bridge::{KeyletArg, keylet_intercept};
                if let Some(r) = keylet_intercept($konst, [$($targ),*]) {
                    return r.map(|k| *out = k);
                }
            }
            util_keylet_into(out, $konst, $($slot),*)
        }
    };
}

keylet_fn! {
    /// `KEYLET_HOOK` (1): the keylet for `account`'s installed `Hook` ledger
    /// object (the object holding that account's chain of hooks — distinct
    /// from [`keylet_hook_definition`], which keys a single hook's own,
    /// account-independent definition object).
    fn keylet_hook(account: &AccountId), keylet_hook_into => KEYLET_HOOK,
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
}

keylet_fn! {
    /// `KEYLET_ACCOUNT` (3): the keylet for `account`'s own `AccountRoot`
    /// ledger object.
    fn keylet_account(account: &AccountId), keylet_account_into => KEYLET_ACCOUNT,
}

keylet_fn! {
    /// `KEYLET_AMENDMENTS` (4): the keylet for the ledger's singleton
    /// `Amendments` object. Takes no arguments — every component the host
    /// call itself takes must be `0`.
    fn keylet_amendments(), keylet_amendments_into => KEYLET_AMENDMENTS,
}

keylet_fn! {
    /// `KEYLET_CHILD` (5): a keylet derived from `parent`, one level down —
    /// the same "hash a parent index to get a pseudo-account's own index"
    /// pattern the protocol uses internally for a handful of derived ledger
    /// objects.
    fn keylet_child(parent: &Hash), keylet_child_into => KEYLET_CHILD,
}

/// Shared `a`/`b` component pair for `KEYLET_SKIP`: `None` for the current
/// skip list (the common case, at its fixed well-known index) is `(0, 0)`;
/// `Some(seq)` for the skip list as of a specific historical ledger sequence
/// is `(seq, 1)` (the constant `1` flags "historical" to the host call).
#[inline(always)]
fn skip_components(ledger_index: Option<u32>) -> (u32, u32) {
    match ledger_index {
        Some(seq) => (seq, 1),
        None => (0, 0),
    }
}

/// `KEYLET_SKIP` (6): the keylet for a `SkipList` ledger object.
/// `ledger_index`: `None` for the current skip list (the common case, at
/// its fixed well-known index); `Some(seq)` for the skip list as of a
/// specific historical ledger sequence.
#[inline(always)]
pub fn keylet_skip(ledger_index: Option<u32>) -> Result<Keylet> {
    let (a, b) = skip_components(ledger_index);
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    {
        use crate::testenv_bridge::{KeyletArg, keylet_intercept};
        let args = [
            KeyletArg::Value(a),
            KeyletArg::Value(b),
            KeyletArg::Unused,
            KeyletArg::Unused,
            KeyletArg::Unused,
            KeyletArg::Unused,
        ];
        if let Some(r) = keylet_intercept(KEYLET_SKIP, args) {
            return r;
        }
    }
    util_keylet(KEYLET_SKIP, a, b, 0, 0, 0, 0)
}

/// Out-param twin of [`keylet_skip`] — see the module doc comment's `_into`
/// twins section.
#[inline(always)]
pub fn keylet_skip_into(out: &mut Keylet, ledger_index: Option<u32>) -> Result<()> {
    let (a, b) = skip_components(ledger_index);
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    {
        use crate::testenv_bridge::{KeyletArg, keylet_intercept};
        let args = [
            KeyletArg::Value(a),
            KeyletArg::Value(b),
            KeyletArg::Unused,
            KeyletArg::Unused,
            KeyletArg::Unused,
            KeyletArg::Unused,
        ];
        if let Some(r) = keylet_intercept(KEYLET_SKIP, args) {
            return r.map(|k| *out = k);
        }
    }
    util_keylet_into(out, KEYLET_SKIP, a, b, 0, 0, 0, 0)
}

keylet_fn! {
    /// `KEYLET_FEES` (7): the keylet for the ledger's singleton `FeeSettings`
    /// object. Takes no arguments — every component the host call itself
    /// takes must be `0`.
    fn keylet_fees(), keylet_fees_into => KEYLET_FEES,
}

keylet_fn! {
    /// `KEYLET_NEGATIVE_UNL` (8): the keylet for the ledger's singleton
    /// `NegativeUNL` object. Takes no arguments — every component the host
    /// call itself takes must be `0`.
    fn keylet_negative_unl(), keylet_negative_unl_into => KEYLET_NEGATIVE_UNL,
}

keylet_fn! {
    /// `KEYLET_LINE` (9): the keylet for the trust line (`RippleState` ledger
    /// object) between `account_a` and `account_b` in `currency` — order of
    /// `account_a`/`account_b` does not matter, a trust line has no fixed
    /// "side" (the protocol canonicalizes the two accounts internally when
    /// computing the index).
    fn keylet_line(account_a: &AccountId, account_b: &AccountId, currency: &CurrencyCode), keylet_line_into => KEYLET_LINE,
}

/// [`keylet_line`] taking an [`IssuedAsset`] in place of separate
/// currency/issuer arguments — the keylet for the trust line between
/// `account` and `asset.issuer` in `asset.currency`.
#[inline(always)]
pub fn keylet_line_for_asset(account: &AccountId, asset: &IssuedAsset) -> Result<Keylet> {
    keylet_line(account, &asset.issuer, &asset.currency)
}

/// Out-param twin of [`keylet_line_for_asset`] — see the module doc
/// comment's `_into` twins section.
#[inline(always)]
pub fn keylet_line_for_asset_into(
    out: &mut Keylet,
    account: &AccountId,
    asset: &IssuedAsset,
) -> Result<()> {
    keylet_line_into(out, account, &asset.issuer, &asset.currency)
}

keylet_fn! {
    /// `KEYLET_OFFER` (10): the keylet for `account`'s `Offer` ledger object
    /// created by the transaction at sequence `seq` (an `OfferCreate`'s own
    /// `Sequence`, or the ticket sequence that authorized it).
    fn keylet_offer(account: &AccountId, seq: u32), keylet_offer_into => KEYLET_OFFER,
}

keylet_fn! {
    /// `KEYLET_QUALITY` (11): the keylet for the order book directory page at
    /// exchange rate `quality_high`/`quality_low` (the top and bottom 32 bits
    /// of the 64-bit quality value), rooted at the order-book directory `dir`.
    fn keylet_quality(dir: &Keylet, quality_high: u32, quality_low: u32), keylet_quality_into => KEYLET_QUALITY,
}

keylet_fn! {
    /// `KEYLET_EMITTED_DIR` (12): the keylet for the ledger's singleton
    /// directory of currently-outstanding emitted transactions. Takes no
    /// arguments — every component the host call itself takes must be `0`.
    fn keylet_emitted_dir(), keylet_emitted_dir_into => KEYLET_EMITTED_DIR,
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
}

keylet_fn! {
    /// `KEYLET_SIGNERS` (14): the keylet for `account`'s `SignerList` ledger
    /// object.
    fn keylet_signers(account: &AccountId), keylet_signers_into => KEYLET_SIGNERS,
}

keylet_fn! {
    /// `KEYLET_CHECK` (15): the keylet for `account`'s `Check` ledger object
    /// created by the transaction at sequence `seq`.
    fn keylet_check(account: &AccountId, seq: u32), keylet_check_into => KEYLET_CHECK,
}

keylet_fn! {
    /// `KEYLET_DEPOSIT_PREAUTH` (16): the keylet for the `DepositPreauth`
    /// ledger object recording that `owner` has preauthorized `authorized`.
    fn keylet_deposit_preauth(owner: &AccountId, authorized: &AccountId), keylet_deposit_preauth_into => KEYLET_DEPOSIT_PREAUTH,
}

keylet_fn! {
    /// `KEYLET_UNCHECKED` (17): `hash` itself, reinterpreted directly as a
    /// keylet index with no type-prefix validation — an escape hatch for a
    /// ledger index already known to be correct (e.g. one read back from
    /// another ledger object's own fields), not a *computed* keylet.
    fn keylet_unchecked(hash: &Hash), keylet_unchecked_into => KEYLET_UNCHECKED,
}

keylet_fn! {
    /// `KEYLET_OWNER_DIR` (18): the keylet for `account`'s owner directory
    /// (the root page listing every ledger object `account` owns).
    fn keylet_owner_dir(account: &AccountId), keylet_owner_dir_into => KEYLET_OWNER_DIR,
}

keylet_fn! {
    /// `KEYLET_PAGE` (19): the keylet for directory page
    /// `index_high`/`index_low` (the top and bottom 32 bits of the page
    /// index) of the directory rooted at `root` (that root directory's own
    /// 32-byte ledger index — see [`keylet_owner_dir`]/[`keylet_quality`] for
    /// how to obtain one).
    fn keylet_page(root: &Hash, index_high: u32, index_low: u32), keylet_page_into => KEYLET_PAGE,
}

keylet_fn! {
    /// `KEYLET_ESCROW` (20): the keylet for `account`'s `Escrow` ledger object
    /// created by the transaction at sequence `seq`.
    fn keylet_escrow(account: &AccountId, seq: u32), keylet_escrow_into => KEYLET_ESCROW,
}

keylet_fn! {
    /// `KEYLET_PAYCHAN` (21): the keylet for the `PayChannel` ledger object
    /// from `src` to `dst` created by the transaction at sequence `seq`.
    fn keylet_paychan(src: &AccountId, dst: &AccountId, seq: u32), keylet_paychan_into => KEYLET_PAYCHAN,
}

keylet_fn! {
    /// `KEYLET_EMITTED` (22): the keylet for the `EmittedTxn` bookkeeping
    /// object tracking the previously-emitted transaction identified by
    /// `hash`. Named for the constant it wraps (`rshooks_core::consts::
    /// KEYLET_EMITTED`, not `KEYLET_EMITTED_TXN`) — see this module's doc
    /// comment.
    fn keylet_emitted(hash: &Hash), keylet_emitted_into => KEYLET_EMITTED,
}

keylet_fn! {
    /// `KEYLET_NFT_OFFER` (23): the keylet for `account`'s `NFTokenOffer`
    /// ledger object created by the transaction at sequence `seq`.
    fn keylet_nft_offer(account: &AccountId, seq: u32), keylet_nft_offer_into => KEYLET_NFT_OFFER,
}

keylet_fn! {
    /// `KEYLET_HOOK_DEFINITION` (24): the keylet for the account-independent
    /// `HookDefinition` ledger object identified by `hash` (a hook's own wasm
    /// hash, the same value `SetHook`'s `sfHookHash`/`hook_hash` names) —
    /// distinct from [`keylet_hook`], which keys a specific *account's*
    /// installed hook chain.
    fn keylet_hook_definition(hash: &Hash), keylet_hook_definition_into => KEYLET_HOOK_DEFINITION,
}

keylet_fn! {
    /// `KEYLET_HOOK_STATE_DIR` (25): the keylet for the directory listing
    /// every hook-state entry `account` has stored under `namespace`.
    fn keylet_hook_state_dir(account: &AccountId, namespace: &NameSpace), keylet_hook_state_dir_into => KEYLET_HOOK_STATE_DIR,
}

keylet_fn! {
    /// `KEYLET_CRON` (26): the keylet for `account`'s `Cron` ledger object
    /// starting at `start_time` (a raw ledger-time value — a `Cron` entry is
    /// indexed by *when* it next fires, not by a per-account sequence
    /// counter, unlike every other `account`-keyed type above).
    fn keylet_cron(account: &AccountId, start_time: u32), keylet_cron_into => KEYLET_CRON,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    // One representative `keylet_fn!`-generated pair (`keylet_offer`
    // exercises both the `&$ty` and `u32` argument kinds) plus the
    // hand-written `keylet_skip`/`keylet_line_for_asset` covers every
    // family in this module; every other helper shares the same macro
    // expansion and host-call shape.
    #[test]
    fn smoke_not_implemented_on_host() {
        let account = AccountId::zeroed();
        let asset = IssuedAsset {
            currency: CurrencyCode::zeroed(),
            issuer: AccountId::zeroed(),
        };

        assert_eq!(keylet_offer(&account, 1), Err(HookError::NotImplemented));
        assert_eq!(keylet_skip(None), Err(HookError::NotImplemented));
        assert_eq!(keylet_skip(Some(1)), Err(HookError::NotImplemented));
        assert_eq!(
            keylet_line_for_asset(&account, &asset),
            Err(HookError::NotImplemented)
        );
    }

    #[test]
    fn smoke_into_not_implemented_on_host() {
        let account = AccountId::zeroed();
        let asset = IssuedAsset {
            currency: CurrencyCode::zeroed(),
            issuer: AccountId::zeroed(),
        };
        let mut out = Keylet::zeroed();

        assert_eq!(
            keylet_offer_into(&mut out, &account, 1),
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
        assert_eq!(
            keylet_line_for_asset_into(&mut out, &account, &asset),
            Err(HookError::NotImplemented)
        );
    }
}
