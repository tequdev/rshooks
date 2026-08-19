//! [`Grant`]: models a `HookGrant` authorizing a foreign-namespace write —
//! see design §5.5.

/// Authorizes a foreign-namespace state write, seeded via
/// [`crate::TestEnv::grant`]. Matching is presence-only (no signature
/// checks) against the currently invoked position's hook hash and/or hook
/// account — see [`crate::TestEnv::grant`]'s doc comment for the full
/// direction/lookup story.
#[derive(Debug, Clone, Copy)]
pub struct Grant(pub(crate) Matcher);

#[derive(Debug, Clone, Copy)]
pub(crate) enum Matcher {
    HookHash([u8; 32]),
    Account([u8; 20]),
    Both { hash: [u8; 32], account: [u8; 20] },
    Any,
}

impl Grant {
    /// Authorizes any hook installed at the hash `hash`, regardless of which
    /// account it runs on.
    #[must_use]
    pub fn hook_hash(hash: [u8; 32]) -> Self {
        Self(Matcher::HookHash(hash))
    }

    /// Authorizes the hook currently invoked on account `account`,
    /// regardless of its hash.
    #[must_use]
    pub fn account(account: [u8; 20]) -> Self {
        Self(Matcher::Account(account))
    }

    /// Authorizes only the hook at hash `hash` when it is running on
    /// `account`.
    #[must_use]
    pub fn both(hash: [u8; 32], account: [u8; 20]) -> Self {
        Self(Matcher::Both { hash, account })
    }

    /// Authorizes any hook, any account — an unconditional grant.
    #[must_use]
    pub fn any() -> Self {
        Self(Matcher::Any)
    }

    /// Whether this grant authorizes the hook currently invoked at position
    /// `current_hook_hash` (`None` if the position's hash was never seeded)
    /// on account `current_hook_account`.
    pub(crate) fn matches(
        &self,
        current_hook_hash: Option<[u8; 32]>,
        current_hook_account: [u8; 20],
    ) -> bool {
        match self.0 {
            Matcher::HookHash(h) => current_hook_hash == Some(h),
            Matcher::Account(a) => current_hook_account == a,
            Matcher::Both { hash, account } => {
                current_hook_hash == Some(hash) && current_hook_account == account
            }
            Matcher::Any => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_hash_matches_by_hash_regardless_of_account() {
        let g = Grant::hook_hash([1u8; 32]);
        assert!(g.matches(Some([1u8; 32]), [2u8; 20]));
        assert!(!g.matches(Some([9u8; 32]), [2u8; 20]));
        assert!(!g.matches(None, [2u8; 20]));
    }

    #[test]
    fn account_matches_by_account_regardless_of_hash() {
        let g = Grant::account([3u8; 20]);
        assert!(g.matches(Some([1u8; 32]), [3u8; 20]));
        assert!(g.matches(None, [3u8; 20]));
        assert!(!g.matches(Some([1u8; 32]), [4u8; 20]));
    }

    #[test]
    fn both_requires_hash_and_account() {
        let g = Grant::both([1u8; 32], [3u8; 20]);
        assert!(g.matches(Some([1u8; 32]), [3u8; 20]));
        assert!(!g.matches(Some([1u8; 32]), [4u8; 20]));
        assert!(!g.matches(Some([9u8; 32]), [3u8; 20]));
    }

    #[test]
    fn any_matches_everything() {
        let g = Grant::any();
        assert!(g.matches(None, [0u8; 20]));
        assert!(g.matches(Some([7u8; 32]), [7u8; 20]));
    }
}
