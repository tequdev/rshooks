//! [`TrustLineView`]: a side-aware read of one trust line (`RippleState`
//! ledger object).
//!
//! A `RippleState` object names its two participants by canonical
//! `AccountId` ordering (low = numerically smaller `AccountId`) rather than
//! by role, and every per-side field/flag comes in a Low/High pair:
//! `sfLowLimit`/`sfHighLimit`, `lsfLowFreeze`/`lsfHighFreeze`,
//! `lsfLowDeepFreeze`/`lsfHighDeepFreeze`, `lsfLowAuth`/`lsfHighAuth`. A hook
//! that inspects a trust line by hand has to re-derive which side one of its
//! two accounts landed on and pick the matching field every time — get it
//! backwards and a policy check silently inverts. `TrustLineView` does that
//! resolution once, at [`load`](TrustLineView::load), and every accessor
//! below takes the account whose fact it reports, not a Low/High field name.
//!
//! This is a **read-only, protocol-facts-only** view: it reports exactly
//! what the ledger object says (a limit, a frozen/deep-frozen/authorized
//! bit), never a policy judgment such as "can this account receive this
//! IOU" — that composition is the caller's, since it also depends on
//! context this type has no access to (e.g. the issuer's own global
//! freeze).
//!
//! Built entirely on [`crate::api::keylet::keylet_line`] and
//! [`crate::slot_obj`]; both stay available for anything this narrower view
//! does not cover (e.g. `sfBalance`, `sfLowNode`/`sfHighNode`).
//!
//! # Eager `Flags`, lazy `Limit`
//!
//! [`load`](TrustLineView::load) reads `sfFlags` once, up front — a single
//! cheap `UInt32` read every accessor but [`limit_of`](TrustLineView::limit_of)
//! needs. It does **not** also read both `sfLowLimit` and `sfHighLimit`:
//! each is a 48-byte IOU `Amount` decode, and a caller asking for one side's
//! limit essentially never wants the other, so reading both unconditionally
//! measured as a real, avoidable cost (see this crate's PR description for
//! the worst-case-instruction comparison). Instead the loaded root slot is
//! kept, and `limit_of` navigates to exactly the one field it needs, only
//! when called — the same amount of work a hand-written hook does when it
//! already knows which side it cares about.

use crate::api::keylet::keylet_line;
use crate::error::{HookError, Result};
use crate::sfield::{sfFlags, sfHighLimit, sfLowLimit};
use crate::slot_obj::SlotObject;
use crate::types::{AccountId, CurrencyCode, STObject};
use crate::xfl::XFL;
use rshooks_core::ls_flags::{
    lsfHighAuth, lsfHighDeepFreeze, lsfHighFreeze, lsfLowAuth, lsfLowDeepFreeze, lsfLowFreeze,
};

/// Which side of a trust line an account occupies — low or high by
/// canonical [`AccountId`] ordering. Returned by [`TrustLineView::side`];
/// every other per-account accessor on [`TrustLineView`] resolves this
/// internally before picking a field or flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLineSide {
    /// The numerically smaller of the two participant `AccountId`s.
    Low,
    /// The numerically larger of the two participant `AccountId`s.
    High,
}

/// The plain-data half of a trust line: which two accounts it is between
/// and its `Flags` word. Holds no slot and needs no host call to construct
/// or query, which is exactly what makes it independently unit-testable —
/// see this module's tests. Split out of [`TrustLineView`] purely for that
/// reason; every method here is also reachable through the identically
/// named public method on `TrustLineView`.
struct LineFacts {
    low: AccountId,
    high: AccountId,
    flags: u32,
}

impl LineFacts {
    #[inline(always)]
    fn side(&self, account: &AccountId) -> Result<TrustLineSide> {
        if *account == self.low {
            Ok(TrustLineSide::Low)
        } else if *account == self.high {
            Ok(TrustLineSide::High)
        } else {
            Err(HookError::DoesNotMatch)
        }
    }

    #[inline(always)]
    fn is_frozen_by(&self, account: &AccountId) -> Result<bool> {
        let bit = match self.side(account)? {
            TrustLineSide::Low => lsfLowFreeze,
            TrustLineSide::High => lsfHighFreeze,
        };
        Ok(self.flags & bit != 0)
    }

    #[inline(always)]
    fn is_deep_frozen_by(&self, account: &AccountId) -> Result<bool> {
        let bit = match self.side(account)? {
            TrustLineSide::Low => lsfLowDeepFreeze,
            TrustLineSide::High => lsfHighDeepFreeze,
        };
        Ok(self.flags & bit != 0)
    }

    #[inline(always)]
    fn is_authorized_by(&self, account: &AccountId) -> Result<bool> {
        let bit = match self.side(account)? {
            TrustLineSide::Low => lsfLowAuth,
            TrustLineSide::High => lsfHighAuth,
        };
        Ok(self.flags & bit != 0)
    }
}

