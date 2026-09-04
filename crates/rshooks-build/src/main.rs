//! The `rshooks` CLI (package `rshooks-build`): drives `cargo build --target
//! wasm32v1-none`, then post-processes and validates the resulting wasm into
//! SetHook-legal Hook binaries. `build` orchestrates a full `#[hooks]` chain
//! (see `rshooks_build::chain_build`); `clean`/`check` operate on a single
//! already-built wasm file. See `docs/MULTI_HOOK_STRUCT_DESIGN.md` §7, §9.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rshooks_build::chain_build::{ChainBuildArgs, run as run_chain_build};
use rshooks_build::{ApiVersion, Options, ValidationReport};

/// A CLI toolchain for building and validating Xahau Hook wasm binaries.
#[derive(Parser)]
#[command(name = "rshooks", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Builds a Rust crate's `#[hooks]` chain for `wasm32v1-none`: one
    /// discovery build plus one selected build per chain entry, each
    /// cleaned and validated into a SetHook-legal binary, plus a generated
    /// `SetHook` transaction template.
    Build {
        /// Path to the crate's `Cargo.toml` (forwarded to `cargo`).
        #[arg(long)]
        manifest_path: Option<PathBuf>,
        /// Build only the named package (forwarded to `cargo -p`).
        #[arg(short = 'p', long)]
        package: Option<String>,
        /// The Hook API version this module targets. Only `0` is currently
        /// supported for chain builds.
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=1))]
        api_version: u8,
        /// Deprecated: insert missing loop guards instead of treating them
        /// as an error. Scheduled for removal; remove the
        /// compiler-generated loop at the source level
        /// (rshooks::buf_eq_*, HookStatic) or write the loop by hand with
        /// guard! instead.
        #[arg(long)]
        auto_guard: bool,
        /// Deprecated: maxiter used for auto-inserted guards. Only
        /// meaningful with the deprecated --auto-guard.
        #[arg(long, default_value_t = 16)]
        default_maxiter: u32,
        /// Output ROOT directory (default: `<target>/rshooks/<crate-name>`).
        /// Generations are published under `<root>/gen-<n>`, with
        /// `<root>/current` pointing at the latest.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Write per-entry output even if it exceeds the 65,535-byte
        /// SetHook limit (clearly marked invalid).
        #[arg(long)]
        allow_oversize: bool,
        /// Skip the Binaryen `wasm-opt` `-Oz` size-optimization pass that
        /// otherwise runs on each entry's raw wasm before cleaning.
        #[arg(long)]
        no_optimize: bool,
        /// SetHook `Account` placeholder value for the generated template
        /// (default: the literal placeholder `<ACCOUNT>`).
        #[arg(long, value_parser = rshooks_build::sethook_template::validate_account)]
        account: Option<String>,
        /// `HookNamespace` placeholder value (64 hex chars) for the
        /// generated template (default: the literal placeholder
        /// `<NAMESPACE>`).
        #[arg(long, value_parser = rshooks_build::sethook_template::validate_namespace)]
        namespace: Option<String>,
        /// Set `hsfOVERRIDE` on declared (non-gap) template entries,
        /// permitting replacement of an existing installed Hook at that
        /// position.
        #[arg(long = "override")]
        override_flag: bool,
    },
    /// Cleans and validates an already-built wasm file, without invoking
    /// cargo.
    Clean {
        /// The input wasm file.
        input: PathBuf,
        /// Where to write the cleaned binary (default:
        /// `<input>.clean.wasm`).
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
        /// The Hook API version this module targets.
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=1))]
        api_version: u8,
        /// Deprecated: insert missing loop guards instead of treating them
        /// as an error. Scheduled for removal; remove the
        /// compiler-generated loop at the source level
        /// (rshooks::buf_eq_*, HookStatic) or write the loop by hand with
        /// guard! instead.
        #[arg(long)]
        auto_guard: bool,
        /// Deprecated: maxiter used for auto-inserted guards. Only
        /// meaningful with the deprecated --auto-guard.
        #[arg(long, default_value_t = 16)]
        default_maxiter: u32,
        /// Write the output even if it exceeds the 65,535-byte SetHook
        /// limit (clearly marked invalid).
        #[arg(long)]
        allow_oversize: bool,
    },
    /// Validates a wasm file against the full SetHook rule set, without
    /// modifying it. Works on any wasm, including C-built hooks.
    Check {
        /// The wasm file to validate.
        file: PathBuf,
        /// The Hook API version this module targets.
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=1))]
        api_version: u8,
    },
}

