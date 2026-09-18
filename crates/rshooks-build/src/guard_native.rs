//! Safe Rust wrapper around the vendored upstream guard checker
//! (`docs/DESIGN.md` §6.5). This is the *authoritative* accept/reject
//! verdict: [`validate_guards_native`] calls into `cpp/guard_shim.cpp`, which
//! in turn calls xahaud's own `validateGuards()`, compiled in unmodified
//! from `vendor/xahaud/`.
//!
//! The Rust validator (`crate::validator`) remains useful for pre-transform
//! diagnostics with precise function/offset locations and for rules the
//! checker doesn't cover (the size gate), but this module's verdict wins on
//! any disagreement.

use std::os::raw::c_char;

/// Log buffer size passed to the native checker. Large enough that a real
/// guard-checker log (which is normally a handful of lines) is never
/// truncated; [`NativeGuardError::truncated`] reports it if one somehow is.
const LOG_BUF_CAPACITY: usize = 64 * 1024;

unsafe extern "C" {
    /// Declared in `cpp/guard_shim.cpp`; see that file for the full ABI
    /// contract. Never called directly outside this module — all calls go
    /// through [`validate_guards_native`], which upholds every invariant
    /// documented there (valid pointer/length pairs, buffer capacity).
    fn rshooks_validate_guards(
        wasm: *const u8,
        wasm_len: usize,
        out_hook_cost: *mut u64,
        out_cbak_cost: *mut u64,
        log_buf: *mut c_char,
        log_cap: usize,
        out_log_len: *mut usize,
    ) -> i32;
}

/// The worst-case instruction counts the upstream checker computed for a
/// module's `hook()` and `cbak()` entry points. These are also what SetHook
/// fee estimation is derived from upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardVerdict {
    /// Worst-case instruction count for `hook()`.
    pub hook_cost: u64,
    /// Worst-case instruction count for `cbak()`. Zero if the module has no
    /// `cbak` export (the checker still returns a pair; upstream reports 0
    /// for the absent entry point).
    pub cbak_cost: u64,
}

/// Why the native checker did not return a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeGuardError {
    /// The upstream checker ran to completion and rejected the module. The
    /// captured log (identical to what a real node would log) explains why.
    Invalid {
        /// The checker's captured log text, verbatim.
        log: String,
        /// True if the log exceeded [`LOG_BUF_CAPACITY`] and was truncated
        /// (the log text is still the first `LOG_BUF_CAPACITY` bytes).
        truncated: bool,
    },
    /// The upstream checker itself threw a C++ exception (upstream documents
    /// `validateGuards` "may throw `overflow_error`" on malformed input,
    /// e.g. a corrupt LEB128 sequence). This is distinct from `Invalid`:
    /// the checker did not reach a verdict at all.
    Exception {
        /// The exception message plus any log text captured before it was
        /// thrown.
        log: String,
        /// True if the log exceeded [`LOG_BUF_CAPACITY`] and was truncated.
        truncated: bool,
    },
    /// The shim returned a status code this wrapper does not recognize —
    /// indicates an ABI mismatch between this file and `guard_shim.cpp`,
    /// not a property of the input wasm.
    UnknownStatus(i32),
}

impl std::fmt::Display for NativeGuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NativeGuardError::Invalid { log, truncated } => {
                write!(f, "upstream guard checker rejected the module:\n{log}")?;
                if *truncated {
                    write!(f, "\n(log truncated at {LOG_BUF_CAPACITY} bytes)")?;
                }
                Ok(())
            }
            NativeGuardError::Exception { log, truncated } => {
                write!(f, "upstream guard checker threw an exception:\n{log}")?;
                if *truncated {
                    write!(f, "\n(log truncated at {LOG_BUF_CAPACITY} bytes)")?;
                }
                Ok(())
            }
            NativeGuardError::UnknownStatus(code) => {
                write!(f, "guard shim returned unrecognized status code {code}")
            }
        }
    }
}

impl std::error::Error for NativeGuardError {}

/// Runs the vendored upstream guard checker (`validateGuards`) against
/// `wasm`, exactly as a real xahaud node would. This is the authoritative
/// verdict (`docs/DESIGN.md` §6.5) — it supersedes the Rust validator's
/// guard/import/export/shape checks whenever the two disagree.
///
/// # Panics
///
/// Never panics on malformed `wasm` input — invalid or exception-triggering
/// input is reported via `Err`, not a panic.
pub fn validate_guards_native(wasm: &[u8]) -> Result<GuardVerdict, NativeGuardError> {
    let mut hook_cost: u64 = 0;
    let mut cbak_cost: u64 = 0;
    let mut log_buf = vec![0u8; LOG_BUF_CAPACITY];
    let mut log_len: usize = 0;

    // SAFETY: `wasm.as_ptr()`/`wasm.len()` describe a single valid,
    // immutably-borrowed slice for the duration of this call (the borrow
    // checker keeps `wasm` alive across it). `log_buf` is a `Vec<u8>` of
    // exactly `LOG_BUF_CAPACITY` bytes; its pointer and that same capacity
    // are passed together, so the C++ side's `memcpy` (bounded by
    // `min(text.size(), log_cap)`, see `guard_shim.cpp`) cannot write past
    // the end of the allocation. `out_hook_cost`/`out_cbak_cost`/
    // `out_log_len` are `&mut` locals of the right type, so they are valid
    // for the single write the shim performs to each. The function has no
    // documented preconditions beyond these (no global state, safe to call
    // repeatedly).
    let status = unsafe {
        rshooks_validate_guards(
            wasm.as_ptr(),
            wasm.len(),
            &mut hook_cost,
            &mut cbak_cost,
            log_buf.as_mut_ptr().cast::<c_char>(),
            log_buf.len(),
            &mut log_len,
        )
    };

    let truncated = log_len > log_buf.len();
    let copied = log_len.min(log_buf.len());
    let log_text = || String::from_utf8_lossy(log_buf.get(..copied).unwrap_or(&[])).into_owned();

    match status {
        0 => Ok(GuardVerdict {
            hook_cost,
            cbak_cost,
        }),
        1 => Err(NativeGuardError::Invalid {
            log: log_text(),
            truncated,
        }),
        2 => Err(NativeGuardError::Exception {
            log: log_text(),
            truncated,
        }),
        other => Err(NativeGuardError::UnknownStatus(other)),
    }
}
