//! Builds and validates SetHook-compatible WebAssembly modules.
//!
//! The pipeline cleans the module, applies the API-version-specific
//! transformations, optionally runs the deprecated auto-guard insertion
//! pass, and validates the result. This host-side crate permits ordinary
//! index-space arithmetic.
#![allow(clippy::arithmetic_side_effects)]

pub mod carriers;
pub mod chain_build;
mod cleaner;
mod encode;
pub mod entry_sidecar;
mod fee;
mod flatten;
mod guard;
mod guard_native;
mod ir;
pub mod metadata;
mod optimizer;
pub mod sethook_template;
mod unnest;
mod validator;
pub mod whitelist;

pub use cleaner::clean;
pub use fee::{FeeEstimate, estimate_fee};
pub use flatten::{FlattenReport, flatten};
#[allow(deprecated)]
pub use guard::auto_guard;
pub use guard_native::{GuardVerdict, NativeGuardError, validate_guards_native};
pub use unnest::{UnnestReport, unnest};
pub use validator::{ValidationError, ValidationReport, validate};

/// The Hook API version a module targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiVersion {
    /// Guard-type hooks require loop guards.
    #[default]
    V0,
    /// Gas-type hooks do not require loop guards.
    V1,
}

/// Options threaded through every pipeline stage.
#[derive(Debug, Clone)]
pub struct Options {
    /// Which Hook API version this module targets.
    pub api_version: ApiVersion,
    /// Deprecated: insert missing loop guards instead of reporting an
    /// error. Scheduled for removal; remove the compiler-generated loop at
    /// the source level (`rshooks::buf_eq_*`, `HookStatic`) or write the
    /// loop by hand with `guard!` instead.
    #[deprecated(
        note = "the auto-guard transform is scheduled for removal; remove the \
                compiler-generated loop at the source level (`rshooks::buf_eq_*`, `HookStatic`) \
                or write the loop by hand with `guard!`"
    )]
    pub auto_guard: bool,
    /// Deprecated: the `maxiter` value used for auto-inserted guards. Only
    /// meaningful with the deprecated [`Options::auto_guard`].
    #[deprecated(note = "only meaningful with the deprecated `auto_guard`")]
    pub default_maxiter: u32,
    /// Permit oversized output from build operations. Validation still reports it.
    pub allow_oversize: bool,
    /// Run Binaryen's `wasm-opt` `-Oz` size optimization as the first
    /// pipeline step, before cleaning. On by default.
    pub optimize: bool,
}

#[allow(deprecated)]
impl Default for Options {
    fn default() -> Self {
        Self {
            api_version: ApiVersion::default(),
            auto_guard: false,
            default_maxiter: 16,
            allow_oversize: false,
            optimize: true,
        }
    }
}

/// Runs the full transformation and validation pipeline.
///
/// Version 0 modules are flattened and unnested before guard processing.
/// Returns the transformed bytes and their validation report.
#[allow(deprecated)]
pub fn run_pipeline(wasm: &[u8], opts: &Options) -> anyhow::Result<(Vec<u8>, ValidationReport)> {
    // wasm-opt runs first, on the raw wasm, before cleaning: see
    // `optimizer` for why this ordering is load-bearing.
    let optimized = if opts.optimize {
        optimizer::optimize(wasm)?
    } else {
        wasm.to_vec()
    };
    let cleaned = cleaner::clean(&optimized, opts)?;
    let flattened = if opts.api_version == ApiVersion::V0 {
        let (bytes, report) = flatten::flatten(&cleaned)?;
        for note in &report.notes {
            eprintln!("note: {note}");
        }
        bytes
    } else {
        cleaned
    };
    let unnested = if opts.api_version == ApiVersion::V0 {
        let (bytes, report) = unnest::unnest(&flattened)?;
        for note in &report.notes {
            eprintln!("note: {note}");
        }
        bytes
    } else {
        flattened
    };
    let guarded = if opts.auto_guard && opts.api_version == ApiVersion::V0 {
        guard::auto_guard(&unnested, opts)?
    } else {
        unnested
    };
    let report = verify(&guarded, opts)?;
    Ok((guarded, report))
}

/// Validates `wasm`.
///
/// For version 0, the upstream guard checker is authoritative for guard/WCE
/// findings ([`ValidationError::guard`]): Rust-only findings in that class
/// are retained as warnings when the checkers disagree. It has no bearing
/// on every other rule ([`ValidationError::hard`]) — those remain hard
/// errors regardless of the native verdict. Size limits are always
/// enforced unless [`Options::allow_oversize`] is set.
pub fn verify(wasm: &[u8], opts: &Options) -> anyhow::Result<ValidationReport> {
    let size_hard_fail = wasm.len() > validator::MAX_SIZE && !opts.allow_oversize;
    let rust_result = validator::validate(wasm, opts);

    if size_hard_fail || opts.api_version != ApiVersion::V0 {
        return rust_result.map_err(anyhow::Error::from);
    }

    merge_verdicts(rust_result, guard_native::validate_guards_native(wasm))
}

