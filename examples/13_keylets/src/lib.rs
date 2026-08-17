//! Computes keylets from deterministic inputs and stores them in Hook state.
//!
//! `Ticket` remains in [`KeyletKey`] to preserve key ordering, but is not
//! computed because the host does not support `keylet_ticket`.

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

/// Returns a keylet or rolls back with `100 + keylet_type`.
#[inline(always)]
fn compute(keylet_type: u32, result: Result<Keylet>) -> Keylet {
    let Ok(k) = result else {
        rollback!(
            b"keylets: a keylet_xxx call failed",
            100i64.wrapping_add(keylet_type as i64)
        )
    };
    k
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
    fn main() -> i64 {
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

        store(&KeyletKey::Hook, &compute(KEYLET_HOOK, keylet_hook(&owner)));
        store(
            &KeyletKey::HookState,
            &compute(
                KEYLET_HOOK_STATE,
                keylet_hook_state(&owner, &TEST_STATE_KEY, &TEST_NAMESPACE),
            ),
        );
        store(
            &KeyletKey::Account,
            &compute(KEYLET_ACCOUNT, keylet_account(&owner)),
        );
        store(
            &KeyletKey::Amendments,
            &compute(KEYLET_AMENDMENTS, keylet_amendments()),
        );
        store(
            &KeyletKey::Child,
            &compute(KEYLET_CHILD, keylet_child(&TEST_HASH)),
        );
        store(&KeyletKey::Skip, &compute(KEYLET_SKIP, keylet_skip(None)));
        store(&KeyletKey::Fees, &compute(KEYLET_FEES, keylet_fees()));
        store(
            &KeyletKey::NegativeUnl,
            &compute(KEYLET_NEGATIVE_UNL, keylet_negative_unl()),
        );
        store(
            &KeyletKey::Line,
            &compute(KEYLET_LINE, keylet_line(&owner, &dest, &TEST_CURRENCY)),
        );
        store(
            &KeyletKey::Offer,
            &compute(KEYLET_OFFER, keylet_offer(&owner, OFFER_SEQ)),
        );
        store(
            &KeyletKey::Quality,
            &compute(
                KEYLET_QUALITY,
                keylet_quality(&TEST_DIR, QUALITY_HIGH, QUALITY_LOW),
            ),
        );
        store(
            &KeyletKey::EmittedDir,
            &compute(KEYLET_EMITTED_DIR, keylet_emitted_dir()),
        );
        store(
            &KeyletKey::Signers,
            &compute(KEYLET_SIGNERS, keylet_signers(&owner)),
        );
        store(
            &KeyletKey::Check,
            &compute(KEYLET_CHECK, keylet_check(&owner, CHECK_SEQ)),
        );
        store(
            &KeyletKey::DepositPreauth,
            &compute(
                KEYLET_DEPOSIT_PREAUTH,
                keylet_deposit_preauth(&owner, &dest),
            ),
        );
        store(
            &KeyletKey::Unchecked,
            &compute(KEYLET_UNCHECKED, keylet_unchecked(&TEST_HASH)),
        );
        store(
            &KeyletKey::OwnerDir,
            &compute(KEYLET_OWNER_DIR, keylet_owner_dir(&owner)),
        );
        store(
            &KeyletKey::Page,
            &compute(
                KEYLET_PAGE,
                keylet_page(&TEST_HASH, PAGE_INDEX_HIGH, PAGE_INDEX_LOW),
            ),
        );
        store(
            &KeyletKey::Escrow,
            &compute(KEYLET_ESCROW, keylet_escrow(&owner, ESCROW_SEQ)),
        );
        store(
            &KeyletKey::Paychan,
            &compute(KEYLET_PAYCHAN, keylet_paychan(&owner, &dest, PAYCHAN_SEQ)),
        );
        store(
            &KeyletKey::Emitted,
            &compute(KEYLET_EMITTED, keylet_emitted(&TEST_HASH)),
        );
        store(
            &KeyletKey::NftOffer,
            &compute(KEYLET_NFT_OFFER, keylet_nft_offer(&owner, NFT_OFFER_SEQ)),
        );
        store(
            &KeyletKey::HookDefinition,
            &compute(KEYLET_HOOK_DEFINITION, keylet_hook_definition(&TEST_HASH)),
        );
        store(
            &KeyletKey::HookStateDir,
            &compute(
                KEYLET_HOOK_STATE_DIR,
                keylet_hook_state_dir(&owner, &TEST_NAMESPACE),
            ),
        );
        store(
            &KeyletKey::Cron,
            &compute(KEYLET_CRON, keylet_cron(&owner, CRON_START_TIME)),
        );

        accept!(b"keylets: ok", 0)
    }
}
