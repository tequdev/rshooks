//! Hook execution flow control: `accept`, `rollback`, `hook_again`,
//! `hook_skip`, `hook_pos`.

use crate::error::{Result, res};

/// Terminate hook execution successfully, optionally carrying a UTF-8-ish
/// message and an application-defined return code.
///
/// On the real wasm host this call never returns (`accept` unwinds hook
/// execution). On host builds the stub returns normally, so this falls back
/// to an infinite loop purely to honor the `-> !` signature without
/// invoking real UB — reachable only in host tests/doctests.
///
/// # Examples
///
/// ```no_run
/// use rshooks::api::control::accept;
///
/// accept(b"done", 0);
/// ```
#[inline(always)]
pub fn accept(msg: &[u8], code: i64) -> ! {
    // A backend's `accept` is `-> !`: if installed, this diverges; with no
    // backend, `with_backend` returns `None` and falls through unchanged.
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    {
        rshooks_core::backend::with_backend(|b| {
            b.accept(msg, code);
        });
    }
    unsafe {
        let _ = rshooks_core::accept(msg.as_ptr() as u32, msg.len() as u32, code);
    }
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::unreachable();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Host-only fallback for `-> !`; never reached in a real wasm hook.
        #[allow(clippy::empty_loop)]
        loop {}
    }
}

/// Terminate hook execution with a failure, rolling back all state changes
/// made by this hook invocation. See [`accept`] for the `-> !` rationale.
#[inline(always)]
pub fn rollback(msg: &[u8], code: i64) -> ! {
    // See `accept`'s matching comment.
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    {
        rshooks_core::backend::with_backend(|b| {
            b.rollback(msg, code);
        });
    }
    unsafe {
        let _ = rshooks_core::rollback(msg.as_ptr() as u32, msg.len() as u32, code);
    }
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::unreachable();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // See the matching arm in `accept` above for why this is here.
        #[allow(clippy::empty_loop)]
        loop {}
    }
}

/// Request that the hook be called again after the originating transaction
/// completes (weak execution). Returns the raw success payload.
///
/// # Examples
///
/// ```
/// use rshooks::api::control::hook_again;
/// use rshooks::error::HookError;
///
/// assert_eq!(hook_again(), Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn hook_again() -> Result<i64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.hook_again()) {
        return res(v);
    }
    res(unsafe { rshooks_core::hook_again() })
}

/// Instruct the enclosing hook chain to skip a specific hook (by hash) on
/// subsequent invocations, according to `flags`.
#[inline(always)]
pub fn hook_skip(hash: &[u8], flags: u32) -> Result<i64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.hook_skip(hash, flags)) {
        return res(v);
    }
    res(unsafe { rshooks_core::hook_skip(hash.as_ptr() as u32, hash.len() as u32, flags) })
}

/// Get the hook's position (index) in the hook chain of the current account.
///
/// Never returns a Hook API error code, so it is exposed as a plain `u8`
/// (a hook chain holds at most 10 hooks) rather than a `Result`.
#[inline(always)]
pub fn hook_pos() -> u8 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.hook_pos()) {
        return v as u8;
    }
    unsafe { rshooks_core::hook_pos() as u8 }
}

/// Proves `accept`/`rollback` consult an installed backend *before* the
/// raw call and diverge through it — the raw call's own host-only
/// infinite-loop fallback is never reached once a backend is installed.
#[cfg(all(test, feature = "testenv"))]
mod testenv_tests {
    #![allow(clippy::panic)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    extern crate std;

    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    use super::*;
    use rshooks_core::backend::{HostBackend, install};

    /// `accept`/`rollback` both panic — standing in for the real testenv
    /// backend's `panic::panic_any(HookExitSignal(..))` unwind (design
    /// §2.2): this test only needs to prove the call chain diverges via
    /// *some* unwind, not decode the exit payload.
    struct DivergingBackend;

    impl HostBackend for DivergingBackend {
        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("DivergingBackend::accept")
        }

        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("DivergingBackend::rollback")
        }
    }

    #[test]
    fn accept_diverges_through_an_installed_backend() {
        let _guard = install(Rc::new(DivergingBackend));
        let result = catch_unwind(AssertUnwindSafe(|| accept(b"done", 0)));
        assert!(
            result.is_err(),
            "accept should have unwound via the backend"
        );
    }

    #[test]
    fn rollback_diverges_through_an_installed_backend() {
        let _guard = install(Rc::new(DivergingBackend));
        let result = catch_unwind(AssertUnwindSafe(|| rollback(b"nope", 1)));
        assert!(
            result.is_err(),
            "rollback should have unwound via the backend"
        );
    }
}
