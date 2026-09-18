//! Equivalence check for `rshooks::api::keylet`'s `_into` twins: for every
//! one of the 26 typed keylet helpers, the by-value `keylet_xxx(...)` form
//! and its `keylet_xxx_into(&mut out, ...)` twin must compute the exact
//! same `Result<Keylet>` for the same arguments, even though the two are
//! independent implementations that don't call each other (see
//! `rshooks::api::keylet`'s module doc comment's "`_into` twins" section
//! for why). Driven under an installed backend with **real** keylet
//! semantics — `rshooks-testenv`'s own host implementation
//! (`crates/rshooks-testenv/src/host/keylet.rs`), reached the same way
//! `examples/13_keylets/tests/keylets.rs` reaches it, through
//! [`TestEnv::invoke`] — not `spy_backend_audit.rs`'s call-counting
//! `SpyBackend`, whose `util_keylet` always returns zeroed bytes regardless
//! of arguments and so could never distinguish the two call paths.
//!
//! Guards specifically against the two independent implementations
//! silently drifting apart — passing a different argument to the
//! underlying host call, or diverging on which errors map to which
//! `HookError` — something a plain "both compile" check would miss; this
//! drives real host-side keylet computation and compares the two outcomes
//! byte-for-byte (`Result<Keylet>: PartialEq`, `HookError` included, so an
//! `Ok`/`Err` disagreement is caught too, not just a value mismatch).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::prelude::*;
use rshooks::*;
use rshooks_testenv::prelude::*;

const OWNER: [u8; 20] = [0x30u8; 20];
const DEST: [u8; 20] = [0x40u8; 20];
const TEST_HASH: Hash = Hash([0xABu8; 32]);
const TEST_STATE_KEY: StateKey = StateKey([0xCDu8; 32]);
const TEST_NAMESPACE: NameSpace = NameSpace([0xEFu8; 32]);
const TEST_CURRENCY: CurrencyCode = CurrencyCode([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0,
]);
const TEST_DIR: Keylet = Keylet([
    0x00, 0x64, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB,
    0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB,
    0xAB, 0xAB,
]);

/// Compares `by_value` (the ergonomic form's already-computed result)
/// against what `into_fn` (its `_into` twin) computes into a fresh
/// `Keylet`, rolling back with `100 + id` on any disagreement — a value
/// mismatch, an error mismatch, or one succeeding where the other failed.
#[inline(always)]
fn check<F: FnOnce(&mut Keylet) -> Result<()>>(id: u32, by_value: Result<Keylet>, into_fn: F) {
    let mut out = Keylet::default();
    let via_into = into_fn(&mut out).map(|()| out);
    if by_value != via_into {
        rollback!(
            b"keylet_xxx/keylet_xxx_into disagree",
            100i64.wrapping_add(i64::from(id))
        );
    }
}

#[hooks]
pub struct KeyletEquivalence;

#[hooks]
impl KeyletEquivalence {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let Ok(owner) = otxn_field_typed(sfAccount) else {
            rollback!(b"sfAccount missing from the originating transaction", 1i64)
        };
        let Ok(dest) = otxn_field_typed(sfDestination) else {
            rollback!(
                b"sfDestination missing from the originating transaction",
                2i64
            )
        };

        check(1, keylet_hook(&owner), |out| keylet_hook_into(out, &owner));
        check(
            2,
            keylet_hook_state(&owner, &TEST_STATE_KEY, &TEST_NAMESPACE),
            |out| keylet_hook_state_into(out, &owner, &TEST_STATE_KEY, &TEST_NAMESPACE),
        );
        check(3, keylet_account(&owner), |out| {
            keylet_account_into(out, &owner)
        });
        check(4, keylet_amendments(), keylet_amendments_into);
        check(5, keylet_child(&TEST_HASH), |out| {
            keylet_child_into(out, &TEST_HASH)
        });
        check(6, keylet_skip(None), |out| keylet_skip_into(out, None));
        check(7, keylet_skip(Some(5)), |out| {
            keylet_skip_into(out, Some(5))
        });
        check(8, keylet_fees(), keylet_fees_into);
        check(9, keylet_negative_unl(), keylet_negative_unl_into);
        check(10, keylet_line(&owner, &dest, &TEST_CURRENCY), |out| {
            keylet_line_into(out, &owner, &dest, &TEST_CURRENCY)
        });
        check(11, keylet_offer(&owner, 1), |out| {
            keylet_offer_into(out, &owner, 1)
        });
        check(12, keylet_quality(&TEST_DIR, 10, 20), |out| {
            keylet_quality_into(out, &TEST_DIR, 10, 20)
        });
        check(13, keylet_emitted_dir(), keylet_emitted_dir_into);
        check(14, keylet_ticket(&owner, 1), |out| {
            keylet_ticket_into(out, &owner, 1)
        });
        check(15, keylet_signers(&owner), |out| {
            keylet_signers_into(out, &owner)
        });
        check(16, keylet_check(&owner, 3), |out| {
            keylet_check_into(out, &owner, 3)
        });
        check(17, keylet_deposit_preauth(&owner, &dest), |out| {
            keylet_deposit_preauth_into(out, &owner, &dest)
        });
        check(18, keylet_unchecked(&TEST_HASH), |out| {
            keylet_unchecked_into(out, &TEST_HASH)
        });
        check(19, keylet_owner_dir(&owner), |out| {
            keylet_owner_dir_into(out, &owner)
        });
        check(20, keylet_page(&TEST_HASH, 1, 2), |out| {
            keylet_page_into(out, &TEST_HASH, 1, 2)
        });
        check(21, keylet_escrow(&owner, 2), |out| {
            keylet_escrow_into(out, &owner, 2)
        });
        check(22, keylet_paychan(&owner, &dest, 5), |out| {
            keylet_paychan_into(out, &owner, &dest, 5)
        });
        check(23, keylet_emitted(&TEST_HASH), |out| {
            keylet_emitted_into(out, &TEST_HASH)
        });
        check(24, keylet_nft_offer(&owner, 6), |out| {
            keylet_nft_offer_into(out, &owner, 6)
        });
        check(25, keylet_hook_definition(&TEST_HASH), |out| {
            keylet_hook_definition_into(out, &TEST_HASH)
        });
        check(26, keylet_hook_state_dir(&owner, &TEST_NAMESPACE), |out| {
            keylet_hook_state_dir_into(out, &owner, &TEST_NAMESPACE)
        });
        check(27, keylet_cron(&owner, 1_700_000_000), |out| {
            keylet_cron_into(out, &owner, 1_700_000_000)
        });

        accept!(b"ok", 0)
    }
}

#[test]
fn by_value_and_into_agree_for_all_26_types() {
    let env = TestEnv::new()
        .hook_account([0x11; 20])
        .otxn(Otxn::new(TxType::Invoke).account(OWNER).destination(DEST));
    let exit = env.invoke::<KeyletEquivalence>(0);
    assert_eq!(
        exit.code, 0,
        "keylet_xxx/keylet_xxx_into disagreed for some type (exit: {exit:?})"
    );
    assert_eq!(exit.exit, ExitType::Accept);
}
