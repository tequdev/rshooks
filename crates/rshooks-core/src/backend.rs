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

/// One argument slot to [`HostBackend::util_keylet`] (its `a`..`f`
/// components). The real Hook API multiplexes two unrelated shapes onto the
/// same six `u32` "slots" depending on `keylet_type`: a bare scalar (a
/// sequence number, a quality component, a ledger-time value, ...) or one
/// half of a `(ptr, len)` pair into the caller's own linear memory. On a
/// native, non-wasm backend a `ptr as u32` value is meaningless (see
/// `.claude/design/TESTENV_DESIGN.md` §1's rejection of `HookHost`/`Guest`
/// for exactly this reason), so `util_keylet` cannot be bridged as six bare
/// `u32`s the way every other Hook API call's pointer arguments are
/// resolved to real slices before the backend ever sees them. `KeyletArg`
/// is that resolution already done: `Value` for a genuine scalar, `Bytes`
/// for whatever a `(ptr, len)` pair would have pointed at. See
/// `crates/rshooks/testenv-call-sites.txt`'s header for exactly which call
/// sites construct which variant.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub enum KeyletArg<'a> {
    /// A bare scalar component (not a pointer) — used verbatim.
    Value(u32),
    /// The resolved bytes a `(ptr, len)` pair would have addressed.
    Bytes(&'a [u8]),
    /// This keylet type does not use this slot.
    Unused,
}

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

    // -- Phase 2 (`.claude/design/TESTENV_PHASE2_DESIGN.md`) --------------
    //
    // Every method below mirrors one `extern.h` Hook API function not
    // covered by Phase 1. As with every method above, the default body is
    // `NOT_IMPLEMENTED` (or its natural `Err`/decoded-`i64` shape), so
    // adding these does not break an existing implementor. P2-A wires the
    // trait + wrapper interception only — no `rshooks-testenv::Backend`
    // override exists yet for any of these (see `host/` in that crate);
    // semantics land in later stages (P2-B..P2-E).

    // -- float_* (pure arithmetic over the XFL `i64` bit pattern; no
    // world state). `float_exponent` has no entry here: xahaud's `XFL`
    // wrapper (`rshooks::xfl::XFL::exponent`) decodes it locally from the
    // bit pattern — there is no `float_exponent` host call to bridge.

    /// Construct a normalized XFL from `exponent`/`mantissa`.
    fn float_set(&self, _exponent: i32, _mantissa: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// `f1 * f2`.
    fn float_multiply(&self, _f1: i64, _f2: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// `f1 * (numerator / denominator)`.
    fn float_mulratio(&self, _f1: i64, _round_up: u32, _numerator: u32, _denominator: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// `-f1`.
    fn float_negate(&self, _f1: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Compares `f1`/`f2` under bitmask `mode`.
    fn float_compare(&self, _f1: i64, _f2: i64, _mode: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// `f1 + f2`.
    fn float_sum(&self, _f1: i64, _f2: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Encodes `amount` as a serialized Amount (native if `currency`/`issuer`
    /// are both `None`, an IOU triple otherwise).
    fn float_sto(
        &self,
        _currency: Option<&[u8]>,
        _issuer: Option<&[u8]>,
        _amount: i64,
        _field_code: u32,
    ) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Decodes a serialized Amount (`sto`) into an XFL bit pattern.
    fn float_sto_set(&self, _sto: &[u8]) -> i64 {
        NOT_IMPLEMENTED
    }

    /// `1 / f1`.
    fn float_invert(&self, _f1: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// `f1 / f2`.
    fn float_divide(&self, _f1: i64, _f2: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// The XFL representation of `1.0`.
    fn float_one(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    /// The mantissa component of `f1`.
    fn float_mantissa(&self, _f1: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Whether `f1` is negative.
    fn float_sign(&self, _f1: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Converts `f1` to an integer, keeping `decimal_places` fractional
    /// digits; `absolute` requests the magnitude instead of erroring on a
    /// negative result.
    fn float_int(&self, _f1: i64, _decimal_places: u32, _absolute: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// `log10(f1)`.
    fn float_log(&self, _f1: i64) -> i64 {
        NOT_IMPLEMENTED
    }

    /// `f1 ^ (1/n)`.
    fn float_root(&self, _f1: i64, _n: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    // -- slot_* (numbered-register navigation; world state in Phase 2).

    /// Serializes the object in `slot_no`.
    fn slot(&self, _slot_no: u32) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Frees `slot_no`.
    fn slot_clear(&self, _slot_no: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// The number of array elements held in `slot_no`.
    fn slot_count(&self, _slot_no: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// The amount held in `slot_no`, as an XFL bit pattern.
    fn slot_float(&self, _slot_no: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Loads the object identified by keylet/hash `data` into `slot_into`
    /// (`0` auto-assigns). Returns the assigned slot number.
    fn slot_set(&self, _data: &[u8], _slot_into: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// The serialized size, in bytes, of the object in `slot_no`.
    fn slot_size(&self, _slot_no: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Extracts array element `array_id` of `parent_slot` into `new_slot`
    /// (`0` auto-assigns). Returns the assigned slot number.
    fn slot_subarray(&self, _parent_slot: u32, _array_id: u32, _new_slot: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Extracts field `field_id` of `parent_slot` into `new_slot` (`0`
    /// auto-assigns). Returns the assigned slot number.
    fn slot_subfield(&self, _parent_slot: u32, _field_id: u32, _new_slot: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// The type of the object in `slot_no` (`flags = 0`: field code;
    /// `flags = 1`: is-native-amount query).
    fn slot_type(&self, _slot_no: u32, _flags: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Loads the originating transaction into `slot_into` (`0`
    /// auto-assigns). Returns the assigned slot number.
    fn otxn_slot(&self, _slot_into: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Loads the current transaction's metadata into `slot_into` (`0`
    /// auto-assigns). Returns the assigned slot number.
    fn meta_slot(&self, _slot_into: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Loads an XPOP's transaction and metadata into the given slots.
    fn xpop_slot(&self, _slot_no_tx: u32, _slot_no_meta: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    // -- sto_* (pure byte-level STObject surgery on caller buffers; no
    // world state).

    /// Locates field `field_id` within `sto`; returns the raw packed
    /// `(offset, length)` value (see `rshooks::api::sto`'s module doc
    /// comment for the packing).
    fn sto_subfield(&self, _sto: &[u8], _field_id: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Locates element `index` within STO array `array`; returns the raw
    /// packed `(offset, length)` value.
    fn sto_subarray(&self, _array: &[u8], _index: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Validates the integrity of `sto`.
    fn sto_validate(&self, _sto: &[u8]) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Inserts or replaces field `field_id` (encoded as `field`) into
    /// `source`.
    fn sto_emplace(&self, _source: &[u8], _field: &[u8], _field_id: u32) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Removes field `field_id` from `source`.
    fn sto_erase(&self, _source: &[u8], _field_id: u32) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    // -- util_* / ledger_keylet.

    /// SHA-512-Half of `data`.
    fn util_sha512h(&self, _data: &[u8]) -> Result<[u8; 32], i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Converts a base58 r-address to its AccountID form.
    fn util_accid(&self, _r_address: &[u8]) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Converts an AccountID to its base58 r-address text form.
    fn util_raddr(&self, _accid: &[u8]) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Verifies that `signature` over `data` was produced by `public_key`.
    fn util_verify(&self, _data: &[u8], _signature: &[u8], _public_key: &[u8]) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Computes a keylet of `keylet_type` from six resolved argument slots
    /// — see [`KeyletArg`]'s doc comment for why these are already resolved
    /// bytes/values rather than raw `u32`s.
    fn util_keylet(&self, _keylet_type: u32, _args: [KeyletArg<'_>; 6]) -> Result<[u8; 34], i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Computes a keylet from a low/high bound pair (as used by range-style
    /// ledger entries), searching the seeded ledger objects for the first
    /// keylet in `(low, high]`.
    fn ledger_keylet(&self, _low: &[u8], _high: &[u8]) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    // -- control leftovers / etxn::prepare / trace_float.

    /// Requests that this hook be called again after the originating
    /// transaction completes (weak execution).
    fn hook_again(&self) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Instructs the enclosing hook chain to skip the hook identified by
    /// `hash` on subsequent invocations, per `flags`.
    fn hook_skip(&self, _hash: &[u8], _flags: u32) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Sets a parameter named `name` to `value` on the hook identified by
    /// `hook_hash`.
    fn hook_param_set(&self, _value: &[u8], _name: &[u8], _hook_hash: &[u8]) -> i64 {
        NOT_IMPLEMENTED
    }

    /// Completes an emission `template` into a ready-to-emit transaction
    /// blob.
    fn prepare(&self, _template: &[u8]) -> Result<Vec<u8>, i64> {
        Err(NOT_IMPLEMENTED)
    }

    /// Emits a trace message (`msg`) followed by an XFL `value`.
    fn trace_float(&self, _msg: &[u8], _value: i64) -> i64 {
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