/// A side-aware handle to one trust line (`RippleState` ledger object).
///
/// See the module doc comment's "Eager `Flags`, lazy `Limit`" section for
/// why this holds a live root slot rather than every field: `side`,
/// `is_frozen_by`, `is_deep_frozen_by`, and `is_authorized_by` cost nothing
/// beyond the [`LineFacts`] read already done at [`load`](Self::load);
/// [`limit_of`](Self::limit_of) makes exactly one further host round trip,
/// only for the side actually asked about.
pub struct TrustLineView {
    facts: LineFacts,
    root: SlotObject<STObject>,
}

impl TrustLineView {
    /// Loads the trust line between `account_a` and `account_b` in
    /// `currency`. Order of the two accounts does not matter — same as
    /// [`keylet_line`], which this calls directly.
    ///
    /// # Errors
    ///
    /// - Whatever [`keylet_line`] itself reports (e.g.
    ///   [`HookError::NotImplemented`] on a non-wasm, non-testenv host)
    ///   surfaces unchanged.
    /// - No such trust line: the slot load's own error (`DOESNT_EXIST` on a
    ///   real host) — distinct from the case below, which means the line
    ///   exists but its record could not be read as expected.
    /// - A missing or wrong-shaped `Flags` field: the underlying
    ///   [`SlotObject`] read's own error surfaces unchanged. Never coerced
    ///   into a default — a malformed line cannot be mistaken for an
    ///   all-zero one. (`LowLimit`/`HighLimit` are not read here — see
    ///   [`limit_of`](Self::limit_of) for their own error contract.)
    #[inline(always)]
    pub fn load(
        account_a: &AccountId,
        account_b: &AccountId,
        currency: &CurrencyCode,
    ) -> Result<Self> {
        let keylet = keylet_line(account_a, account_b, currency)?;
        let root = SlotObject::from_keylet(&keylet)?;
        let flags: u32 = root.get(sfFlags)?.value()?;
        let (low, high) = if *account_a < *account_b {
            (*account_a, *account_b)
        } else {
            (*account_b, *account_a)
        };
        Ok(Self {
            facts: LineFacts { low, high, flags },
            root,
        })
    }

    /// Which side of this line `account` occupies.
    ///
    /// # Errors
    ///
    /// [`HookError::DoesNotMatch`] if `account` is neither of this line's
    /// two participants.
    #[inline(always)]
    pub fn side(&self, account: &AccountId) -> Result<TrustLineSide> {
        self.facts.side(account)
    }

    /// The credit limit `account` itself set on this line — `sfLowLimit` if
    /// `account` is the low participant, `sfHighLimit` if high. This is the
    /// limit that participant extended to the other side, not a limit
    /// imposed on `account`.
    ///
    /// Reads and decodes exactly one of `sfLowLimit`/`sfHighLimit` from the
    /// live slot on every call (see the module doc comment) — cache the
    /// result if a hook calls this more than once for the same account.
    ///
    /// # Errors
    ///
    /// - [`HookError::DoesNotMatch`] if `account` is not a participant (see
    ///   [`side`](Self::side)).
    /// - A missing or wrong-shaped `LowLimit`/`HighLimit` field: the
    ///   underlying [`SlotObject`] read's own error surfaces unchanged.
    #[inline(always)]
    pub fn limit_of(&self, account: &AccountId) -> Result<XFL> {
        match self.facts.side(account)? {
            TrustLineSide::Low => self.root.get(sfLowLimit)?.as_xfl(),
            TrustLineSide::High => self.root.get(sfHighLimit)?.as_xfl(),
        }
    }

    /// Whether `account` has frozen the *other* side of this line —
    /// inspects `lsfLowFreeze` (the flag the low account controls) when
    /// `account` is low, `lsfHighFreeze` when high. This is the freeze
    /// `account` imposed, not whether `account` itself was frozen by its
    /// counterparty — call this with the counterparty's `AccountId` to ask
    /// that.
    ///
    /// # Errors
    ///
    /// [`HookError::DoesNotMatch`] if `account` is not a participant (see
    /// [`side`](Self::side)).
    #[inline(always)]
    pub fn is_frozen_by(&self, account: &AccountId) -> Result<bool> {
        self.facts.is_frozen_by(account)
    }

    /// Whether `account` has deep-frozen the *other* side of this line —
    /// `lsfLowDeepFreeze`/`lsfHighDeepFreeze`, same direction as
    /// [`is_frozen_by`](Self::is_frozen_by): the freeze `account` imposed,
    /// not one imposed on it.
    ///
    /// # Errors
    ///
    /// [`HookError::DoesNotMatch`] if `account` is not a participant (see
    /// [`side`](Self::side)).
    #[inline(always)]
    pub fn is_deep_frozen_by(&self, account: &AccountId) -> Result<bool> {
        self.facts.is_deep_frozen_by(account)
    }

