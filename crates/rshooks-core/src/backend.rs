//! Native-only mock host seam for off-chain unit tests.
//!
//! [`HostBackend`] is the interception point the `rshooks` wrapper layer
//! consults (native, `testenv`-only) before making a raw Hook API call: an
//! installed backend answers the call instead of falling through to the
//! `NOT_IMPLEMENTED` host stubs. This module has no presence at all on
//! `target_arch = "wasm32"` or in builds that do not enable the `testenv`
//! feature.
//!
//! `#[doc(hidden)]`: this is an unstable internal contract between
//! `rshooks`/`rshooks-core` and `rshooks-testenv`, not a stable public API.
//! [`HostBackend`] is intentionally left unsealed so a downstream crate
//! (`rshooks-testenv`) can implement it.

extern crate std;

use std::cell::RefCell;
use std::rc::Rc;
use std::thread_local;
use std::vec::Vec;

use crate::error::NOT_IMPLEMENTED;

/// The native mock-host contract. Every method mirrors a semantic Hook API
/// operation (not a raw FFI signature): callers pass owned/borrowed Rust
/// values, not `u32` pointers.
///
/// Every non-control method has a default body returning `NOT_IMPLEMENTED`
/// (or the API's natural `Err` variant of it), so adding a method here in a
/// later phase does not break existing implementors. `accept`/`rollback` are
/// `-> !` and have no default: an implementor must define how execution
/// terminates.
///
/// No `Send`/`Sync` bound: the registry below is thread-local, and a
/// backend only ever runs on the thread that installed it.
#[doc(hidden)]
pub trait HostBackend {
    fn state(&self, _key: &[u8]) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn state_set(&self, _key: &[u8], _data: &[u8]) -> Result<i64, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn state_foreign(
        &self,
        _key: &[u8],
        _ns: Option<&[u8; 32]>,
        _acc: Option<&[u8; 20]>,
    ) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn state_foreign_set(
        &self,
        _key: &[u8],
        _data: &[u8],
        _ns: Option<&[u8; 32]>,
        _acc: Option<&[u8; 20]>,
    ) -> Result<i64, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn otxn_field(&self, _field_id: u32) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn otxn_type(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn otxn_id(&self, _flags: u32) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn otxn_param(&self, _name: &[u8]) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn otxn_burden(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn otxn_generation(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn hook_param(&self, _name: &[u8]) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn hook_account(&self) -> Result<[u8; 20], i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn hook_hash(&self, _hook_no: i32) -> Result<[u8; 32], i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn hook_pos(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn ledger_seq(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn ledger_last_time(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn ledger_last_hash(&self) -> Result<[u8; 32], i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn ledger_nonce(&self) -> Result<[u8; 32], i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn fee_base(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn etxn_reserve(&self, _count: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    fn etxn_fee_base(&self, _tx_blob: &[u8]) -> i64 {
        NOT_IMPLEMENTED
    }

    fn etxn_details(&self) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn etxn_burden(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn etxn_generation(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    fn etxn_nonce(&self) -> Result<[u8; 32], i64> {
        Err(NOT_IMPLEMENTED)
    }

    fn emit(&self, _tx_blob: &[u8]) -> Result<[u8; 32], i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Required: an implementor must define how hook execution terminates.
    fn accept(&self, msg: &[u8], code: i64) -> !;

    /// Required: an implementor must define how hook execution terminates.
    fn rollback(&self, msg: &[u8], code: i64) -> !;

    fn trace(&self, _msg: &[u8], _data: &[u8], _as_hex: bool) -> i64 {
        NOT_IMPLEMENTED
    }

    fn trace_num(&self, _msg: &[u8], _num: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Consulted by `HookStatic::take()` under `testenv` to decide whether a
    /// static cell at `cell_addr` may hand out a fresh per-invocation clone.
    /// Default `true`: a backend that does not model statics imposes no
    /// restriction.
    fn static_take_allowed(&self, _cell_addr: usize) -> bool {
        true
    }
}

thread_local! {
    static BACKEND: RefCell<Option<Rc<dyn HostBackend>>> = const { RefCell::new(None) };
}

/// RAII handle returned by [`install`]. Restores the thread's backend slot
/// to empty when dropped (including while unwinding), so a forgotten guard
/// cannot leave a stale backend installed for a later test on the same
/// thread.
#[doc(hidden)]
#[must_use = "the backend is uninstalled when this guard is dropped"]
pub struct BackendGuard {
    _private: (),
}

impl Drop for BackendGuard {
    fn drop(&mut self) {
        BACKEND.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

/// Installs `backend` as the current thread's mock host, returning a guard
/// that uninstalls it on drop.
///
/// # Panics
///
/// Panics if a backend is already installed on this thread — this catches
/// both reentrancy (an `invoke` triggering another `invoke` on the same
/// thread) and a forgotten/leaked previous guard.
#[doc(hidden)]
pub fn install(backend: Rc<dyn HostBackend>) -> BackendGuard {
    BACKEND.with(|cell| {
        let mut slot = cell.borrow_mut();
        assert!(
            slot.is_none(),
            "rshooks_core::backend::install: a backend is already installed on this thread \
             (reentrant invoke, or a previous BackendGuard was not dropped)"
        );
        *slot = Some(backend);
    });
    BackendGuard { _private: () }
}

/// Runs `f` with the current thread's installed backend, if any.
///
/// Returns `None` when no backend is installed on this thread.
#[doc(hidden)]
pub fn with_backend<R>(f: impl FnOnce(&dyn HostBackend) -> R) -> Option<R> {
    BACKEND.with(|cell| cell.borrow().as_ref().map(|b| f(&**b)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    struct StubBackend;

    impl HostBackend for StubBackend {
        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("StubBackend::accept called")
        }

        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("StubBackend::rollback called")
        }
    }

    struct MarkerBackend(&'static str);

    impl HostBackend for MarkerBackend {
        fn otxn_type(&self) -> i64 {
            // Reuse an i64-returning method as a cheap identity marker.
            self.0.len() as i64
        }

        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("MarkerBackend::accept called")
        }

        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("MarkerBackend::rollback called")
        }
    }

    #[test]
    fn no_backend_installed_returns_none() {
        assert!(with_backend(|b| b.otxn_type()).is_none());
    }

    #[test]
    fn install_and_with_backend_roundtrip() {
        let guard = install(Rc::new(StubBackend));
        let result = with_backend(|b| b.state(b"missing"));
        assert_eq!(result, Some(Err(NOT_IMPLEMENTED)));
        drop(guard);
        assert!(with_backend(|b| b.otxn_type()).is_none());
    }

    #[test]
    fn guard_drop_restores_empty_slot_and_allows_nested_install_of_a_different_backend() {
        {
            let _guard = install(Rc::new(MarkerBackend("first")));
            assert_eq!(with_backend(|b| b.otxn_type()), Some(5));
        }
        // First guard dropped: slot must be empty again.
        assert!(with_backend(|b| b.otxn_type()).is_none());

        // A different backend can now be installed without panicking.
        {
            let _guard = install(Rc::new(MarkerBackend("second-backend")));
            assert_eq!(with_backend(|b| b.otxn_type()), Some(14));
        }
        assert!(with_backend(|b| b.otxn_type()).is_none());
    }

    #[test]
    #[should_panic(expected = "a backend is already installed on this thread")]
    fn occupied_slot_is_rejected() {
        let _first = install(Rc::new(StubBackend));
        let _second = install(Rc::new(StubBackend));
    }

    #[test]
    fn default_body_returns_not_implemented() {
        let guard = install(Rc::new(StubBackend));
        assert_eq!(with_backend(|b| b.state(b"k")), Some(Err(NOT_IMPLEMENTED)));
        assert_eq!(
            with_backend(|b| b.state_set(b"k", b"v")),
            Some(Err(NOT_IMPLEMENTED))
        );
        assert_eq!(with_backend(|b| b.otxn_burden()), Some(NOT_IMPLEMENTED));
        assert_eq!(
            with_backend(|b| b.trace(b"m", b"d", false)),
            Some(NOT_IMPLEMENTED)
        );
        assert!(with_backend(|b| b.static_take_allowed(0)).unwrap());
        drop(guard);
    }
}
