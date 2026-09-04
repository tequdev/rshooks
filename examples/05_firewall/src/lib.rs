#![no_std]

use rshooks::prelude::*;
use rshooks::*;

#[hooks]
pub struct Firewall {
    /// The blocked account, configured via the `BL` Hook parameter.
    #[hook_param(name = b"BL")]
    blocked: HookParam<AccountId>,
}

hook_errors! {
    /// Errors returned by the firewall hook.
    pub enum FirewallError {
        /// The originating account could not be read.
        CouldNotReadSender = 1,
        /// The originating account is blocked.
        BlockedAccount = 2,
        /// The `BL` Hook parameter is present but malformed, or the host
        /// call to read it failed for a reason other than absence.
        CouldNotReadBlocklist = 3,
    }
}

/// Reads the configured `BL` blocklist account, if any.
///
/// Only an absent `BL` resolves to "nothing to block": `HookParam::get`
/// already returns `Ok(None)` solely for that case, so any `Err` here is a
/// decode failure or another host error, not absence, and must not be
/// silently treated as "not configured".
fn blocked_account() -> Option<AccountId> {
    match Firewall.hook_param.blocked.get() {
        Ok(blocked) => blocked,
        Err(_) => rollback!(
            b"firewall: could not read BL parameter",
            FirewallError::CouldNotReadBlocklist
        ),
    }
}

#[hooks]
impl Firewall {
    /// Rejects the originating transaction if its sender matches the
    /// configured blocklist account.
    #[hook(0, on = [Payment])]
    fn main(&self) -> HookResult {
        let Ok(sender) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"firewall: could not read otxn sender",
                FirewallError::CouldNotReadSender
            )
        };

        let Some(blocked) = blocked_account() else {
            accept!()
        };

        // `AccountId`'s `==` is loop-free too, but spelling this as
        // `buf_eq_20` makes the loop-free mechanism explicit.
        if buf_eq_20(&sender, &blocked) {
            rollback!(b"firewall: blocked account", FirewallError::BlockedAccount);
        }

        accept!()
    }
}