    /// Whether `account` has authorized the *other* side to hold its
    /// issuance — `lsfLowAuth`/`lsfHighAuth`, same direction as
    /// [`is_frozen_by`](Self::is_frozen_by): the authorization `account`
    /// granted, not one granted to it.
    ///
    /// # Errors
    ///
    /// [`HookError::DoesNotMatch`] if `account` is not a participant (see
    /// [`side`](Self::side)).
    #[inline(always)]
    pub fn is_authorized_by(&self, account: &AccountId) -> Result<bool> {
        self.facts.is_authorized_by(account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOW: AccountId = AccountId([0x01u8; 20]);
    const HIGH: AccountId = AccountId([0x02u8; 20]);
    const OTHER: AccountId = AccountId([0x03u8; 20]);

    /// Builds a [`LineFacts`] directly — the only way to exercise
    /// `side`/the flag accessors without a live host, since a full
    /// [`TrustLineView`] also holds a [`SlotObject`] that only
    /// [`SlotObject::from_keylet`] (a real host call) can produce.
    /// [`TrustLineView::load`] itself and [`TrustLineView::limit_of`]'s
    /// live-slot behavior are checked separately: `load` against the
    /// `NotImplemented` stub below, and the rest in
    /// `crates/rshooks-testenv/tests/trust_line_view.rs`, which can
    /// actually populate a slot.
    fn facts(flags: u32) -> LineFacts {
        LineFacts {
            low: LOW,
            high: HIGH,
            flags,
        }
    }

    #[test]
    fn load_surfaces_the_host_stub() {
        let currency = CurrencyCode::zeroed();
        assert_eq!(
            TrustLineView::load(&LOW, &HIGH, &currency).err(),
            Some(HookError::NotImplemented)
        );
        // Order-independent, same as `keylet_line`.
        assert_eq!(
            TrustLineView::load(&HIGH, &LOW, &currency).err(),
            Some(HookError::NotImplemented)
        );
    }

    #[test]
    fn side_resolves_both_participants_and_rejects_a_stranger() {
        let f = facts(0);
        assert_eq!(f.side(&LOW), Ok(TrustLineSide::Low));
        assert_eq!(f.side(&HIGH), Ok(TrustLineSide::High));
        assert_eq!(f.side(&OTHER), Err(HookError::DoesNotMatch));
    }

    #[test]
    fn freeze_flags_are_read_per_side_and_do_not_leak_across_sides() {
        // Only the low account froze; the high account did not.
        let f = facts(lsfLowFreeze);
        assert_eq!(f.is_frozen_by(&LOW), Ok(true));
        assert_eq!(f.is_frozen_by(&HIGH), Ok(false));

        // Only the high account froze.
        let f = facts(lsfHighFreeze);
        assert_eq!(f.is_frozen_by(&LOW), Ok(false));
        assert_eq!(f.is_frozen_by(&HIGH), Ok(true));
    }

    #[test]
    fn deep_freeze_flags_are_read_per_side() {
        let f = facts(lsfLowDeepFreeze);
        assert_eq!(f.is_deep_frozen_by(&LOW), Ok(true));
        assert_eq!(f.is_deep_frozen_by(&HIGH), Ok(false));

        let f = facts(lsfHighDeepFreeze);
        assert_eq!(f.is_deep_frozen_by(&LOW), Ok(false));
        assert_eq!(f.is_deep_frozen_by(&HIGH), Ok(true));
    }

    #[test]
    fn auth_flags_are_read_per_side() {
        let f = facts(lsfLowAuth);
        assert_eq!(f.is_authorized_by(&LOW), Ok(true));
        assert_eq!(f.is_authorized_by(&HIGH), Ok(false));

        let f = facts(lsfHighAuth);
        assert_eq!(f.is_authorized_by(&LOW), Ok(false));
        assert_eq!(f.is_authorized_by(&HIGH), Ok(true));
    }

    #[test]
    fn every_flag_accessor_rejects_a_non_participant() {
        let f = facts(lsfLowFreeze | lsfHighDeepFreeze | lsfLowAuth);
        assert_eq!(f.side(&OTHER), Err(HookError::DoesNotMatch));
        assert_eq!(f.is_frozen_by(&OTHER), Err(HookError::DoesNotMatch));
        assert_eq!(f.is_deep_frozen_by(&OTHER), Err(HookError::DoesNotMatch));
        assert_eq!(f.is_authorized_by(&OTHER), Err(HookError::DoesNotMatch));
    }
}
