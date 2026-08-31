//! Information about the executing hook itself: its account, hash, and
//! parameters.

use crate::convert::{FixedRead, TypedParamName};
use crate::error::{Result, res};
use crate::types::{AccountId, Hash};

/// The AccountID this hook is installed on, written into `out`. Returns the
/// number of bytes written. [`hook_account_buf`] is the fixed-size
/// convenience twin.
#[inline(always)]
pub fn hook_account<B: AsMut<[u8]> + ?Sized>(out: &mut B) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.hook_account()) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe { rshooks_core::hook_account(out.as_mut_ptr() as u32, out.len() as u32) })
        .map(|v| v as usize)
}

/// The AccountID this hook is installed on.
#[inline(always)]
pub fn hook_account_buf() -> Result<AccountId> {
    let mut buf = AccountId::default();
    let _ = hook_account(buf.as_mut())?;
    Ok(buf)
}

/// The hash of the hook definition at chain position `hook_no`, written into
/// `out`. Returns the number of bytes written. [`hook_hash_buf`] is the
/// fixed-size convenience twin.
#[inline(always)]
pub fn hook_hash<B: AsMut<[u8]> + ?Sized>(out: &mut B, hook_no: i32) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.hook_hash(hook_no)) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe { rshooks_core::hook_hash(out.as_mut_ptr() as u32, out.len() as u32, hook_no) })
        .map(|v| v as usize)
}

/// The hash of the hook definition at chain position `hook_no` on this
/// hook's account (negative indices address relative to the current hook,
/// per Hook API convention).
#[inline(always)]
pub fn hook_hash_buf(hook_no: i32) -> Result<Hash> {
    let mut buf = Hash::default();
    let _ = hook_hash(buf.as_mut(), hook_no)?;
    Ok(buf)
}

/// Read this hook's own parameter `name` into `out`. Returns the number of
/// bytes written.
#[inline(always)]
pub fn hook_param<B: AsMut<[u8]> + ?Sized>(out: &mut B, name: &[u8]) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.hook_param(name)) {
        return crate::testenv_bridge::write_bytes_truncate(out, r);
    }
    res(unsafe {
        rshooks_core::hook_param(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            name.as_ptr() as u32,
            name.len() as u32,
        )
    })
    .map(|v| v as usize)
}

/// Read this hook's own parameter `name`, requiring it to be exactly `T`'s
/// length — any [`crate::convert::FixedRead`] type. A parameter longer than
/// `T` fails as [`crate::error::HookError::TooSmall`] from the underlying
/// host call; a parameter shorter is caught by `T::read_exact` itself and
/// mapped to the same variant. No loop, no panic.
///
/// `T` is inferred from context, not a turbofish.
///
/// # Examples
///
/// ```
/// use rshooks::api::hook_ctx::hook_param_exact;
/// use rshooks::error::{HookError, Result};
///
/// let value: Result<[u8; 4]> = hook_param_exact(b"x");
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn hook_param_exact<T: FixedRead>(name: &[u8]) -> Result<T> {
    T::read_exact(|buf| hook_param(buf, name))
}

/// Read this hook's own parameter, named by `name` itself, so the name and
/// its paired value type can't drift apart into naming a *different*
/// parameter than intended. `N::Value` (the return type) is inferred from
/// `name`'s own type — no turbofish.
///
/// Costs nothing beyond [`hook_param_exact`] for a plain byte-string name
/// (a hand-written [`crate::convert::TypedParamName`] overriding
/// `with_name_bytes` to hand back a `'static` literal); a composite,
/// struct-shaped name costs a small runtime encode instead.
///
/// # Examples
///
/// ```
/// use rshooks::api::hook_ctx::hook_param_typed;
/// use rshooks::convert::TypedParamName;
/// use rshooks::error::{HookError, Result};
///
/// struct Threshold([u8; 8]);
///
/// impl rshooks::convert::FixedRead for Threshold {
///     fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
///         <[u8; 8]>::read_exact(read).map(Threshold)
///     }
/// }
///
/// struct MinName;
///
/// impl rshooks::convert::ToBytes for MinName {
///     const MAX_LEN: usize = 3;
///     fn write(&self, buf: &mut [u8]) -> usize {
///         rshooks::convert::ToBytes::write(b"MIN", buf)
///     }
/// }
///
/// impl TypedParamName for MinName {
///     type Value = Threshold;
/// }
///
/// let value = hook_param_typed(&MinName);
/// assert_eq!(value.err(), Some(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn hook_param_typed<N: TypedParamName>(name: &N) -> Result<N::Value> {
    name.with_name_bytes(hook_param_exact::<N::Value>)
}

