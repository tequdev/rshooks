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
    }
}

/// Reads the configured `BL` blocklist account, if any.
///
/// Treats a decode failure the same as an absent parameter (`.ok().flatten()`
/// collapsing both `Err` and `Ok(None)` to `None`): any read failure falls
/// straight to "nothing to block", not just an unset `BL`.
fn blocked_account() -> Option<AccountId> {
    Firewall.hook_param.blocked.get().ok().flatten()
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
