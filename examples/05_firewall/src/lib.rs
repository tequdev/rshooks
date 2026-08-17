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
/// Deliberately masks a decode failure too (`.ok().flatten()` collapsing
/// both `Err` and `Ok(None)` to `None`), matching this example's original
/// `hook_parameter!`-based behavior of falling straight to "nothing to
/// block" on *any* read failure — not just when `BL` is unset. See example
/// 12's `config()` helper for the identical masking precedent.
fn blocked_account() -> Option<AccountId> {
    Firewall.blocked.get().ok().flatten()
}

#[hooks]
impl Firewall {
    /// Rejects the originating transaction if its sender matches the
    /// configured blocklist account.
    #[hook(0, on = [Payment])]
    fn main() -> i64 {
        let Ok(sender) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"firewall: could not read otxn sender",
                FirewallError::CouldNotReadSender
            )
        };

        let Some(blocked) = blocked_account() else {
            accept!()
        };

        // Avoid `==`, which can compile to an unguarded loop.
        if buf_eq_20(&sender, &blocked) {
            rollback!(b"firewall: blocked account", FirewallError::BlockedAccount);
        }

        accept!()
    }
}