/// Calls the host `hook_param` function directly and returns its
/// **undecoded** `i64` result — no [`res`] applied, so no
/// [`crate::error::HookError`] is ever constructed here. `pub(crate)` fast
/// path for [`hook_param_opt`], which must compare the raw code against
/// [`rshooks_core::DOESNT_EXIST`] before deciding whether to decode at all.
/// Duplicates [`hook_param`]'s own call rather than routing through it,
/// since doing so changes the compiled block nesting of unrelated call
/// sites elsewhere in a hook.
#[inline(always)]
pub(crate) fn hook_param_raw_code(buf: &mut [u8], name: &[u8]) -> i64 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.hook_param(name)) {
        return crate::testenv_bridge::write_bytes_truncate_code(buf, r);
    }
    unsafe {
        rshooks_core::hook_param(
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            name.as_ptr() as u32,
            name.len() as u32,
        )
    }
}

/// Read this hook's own parameter `name`, distinguishing "parameter is
/// absent" from every other outcome — the absence-aware counterpart to
/// [`hook_param_exact`], used by [`crate::decl`]'s `HookParam`/`HookParamAt`
/// accessors.
///
/// `Ok(None)` means no parameter named `name` was supplied. A parameter
/// that *is* present but fails to decode as `T` (too short, wrong shape)
/// still comes back as `Err` — this never folds a genuine decode failure
/// into `None`, the same "absence vs. error, never confused" contract
/// [`mod@crate::state`]'s `state_get` documents for hook state.
///
/// # Examples
///
/// ```
/// use rshooks::api::hook_ctx::hook_param_opt;
/// use rshooks::error::{HookError, Result};
///
/// // Host stub: every Hook API call returns `NotImplemented` on a host
/// // build, so this only proves the call chain compiles and runs (it is
/// // not the `DOESNT_EXIST` sentinel `hook_param_opt` maps to `Ok(None)`).
/// let value: Result<Option<[u8; 4]>> = hook_param_opt(b"x");
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn hook_param_opt<T: FixedRead>(name: &[u8]) -> Result<Option<T>> {
    let mut absent = false;
    let r = T::read_exact(|buf| {
        let code = hook_param_raw_code(buf, name);
        if code == rshooks_core::DOESNT_EXIST {
            absent = true;
            return Ok(0);
        }
        res(code).map(|v| v as usize)
    });
    if absent {
        return Ok(None);
    }
    r.map(Some)
}