fn api_version_from(v: u8) -> ApiVersion {
    if v == 1 {
        ApiVersion::V1
    } else {
        ApiVersion::V0
    }
}

/// Prints a build-time deprecation warning for `--auto-guard` to stderr.
/// Called once per invocation, before running the pipeline, whenever the
/// flag is set on either `Build` or `Clean`.
fn warn_auto_guard_deprecated() {
    eprintln!(
        "warning: --auto-guard is deprecated and scheduled for removal; remove the \
         compiler-generated loop at the source level (rshooks::buf_eq_*, HookStatic) or write \
         the loop by hand with guard! instead"
    );
}

#[allow(deprecated)]
fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Build {
            manifest_path,
            package,
            api_version,
            auto_guard,
            default_maxiter,
            out,
            allow_oversize,
            no_optimize,
            account,
            namespace,
            override_flag,
        } => {
            if auto_guard {
                warn_auto_guard_deprecated();
            }
            let args = ChainBuildArgs {
                manifest_path,
                package,
                api_version,
                auto_guard,
                default_maxiter,
                out,
                allow_oversize,
                no_optimize,
                account,
                namespace,
                override_flag,
            };
            run_chain_build(&args)
        }
        Cmd::Clean {
            input,
            out,
            api_version,
            auto_guard,
            default_maxiter,
            allow_oversize,
        } => {
            if auto_guard {
                warn_auto_guard_deprecated();
            }
            let opts = Options {
                api_version: api_version_from(api_version),
                auto_guard,
                default_maxiter,
                allow_oversize,
                ..Options::default()
            };
            cmd_clean(&input, out, &opts)
        }
        Cmd::Check { file, api_version } => {
            let opts = Options {
                api_version: api_version_from(api_version),
                ..Options::default()
            };
            cmd_check(&file, &opts)
        }
    }
}

fn print_report(report: &ValidationReport) {
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
    if let Some(verdict) = report.guard_verdict {
        println!(
            "worst-case instructions: hook={} cbak={}",
            verdict.hook_cost, verdict.cbak_cost
        );
    }
    println!("max nesting depth: {}", report.max_nesting_depth);
}

fn print_size_and_fee(bytes: &[u8]) {
    let fee = rshooks_build::estimate_fee(bytes.len());
    println!("size: {} bytes", fee.bytes);
    println!(
        "estimated SetHook fee: {} drops ({} XAH)",
        fee.drops,
        fee.xah_string()
    );
}

fn cmd_clean(input: &Path, out: Option<PathBuf>, opts: &Options) -> Result<()> {
    let wasm = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let out_path = out.unwrap_or_else(|| {
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
        input.with_file_name(format!("{stem}.clean.wasm"))
    });
    let (output, report) = run_pipeline_and_report(&wasm, opts)?;
    write_wasm(&output, &out_path, &report)
}

fn run_pipeline_and_report(wasm: &[u8], opts: &Options) -> Result<(Vec<u8>, ValidationReport)> {
    let (output, report) = rshooks_build::run_pipeline(wasm, opts)?;
    print_report(&report);
    Ok((output, report))
}

fn write_wasm(output: &[u8], out_path: &Path, report: &ValidationReport) -> Result<()> {
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory {}", parent.display()))?;
    }
    let mut f = std::fs::File::create(out_path)
        .with_context(|| format!("creating {}", out_path.display()))?;
    f.write_all(output)
        .with_context(|| format!("writing {}", out_path.display()))?;
    println!("wrote {}", out_path.display());
    print_size_and_fee(output);
    if report.oversize_allowed {
        println!("WARNING: output marked INVALID (oversize)");
    }
    Ok(())
}

fn cmd_check(file: &Path, opts: &Options) -> Result<()> {
    let wasm = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    match rshooks_build::verify(&wasm, opts) {
        Ok(report) => {
            print_report(&report);
            println!("OK: {} is a valid SetHook wasm binary", file.display());
            print_size_and_fee(&wasm);
            Ok(())
        }
        Err(e) => {
            eprintln!("INVALID: {} failed validation:", file.display());
            for line in e.to_string().lines() {
                eprintln!("  - {line}");
            }
            std::process::exit(1);
        }
    }
}
