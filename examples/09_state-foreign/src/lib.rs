#![no_std]

use rshooks::prelude::*;
use rshooks::*;

/// The right-padded key for the target account's flag.
const ENABLED_KEY: StateKey = StateKey(pad!(b"enabled"));

hook_errors! {
    /// Errors returned by the foreign-state hook.
    pub enum StateForeignError {
        /// The target account is not configured.
        AcctNotConfigured = 1,
        /// The target has no flag in this hook's namespace.
        NotConfiguredOnTarget = 2,
        /// Reading the target flag failed.
        ReadFailed = 3,
        /// The target flag is disabled.
        FlagOff = 4,
    }
}

#[hooks]
pub struct StateForeign {
    /// The target account whose flag this hook reads (`ACCT`).
    #[hook_param(name = b"ACCT", required)]
    acct: HookParam<AccountId>,

    /// The target account's flag, read via `state_foreign` under
    /// [`ENABLED_KEY`] in this hook's own namespace.
    #[state(key = &ENABLED_KEY)]
    enabled: State<[u8; 1]>,
}

#[hooks]
impl StateForeign {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let Ok(target) = self.acct.get_required() else {
            rollback!(
                b"state-foreign: ACCT parameter not configured",
                StateForeignError::AcctNotConfigured
            )
        };

        let flag = match self.enabled.get_foreign(None, Some(target.as_ref())) {
            Ok(Some(v)) => v,
            Ok(None) => rollback!(
                b"state-foreign: not configured on target account",
                StateForeignError::NotConfiguredOnTarget
            ),
            Err(_) => rollback!(
                b"state-foreign: state_foreign read failed",
                StateForeignError::ReadFailed
            ),
        };

        if flag[0] == 0 {
            rollback!(
                b"state-foreign: target account's flag is off",
                StateForeignError::FlagOff
            );
        }

        Ok(Accept::from_code(0))
    }
}