/// Set a parameter named `name` to `value` on the hook identified by
/// `hook_hash`. Returns the number of bytes written.
#[inline(always)]
pub fn hook_param_set(value: &[u8], name: &[u8], hook_hash: &[u8]) -> Result<usize> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) =
        rshooks_core::backend::with_backend(|b| b.hook_param_set(value, name, hook_hash))
    {
        return res(v).map(|v| v as usize);
    }
    res(unsafe {
        rshooks_core::hook_param_set(
            value.as_ptr() as u32,
            value.len() as u32,
            name.as_ptr() as u32,
            name.len() as u32,
            hook_hash.as_ptr() as u32,
            hook_hash.len() as u32,
        )
    })
    .map(|v| v as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        assert_eq!(hook_account_buf(), Err(HookError::NotImplemented));
        assert_eq!(hook_hash_buf(0), Err(HookError::NotImplemented));
        let mut out = [0u8; 32];
        assert_eq!(hook_account(&mut out), Err(HookError::NotImplemented));
        assert_eq!(hook_hash(&mut out, 0), Err(HookError::NotImplemented));
        assert_eq!(hook_param(&mut out, b"x"), Err(HookError::NotImplemented));
        assert_eq!(
            hook_param_set(b"v", b"x", &[0u8; 32]),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            hook_param_exact::<[u8; 4]>(b"x"),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            hook_param_opt::<[u8; 4]>(b"x"),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            hook_param_typed(&TestParamName),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            hook_param_typed(&TestKeyParamName { tag: 1 }),
            Err(HookError::NotImplemented)
        );
    }

    // Simple/static form: `Name` is a marker type, `with_name_bytes`
    // overridden to skip the default (encoding) body entirely.
    #[derive(Debug, PartialEq)]
    struct TestParam([u8; 4]);

    impl FixedRead for TestParam {
        fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
            <[u8; 4]>::read_exact(read).map(TestParam)
        }
    }

    struct TestParamName;

    impl crate::convert::ToBytes for TestParamName {
        const MAX_LEN: usize = 1;

        fn write(&self, buf: &mut [u8]) -> usize {
            crate::convert::ToBytes::write(b"x", buf)
        }
    }

    impl crate::convert::TypedParamName for TestParamName {
        type Value = TestParam;

        fn with_name_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
            f(b"x")
        }
    }

    // Composite form: relies on `TypedParamName::with_name_bytes`'s
    // default body (the genuine runtime encode, exercised here with a
    // trivial one-field name).
    #[derive(Debug, PartialEq)]
    struct TestKeyParam([u8; 4]);

    impl FixedRead for TestKeyParam {
        fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
            <[u8; 4]>::read_exact(read).map(TestKeyParam)
        }
    }

    struct TestKeyParamName {
        tag: u8,
    }

    impl crate::convert::ToBytes for TestKeyParamName {
        const MAX_LEN: usize = 1;

        fn write(&self, buf: &mut [u8]) -> usize {
            crate::convert::ToBytes::write(&self.tag, buf)
        }
    }

    impl crate::convert::TypedParamName for TestKeyParamName {
        type Value = TestKeyParam;
    }
}

/// Proves `hook_param`'s testenv interception truncates rather than
/// returning `TooSmall` on an undersized destination — the documented
/// xahaud `hook_param` asymmetry (see [`crate::testenv_bridge::write_bytes_truncate`]'s
/// doc comment), unlike every other byte-returning call in this crate.
#[cfg(all(test, feature = "testenv"))]
mod testenv_tests {
    #![allow(clippy::unwrap_used, clippy::panic)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    extern crate std;

    use std::rc::Rc;
    use std::vec::Vec;

    use super::*;
    use rshooks_core::backend::{HostBackend, install};

    /// Answers `hook_param` with a fixed byte string regardless of `name`;
    /// `accept`/`rollback` are unused by these tests and simply panic if
    /// ever reached.
    struct FixedBytesBackend(&'static [u8]);

    impl HostBackend for FixedBytesBackend {
        fn hook_param(&self, _name: &[u8]) -> core::result::Result<Vec<u8>, i64> {
            Ok(self.0.to_vec())
        }

        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("FixedBytesBackend::accept unexpectedly called")
        }

        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("FixedBytesBackend::rollback unexpectedly called")
        }
    }

    #[test]
    fn hook_param_undersized_destination_truncates_never_too_small() {
        let _guard = install(Rc::new(FixedBytesBackend(&[1, 2, 3, 4, 5, 6, 7, 8, 9])));
        let mut out = [0u8; 4]; // shorter than the backend's 9-byte value
        assert_eq!(hook_param(&mut out, b"x"), Ok(4));
        assert_eq!(out, [1, 2, 3, 4]);
    }

    #[test]
    fn hook_param_raw_code_undersized_destination_truncates() {
        let _guard = install(Rc::new(FixedBytesBackend(&[1, 2, 3, 4, 5, 6, 7, 8, 9])));
        let mut out = [0u8; 4]; // shorter than the backend's 9-byte value
        assert_eq!(hook_param_raw_code(&mut out, b"x"), 4);
        assert_eq!(out, [1, 2, 3, 4]);
    }
}
