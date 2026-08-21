//! Verifies `account_id!` against the corresponding runtime Hook APIs.

#![no_std]

use rshooks::prelude::*;
use rshooks::*;

/// r-address used by the compile-time and runtime conversions.
const OWNER_RADDR: &[u8; 34] = b"rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";

/// Compile-time decode of [`OWNER_RADDR`].
const OWNER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");

/// Scratch buffer for `util_raddr` output.
static RADDR_BUF: HookStatic<[u8; 34]> = HookStatic::new([0u8; 34]);

hook_errors! {
    /// `account-id-macro` rollback codes.
    pub enum AccountIdMacroError {
        /// `hook_account()` failed (host error).
        HookAccountFailed = 1,
        /// The Hook account does not match [`OWNER`].
        HookAccountMismatch = 2,
        /// `util_accid` failed to convert `OWNER_RADDR` at runtime.
        UtilAccidFailed = 3,
        /// `util_accid`'s runtime conversion of `OWNER_RADDR` disagrees
        /// with `account_id!`'s compile-time decode of the same string.
        UtilAccidMismatch = 4,
        /// `util_raddr` failed to convert `OWNER` back to text at runtime.
        UtilRaddrFailed = 5,
        /// `util_raddr` wrote a different number of bytes than
        /// `OWNER_RADDR`'s length.
        UtilRaddrLenMismatch = 6,
        /// `util_raddr`'s output does not round-trip to `OWNER_RADDR`.
        UtilRaddrMismatch = 7,
        /// [`RADDR_BUF`] was already taken.
        RaddrBufAlreadyTaken = 8,
    }
}

#[hooks]
pub struct AccountIdMacro;

#[hooks]
impl AccountIdMacro {
    /// Accepts only when all conversion paths agree.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let Ok(installed_on) = hook_account_buf() else {
            rollback!(
                b"account-id-macro: hook_account failed",
                AccountIdMacroError::HookAccountFailed
            )
        };
        if !buf_eq_20(&installed_on, &OWNER) {
            rollback!(
                b"account-id-macro: hook_account does not match the account_id! constant",
                AccountIdMacroError::HookAccountMismatch
            );
        }

        let Ok(runtime_accid) = util_accid_buf(OWNER_RADDR) else {
            rollback!(
                b"account-id-macro: util_accid failed",
                AccountIdMacroError::UtilAccidFailed
            )
        };
        if !buf_eq_20(&runtime_accid, &OWNER) {
            rollback!(
                b"account-id-macro: util_accid does not match the account_id! constant",
                AccountIdMacroError::UtilAccidMismatch
            );
        }

        let Some(raddr_buf) = RADDR_BUF.take() else {
            rollback!(
                b"account-id-macro: RADDR_BUF already taken",
                AccountIdMacroError::RaddrBufAlreadyTaken
            );
        };
        let Ok(n) = util_raddr(raddr_buf, OWNER.as_ref()) else {
            rollback!(
                b"account-id-macro: util_raddr failed",
                AccountIdMacroError::UtilRaddrFailed
            )
        };
        if n != OWNER_RADDR.len() {
            rollback!(
                b"account-id-macro: util_raddr wrote an unexpected length",
                AccountIdMacroError::UtilRaddrLenMismatch
            );
        }
        if !buf_eq_34(raddr_buf, OWNER_RADDR) {
            rollback!(
                b"account-id-macro: util_raddr does not round-trip to OWNER_RADDR",
                AccountIdMacroError::UtilRaddrMismatch
            );
        }

        accept!(b"account-id-macro: all three checks passed", 0)
    }
}
