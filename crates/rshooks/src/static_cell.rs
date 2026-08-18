//! [`HookStatic`]: safe, take-once access to `static` hook buffers.
//!
//! Constant templates and large zero-initialized buffers should live in
//! `static`s rather than stack locals (data segment / BSS instead of
//! runtime store chains or a compiler-generated memset — see
//! `docs/DESIGN.md` §6.3, "static-buffer idiom"). A bare `static mut`
//! makes that possible but forces every hook to repeat an unsafe,
//! clippy-fighting access incantation and offers no protection against
//! creating two aliasing `&mut` to the same buffer.
//!
//! `HookStatic<T>` wraps the buffer with a take-once flag: [`take`] hands
//! out the one-and-only `&'static mut T` and every later call returns
//! `None`. The single `unsafe` lives here, justified once; call sites are
//! plain safe Rust.
//!
//! [`take`]: HookStatic::take

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

/// Process-global synchronization for the testenv claim path — see
/// [`HookStatic::take`]'s `testenv`-gated body for the invariant this
/// protects. Kept out of [`HookStatic`] itself: the cell carries no
/// cfg-dependent field on any target.
#[cfg(all(not(target_arch = "wasm32"), feature = "testenv"))]
mod testenv_claim {
    extern crate std;

    /// A single process-wide lock serializing every `HookStatic::take`
    /// call (plain or testenv) against every other one, on any cell. Held
    /// only for the duration of one claim decision — short-lived by
    /// construction, since a claim decision does no I/O and no recursive
    /// `take` call.
    pub(super) static CLAIM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}

/// A `static`-friendly cell handing out exclusive access to its contents
/// exactly once per instance lifetime.
///
/// Hooks run single-threaded and every hook invocation executes in a
/// freshly instantiated wasm instance, so "once per instance lifetime"
/// means "once per hook execution".
///
/// # Examples
///
/// ```
/// use rshooks::static_cell::HookStatic;
///
/// static BUF: HookStatic<[u8; 4]> = HookStatic::new([1, 2, 3, 4]);
///
/// let buf = BUF.take().expect("first take");
/// buf[0] = 9;
/// assert!(BUF.take().is_none()); // exclusive: handed out only once
/// ```
pub struct HookStatic<T: Clone> {
    // Atomic (not Cell) so `take` is race-free even on multi-threaded hosts
    // (tests, rust-analyzer); on single-threaded wasm it lowers to the same
    // plain load/store.
    taken: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: on every target, the only mutation reachable through `&self` via
// the plain (non-testenv, or no-backend-installed) path is the atomic
// flag; that path hands out the interior `&mut` exactly once (the swap has
// exactly one winner, on any number of threads), so shared references
// never expose aliased mutation through it. Under
// `testenv`, a second claim path exists (`HookStatic::take`'s
// backend-installed body) that never hands out `&mut` to the original
// storage at all — it only ever reads `self.value` to produce an
// independent clone, and only while `testenv_claim::CLAIM_LOCK` excludes
// the plain path's own swap-then-borrow — so the two paths never observe
// or produce aliased mutation of one another.
unsafe impl<T: Send + Clone> Sync for HookStatic<T> {}

impl<T: Clone> HookStatic<T> {
    /// Creates a cell. `const`, so it can initialize a `static`: the value
    /// bytes land in a wasm data segment (or BSS when all-zero).
    pub const fn new(value: T) -> Self {
        Self {
            taken: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Returns the exclusive `&'static mut` to the contents on the first
    /// call, and `None` on every call after that.
    ///
    /// The take-once flag is what makes this safe: two aliasing `&mut` to
    /// the same static can never be produced. There is deliberately no
    /// "give back" operation — a hook runs once and exits.
    ///
    /// Identical on every target except native `testenv` builds with a
    /// backend installed on the calling thread — see the `testenv`-gated
    /// overload below for that path.
    #[cfg(any(target_arch = "wasm32", not(feature = "testenv")))]
    #[allow(clippy::mut_from_ref)] // uniqueness enforced by the take-once flag, not by the type system
    #[inline(always)]
    pub fn take(&'static self) -> Option<&'static mut T> {
        if self.taken.swap(true, Ordering::AcqRel) {
            None
        } else {
            // SAFETY: exactly one caller ever wins the swap above, so the
            // returned reference is unique.
            Some(unsafe { &mut *self.value.get() })
        }
    }

    /// Native `testenv` counterpart to the plain [`take`](Self::take):
    /// with a backend installed on the calling thread, this never touches
    /// the take-once flag and never hands out the original storage at
    /// all. Instead it asks the backend
    /// (`HostBackend::static_take_allowed`, keyed by this cell's own
    /// address) whether a fresh claim is allowed this invocation: `false`
    /// is `None`; `true` clones the pristine storage and leaks the clone,
    /// so every invocation gets its own independent `&'static mut T` with
    /// no static-side epoch or field. With no backend installed on the
    /// thread, falls through to the plain take-once behavior unchanged.
    ///
    /// `testenv_claim::CLAIM_LOCK` is held for the whole decision on both
    /// branches: the plain branch's `taken` swap and this branch's storage
    /// read-then-clone can therefore never interleave, so the clone read
    /// is never racing a live `&mut` to the same storage, and the two
    /// claim paths degrade to `None` rather than aliasing when mixed
    /// within one process.
    #[cfg(all(not(target_arch = "wasm32"), feature = "testenv"))]
    #[allow(clippy::mut_from_ref)]
    #[inline(always)]
    pub fn take(&'static self) -> Option<&'static mut T> {
        extern crate std;
        use std::boxed::Box;

        let _guard = testenv_claim::CLAIM_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let addr = self as *const Self as usize;
        if let Some(allowed) = rshooks_core::backend::with_backend(|b| b.static_take_allowed(addr))
        {
            if self.taken.load(Ordering::Acquire) || !allowed {
                return None;
            }
            // SAFETY: `_guard` excludes any concurrent plain `take()` swap
            // on this cell for the duration of this read, and this branch
            // never constructs a `&mut` to `self.value` — only a `&T` for
            // the immediate `.clone()` call.
            let cloned = unsafe { (*self.value.get()).clone() };
            return Some(Box::leak(Box::new(cloned)));
        }
        if self.taken.swap(true, Ordering::AcqRel) {
            None
        } else {
            // SAFETY: as the plain path above — `_guard` additionally
            // rules out a concurrent testenv clone read racing this swap.
            Some(unsafe { &mut *self.value.get() })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    static CELL: HookStatic<[u8; 3]> = HookStatic::new([7, 8, 9]);

    #[test]
    fn take_yields_value_once_then_none() {
        let first = CELL.take();
        let second = CELL.take();
        // Exactly one of the two calls got the buffer (test order within
        // this fn is deterministic: the first).
        let buf = first.expect("first take yields the buffer");
        assert_eq!(buf, &mut [7, 8, 9]);
        buf[0] = 42;
        assert!(second.is_none());
        assert!(CELL.take().is_none());
    }

    #[test]
    fn take_is_exclusive_across_threads() {
        extern crate std;
        use std::{thread, vec::Vec};

        static RACE: HookStatic<u32> = HookStatic::new(0);

        let handles: Vec<_> = (0..8)
            .map(|_| thread::spawn(|| RACE.take().is_some()))
            .collect();
        let winners = handles
            .into_iter()
            .map(|h| h.join())
            .filter(|r| matches!(r, Ok(true)))
            .count();
        assert_eq!(winners, 1, "exactly one thread may win the take");
    }
}
