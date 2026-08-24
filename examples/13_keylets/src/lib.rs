//! Computes keylets from deterministic inputs and stores them in Hook state.
//!
//! `Ticket` remains in [`KeyletKey`] to preserve key ordering, but is not
//! computed because the host does not support `keylet_ticket`.
//!
//! Every keylet is computed through its `_into` out-param twin (e.g.
//! [`keylet_hook_into`]) rather than the by-value `keylet_xxx` form: the
//! result is about to be borrowed into [`state_set`] anyway, and writing
//! directly into this function's own `Keylet` local avoids the extra copy
//! the by-value form's own scratch buffer would otherwise require (see
//! `rshooks::api::keylet`'s module doc comment's "`_into` twins" section).

#![no_std]

use rshooks::prelude::*;
use rshooks::*;

state_keys! {
    /// One state entry per keylet type.
    enum KeyletKey {
        Hook,
        HookState,
        Account,
        Amendments,
        Child,
        Skip,
        Fees,
        NegativeUnl,
        Line,
        Offer,
        Quality,
        EmittedDir,
        /// Preserved for state-key ordering; not computed by this example.
        Ticket,
        Signers,
        Check,
        DepositPreauth,
        Unchecked,
        OwnerDir,
        Page,
        Escrow,
        Paychan,
        Emitted,
        NftOffer,
        HookDefinition,
        HookStateDir,
        Cron,
    }
}

hook_errors! {
    /// `keylets` rollback codes.
    pub enum KeyletsError {
        /// The originating transaction has no `sfAccount` field.
        AccountFieldMissing = 1,
        /// The originating transaction has no `sfDestination` field.
        DestinationFieldMissing = 2,
        /// Writing a computed keylet to hook state failed.
        StateWriteFailed = 4,
    }
}

/// Deterministic hash input.
const TEST_HASH: Hash = Hash([0xAB; 32]);

/// Deterministic Hook-state key input.
const TEST_STATE_KEY: StateKey = StateKey([0xCD; 32]);

/// Deterministic Hook-state namespace input.
const TEST_NAMESPACE: NameSpace = NameSpace([0xEF; 32]);

/// `USD` in the standard 20-byte currency encoding.
const TEST_CURRENCY: CurrencyCode = CurrencyCode([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0,
]);

/// Directory-node keylet input for `keylet_quality`.
const TEST_DIR: Keylet = Keylet([
    0x00, 0x64, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB,
    0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB,
    0xAB, 0xAB,
]);

const OFFER_SEQ: u32 = 1;
const ESCROW_SEQ: u32 = 2;
const CHECK_SEQ: u32 = 3;
const PAYCHAN_SEQ: u32 = 5;
const NFT_OFFER_SEQ: u32 = 6;
/// Start time input for `keylet_cron`.
const CRON_START_TIME: u32 = 1_700_000_000;
const QUALITY_HIGH: u32 = 10;
const QUALITY_LOW: u32 = 20;
const PAGE_INDEX_HIGH: u32 = 1;
const PAGE_INDEX_LOW: u32 = 2;

/// Rolls back with `100 + keylet_type` if `result` (a `keylet_xxx_into`
/// call's own outcome) failed — the corresponding `out` is only meaningful
/// to read afterward.
#[inline(always)]
fn check(keylet_type: u32, result: Result<()>) {
    let Ok(()) = result else {
        rollback!(
            b"keylets: a keylet_xxx call failed",
            100i64.wrapping_add(keylet_type as i64)
        )
    };
}

/// Stores a keylet in Hook state.
#[inline(always)]
fn store(key: &KeyletKey, value: &Keylet) {
    if state_set(value.as_ref(), &key.encode()).is_err() {
        rollback!(b"keylets: state_set failed", KeyletsError::StateWriteFailed);
    }
}

#[hooks]
pub struct Keylets;

