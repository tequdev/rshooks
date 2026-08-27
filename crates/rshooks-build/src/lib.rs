//! Builds and validates SetHook-compatible WebAssembly modules.
//!
//! The pipeline cleans the module, applies the API-version-specific
//! transformations, optionally inserts loop guards, and validates the result.
//! This host-side crate permits ordinary index-space arithmetic.
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
pub mod sethook_template;
mod unnest;
mod validator;
pub mod whitelist;

pub use cleaner::clean;
pub use fee::{FeeEstimate, estimate_fee};
pub use flatten::{FlattenReport, flatten};
pub use guard::auto_guard;
pub use guard_native::{GuardVerdict, NativeGuardError, validate_guards_native};
pub use unnest::{UnnestReport, unnest};
pub use validator::{ValidationReport, validate};

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
    /// Insert missing loop guards instead of reporting an error.
    pub auto_guard: bool,
    /// The `maxiter` value used for auto-inserted guards.
    pub default_maxiter: u32,
    /// Permit oversized output from build operations. Validation still reports it.
    pub allow_oversize: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            api_version: ApiVersion::default(),
            auto_guard: false,
            default_maxiter: 16,
            allow_oversize: false,
        }
    }
}

/// Runs the full transformation and validation pipeline.
///
/// Version 0 modules are flattened and unnested before guard processing.
/// Returns the transformed bytes and their validation report.
pub fn run_pipeline(wasm: &[u8], opts: &Options) -> anyhow::Result<(Vec<u8>, ValidationReport)> {
    let cleaned = cleaner::clean(wasm, opts)?;
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
/// For version 0, the upstream guard checker is authoritative; Rust-only
/// findings are retained as warnings when the checkers disagree. Size limits
/// are always enforced unless [`Options::allow_oversize`] is set.
pub fn verify(wasm: &[u8], opts: &Options) -> anyhow::Result<ValidationReport> {
    let size_hard_fail = wasm.len() > validator::MAX_SIZE && !opts.allow_oversize;
    let rust_result = validator::validate(wasm, opts);

    if size_hard_fail || opts.api_version != ApiVersion::V0 {
        return rust_result;
    }

    match guard_native::validate_guards_native(wasm) {
        Ok(verdict) => {
            let mut report = match rust_result {
                Ok(report) => report,
                Err(rust_err) => {
                    let mut report = ValidationReport::default();
                    report.warnings.push(format!(
                        "DIVERGENCE: the Rust validator flagged issue(s) that the authoritative \
                         upstream guard checker accepted; the native verdict wins and the \
                         module is treated as valid. Rust findings:\n{rust_err}"
                    ));
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
    fn options_default_values_are_pinned() {
        let o = Options::default();
        assert_eq!(o.api_version, ApiVersion::V0);
        assert!(!o.auto_guard);
        assert_eq!(o.default_maxiter, 16);
        assert!(!o.allow_oversize);
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
}
