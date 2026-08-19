//! [`HookExit`]/[`ExitType`]: how [`crate::TestEnv::invoke`] reports how a
//! hook entry terminated, and the internal unwind payload that gets it
//! there.

use std::vec::Vec;

/// How a hook entry terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitType {
    /// `accept!` — committed: state writes and validated emissions from
    /// this invocation are kept.
    Accept,
    /// `rollback!` — the invocation's state snapshot is restored and its
    /// emissions discarded.
    Rollback,
    /// A plain `return code` from the entry function. **Provisional**: no
    /// live-node evidence pins the real on-chain commit semantics of a bare
    /// return yet (design §4), so this harness conservatively treats it
    /// like [`ExitType::Rollback`] (state snapshot restored) with
    /// [`HookExit::is_success`] `== false`.
    Return,
}

/// How a [`crate::TestEnv::invoke`]d hook entry terminated.
#[derive(Debug, Clone)]
pub struct HookExit {
    /// How execution terminated.
    pub exit: ExitType,
    /// The application-defined return code (`accept!`/`rollback!`'s second
    /// argument, or the plain `i64` an entry returned).
    pub code: i64,
    /// The message bytes passed to `accept!`/`rollback!`. Empty for
    /// [`ExitType::Return`] (a bare `return` carries no message).
    pub msg: Vec<u8>,
}

impl HookExit {
    /// Whether this exit represents success — `true` only for
    /// [`ExitType::Accept`]. See [`ExitType::Return`]'s doc comment for why
    /// a bare return is not success here.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self.exit, ExitType::Accept)
    }
}

/// Internal unwind payload `panic::panic_any` carries from the mock
/// backend's `accept`/`rollback` up to `TestEnv::invoke`'s `catch_unwind` —
/// see design §2.2. Never exposed to a test author directly; `invoke` always
/// downcasts it into a [`HookExit`] (or, for any other panic payload,
/// resumes the unwind unchanged).
pub(crate) struct HookExitSignal(pub(crate) HookExit);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_accept_is_success() {
        let accept = HookExit {
            exit: ExitType::Accept,
            code: 0,
            msg: Vec::new(),
        };
        let rollback = HookExit {
            exit: ExitType::Rollback,
            code: 1,
            msg: Vec::new(),
        };
        let ret = HookExit {
            exit: ExitType::Return,
            code: 0,
            msg: Vec::new(),
        };
        assert!(accept.is_success());
        assert!(!rollback.is_success());
        assert!(!ret.is_success());
    }
}