#[hooks]
impl Keylets {
    /// Hook entry point. See the module doc comment for the full behavior.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let Ok(owner) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"keylets: sfAccount missing from the originating transaction",
                KeyletsError::AccountFieldMissing
            )
        };
        let Ok(dest) = otxn_field_typed(sfDestination) else {
            rollback!(
                b"keylets: sfDestination missing from the originating transaction",
                KeyletsError::DestinationFieldMissing
            )
        };

        let mut k = Keylet::default();
        check(KEYLET_HOOK, keylet_hook_into(&mut k, &owner));
        store(&KeyletKey::Hook, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_HOOK_STATE,
            keylet_hook_state_into(&mut k, &owner, &TEST_STATE_KEY, &TEST_NAMESPACE),
        );
        store(&KeyletKey::HookState, &k);

        let mut k = Keylet::default();
        check(KEYLET_ACCOUNT, keylet_account_into(&mut k, &owner));
        store(&KeyletKey::Account, &k);

        let mut k = Keylet::default();
        check(KEYLET_AMENDMENTS, keylet_amendments_into(&mut k));
        store(&KeyletKey::Amendments, &k);

        let mut k = Keylet::default();
        check(KEYLET_CHILD, keylet_child_into(&mut k, &TEST_HASH));
        store(&KeyletKey::Child, &k);

        let mut k = Keylet::default();
        check(KEYLET_SKIP, keylet_skip_into(&mut k, None));
        store(&KeyletKey::Skip, &k);

        let mut k = Keylet::default();
        check(KEYLET_FEES, keylet_fees_into(&mut k));
        store(&KeyletKey::Fees, &k);

        let mut k = Keylet::default();
        check(KEYLET_NEGATIVE_UNL, keylet_negative_unl_into(&mut k));
        store(&KeyletKey::NegativeUnl, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_LINE,
            keylet_line_into(&mut k, &owner, &dest, &TEST_CURRENCY),
        );
        store(&KeyletKey::Line, &k);

        let mut k = Keylet::default();
        check(KEYLET_OFFER, keylet_offer_into(&mut k, &owner, OFFER_SEQ));
        store(&KeyletKey::Offer, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_QUALITY,
            keylet_quality_into(&mut k, &TEST_DIR, QUALITY_HIGH, QUALITY_LOW),
        );
        store(&KeyletKey::Quality, &k);

        let mut k = Keylet::default();
        check(KEYLET_EMITTED_DIR, keylet_emitted_dir_into(&mut k));
        store(&KeyletKey::EmittedDir, &k);

        let mut k = Keylet::default();
        check(KEYLET_SIGNERS, keylet_signers_into(&mut k, &owner));
        store(&KeyletKey::Signers, &k);

        let mut k = Keylet::default();
        check(KEYLET_CHECK, keylet_check_into(&mut k, &owner, CHECK_SEQ));
        store(&KeyletKey::Check, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_DEPOSIT_PREAUTH,
            keylet_deposit_preauth_into(&mut k, &owner, &dest),
        );
        store(&KeyletKey::DepositPreauth, &k);

        let mut k = Keylet::default();
        check(KEYLET_UNCHECKED, keylet_unchecked_into(&mut k, &TEST_HASH));
        store(&KeyletKey::Unchecked, &k);

        let mut k = Keylet::default();
        check(KEYLET_OWNER_DIR, keylet_owner_dir_into(&mut k, &owner));
        store(&KeyletKey::OwnerDir, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_PAGE,
            keylet_page_into(&mut k, &TEST_HASH, PAGE_INDEX_HIGH, PAGE_INDEX_LOW),
        );
        store(&KeyletKey::Page, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_ESCROW,
            keylet_escrow_into(&mut k, &owner, ESCROW_SEQ),
        );
        store(&KeyletKey::Escrow, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_PAYCHAN,
            keylet_paychan_into(&mut k, &owner, &dest, PAYCHAN_SEQ),
        );
        store(&KeyletKey::Paychan, &k);

        let mut k = Keylet::default();
        check(KEYLET_EMITTED, keylet_emitted_into(&mut k, &TEST_HASH));
        store(&KeyletKey::Emitted, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_NFT_OFFER,
            keylet_nft_offer_into(&mut k, &owner, NFT_OFFER_SEQ),
        );
        store(&KeyletKey::NftOffer, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_HOOK_DEFINITION,
            keylet_hook_definition_into(&mut k, &TEST_HASH),
        );
        store(&KeyletKey::HookDefinition, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_HOOK_STATE_DIR,
            keylet_hook_state_dir_into(&mut k, &owner, &TEST_NAMESPACE),
        );
        store(&KeyletKey::HookStateDir, &k);

        let mut k = Keylet::default();
        check(
            KEYLET_CRON,
            keylet_cron_into(&mut k, &owner, CRON_START_TIME),
        );
        store(&KeyletKey::Cron, &k);

        Ok(Accept::new(b"keylets: ok", 0))
    }
}