/// Reconciles the Rust validator's verdict with the native guard checker's
/// verdict for an API-version-0 module. Pulled out of [`verify`] as a pure
/// function so the reconciliation rules can be unit-tested without going
/// through the native FFI call.
///
/// The native checker only ever re-derives guard/WCE findings
/// ([`ValidationError::guard`]); it says nothing about MVP validity,
/// export/import shape, structural sections, or float opcodes
/// ([`ValidationError::hard`]). Its acceptance may downgrade the former to
/// a warning but must never override the latter.
fn merge_verdicts(
    rust_result: Result<ValidationReport, ValidationError>,
    native_result: Result<crate::GuardVerdict, guard_native::NativeGuardError>,
) -> anyhow::Result<ValidationReport> {
    match native_result {
        Ok(verdict) => {
            let mut report = match rust_result {
                Ok(report) => report,
                Err(rust_err) if !rust_err.hard.is_empty() => {
                    anyhow::bail!(rust_err.hard.join("\n"));
                }
                Err(rust_err) => {
                    let mut report = ValidationReport::default();
                    if !rust_err.guard.is_empty() {
                        report.warnings.push(format!(
                            "DIVERGENCE: the Rust validator flagged guard/nesting issue(s) that \
                             the authoritative upstream guard checker accepted; the native \
                             verdict wins and the module is treated as valid. Rust findings:\n{}",
                            rust_err.guard.join("\n")
                        ));
                    }
                    report
                }
            };
            report.guard_verdict = Some(verdict);
            Ok(report)
        }
        Err(native_err) => {
            let mut msg = String::new();
            match &rust_result {
                Ok(report) => {
                    for w in &report.warnings {
                        msg.push_str(&format!("rust validator warning: {w}\n"));
                    }
                    msg.push_str(
                        "DIVERGENCE: the Rust validator accepted this module, but the \
                         authoritative upstream guard checker rejected it. The native verdict \
                         wins.\n\n",
                    );
                }
                Err(rust_err) => {
                    msg.push_str("the Rust validator also flagged issues:\n");
                    msg.push_str(&rust_err.to_string());
                    msg.push_str("\n\n");
                }
            }
            msg.push_str(&format!(
                "upstream guard checker rejected the module (authoritative verdict):\n{native_err}"
            ));
            anyhow::bail!(msg)
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    fn wasm(src: &str) -> Vec<u8> {
        wat::parse_str(src).expect("fixture is valid wat")
    }

    #[test]
    #[allow(deprecated)]
    fn options_default_values_are_pinned() {
        let o = Options::default();
        assert_eq!(o.api_version, ApiVersion::V0);
        assert!(!o.auto_guard);
        assert_eq!(o.default_maxiter, 16);
        assert!(!o.allow_oversize);
        assert!(o.optimize);
    }

    #[test]
    fn run_pipeline_v1_skips_flatten_and_unnest() {
        // A helper function that survives cleaning (reachable from `hook`)
        // but is not a `hook`/`cbak` entry point: under V0 it would be
        // inlined away by flatten; under V1 it must survive untouched.
        let src = r#"
        (module
          (func $helper (param i32) (result i64) (i64.extend_i32_u (local.get 0)))
          (func $hook (param i32) (result i64) (call $helper (local.get 0)))
          (export "hook" (func $hook)))
        "#;
        let opts = Options {
            api_version: ApiVersion::V1,
            // Disabled here so `out` can be compared byte-for-byte against
            // `cleaner::clean`'s direct output below; optimization is
            // orthogonal to this test's flatten/unnest-skip assertion.
            optimize: false,
            ..Options::default()
        };
        let (out, _report) = run_pipeline(&wasm(src), &opts).expect("V1 pipeline succeeds");

        // The cleaned-but-unflattened output must still have 2 defined
        // functions (helper survives as a separate function; V0 would
        // inline it away, leaving only 1).
        let mut func_count = 0u32;
        for payload in wasmparser::Parser::new(0).parse_all(&out) {
            if let wasmparser::Payload::FunctionSection(r) = payload.expect("valid wasm") {
                func_count = r.count();
            }
        }
        assert_eq!(
            func_count, 2,
            "V1 must skip flatten/unnest: helper should survive as its own function"
        );

        // And it must equal `clean`'s output directly (V1 pipeline is
        // exactly cleaner + verify, nothing else).
        let cleaned = cleaner::clean(&wasm(src), &opts).expect("clean succeeds");
        assert_eq!(out, cleaned);
    }

    fn fake_verdict() -> GuardVerdict {
        GuardVerdict {
            hook_cost: 1,
            cbak_cost: 0,
        }
    }

    #[test]
    fn merge_verdicts_downgrades_a_guard_only_rust_error_to_a_warning() {
        let rust_result = Err(ValidationError {
            hard: Vec::new(),
            guard: vec!["fake guard-shape finding".to_string()],
        });
        let report = merge_verdicts(rust_result, Ok(fake_verdict()))
            .expect("a guard-only finding must be tolerated when the native checker accepts");
        assert_eq!(report.guard_verdict, Some(fake_verdict()));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("fake guard-shape finding")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn merge_verdicts_never_downgrades_a_hard_error() {
        let rust_result = Err(ValidationError {
            hard: vec!["fake non-guard finding".to_string()],
            guard: Vec::new(),
        });
        let err = merge_verdicts(rust_result, Ok(fake_verdict())).unwrap_err();
        assert!(
            err.to_string().contains("fake non-guard finding"),
            "a hard finding must survive the native checker's acceptance: {err}"
        );
    }

    #[test]
    fn merge_verdicts_never_downgrades_a_hard_error_even_alongside_a_guard_finding() {
        let rust_result = Err(ValidationError {
            hard: vec!["fake non-guard finding".to_string()],
            guard: vec!["fake guard-shape finding".to_string()],
        });
        let err = merge_verdicts(rust_result, Ok(fake_verdict())).unwrap_err();
        assert!(err.to_string().contains("fake non-guard finding"), "{err}");
    }

    #[test]
    fn merge_verdicts_accepts_cleanly_when_both_checkers_agree() {
        let report = merge_verdicts(Ok(ValidationReport::default()), Ok(fake_verdict()))
            .expect("both checkers accepting must accept");
        assert_eq!(report.guard_verdict, Some(fake_verdict()));
        assert!(report.warnings.is_empty());
    }
}
