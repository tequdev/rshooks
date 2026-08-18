//! The v2 multi-artifact build orchestrator ("Strategy A" — discovery build
//! + per-index `--cfg` reselected builds).
//!
//! See docs/MULTI_HOOK_STRUCT_DESIGN.md §7 (build strategy) and §9 (SetHook
//! template), and the rshooks v0.2 implementation contract §C, for the
//! normative behavior this module implements.

use std::collections::BTreeMap;
use std::io::{BufRead, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::carriers::{self, EntryDecl};
use crate::metadata::{self, hook_hash};
use crate::{ApiVersion, Options, entry_sidecar, sethook_template};

/// CLI-facing inputs to a `rshooks build` invocation.
#[derive(Debug, Clone, Default)]
pub struct ChainBuildArgs {
    /// Forwarded to every `cargo`/`rustc` invocation.
    pub manifest_path: Option<PathBuf>,
    /// Forwarded as `-p <package>`.
    pub package: Option<String>,
    /// Only `0` (Guard-type) is currently supported for chain builds.
    pub api_version: u8,
    /// Insert missing loop guards instead of treating them as an error.
    pub auto_guard: bool,
    /// `maxiter` used for auto-inserted guards.
    pub default_maxiter: u32,
    /// Output ROOT directory (default: `<target>/rshooks/<crate-name>`).
    pub out: Option<PathBuf>,
    /// Permit oversized per-entry output.
    pub allow_oversize: bool,
    /// SetHook `Account` placeholder value for the generated template.
    pub account: Option<String>,
    /// `HookNamespace` placeholder value for the generated template.
    pub namespace: Option<String>,
    /// Set `hsfOVERRIDE` on declared template entries.
    pub override_flag: bool,
}

/// Runs the full v2 chain build: discovery, per-index selected builds,
/// pipeline, sidecar/template generation, and generation-directory
/// publication. Prints progress and a final summary to stdout/stderr.
pub fn run(args: &ChainBuildArgs) -> Result<()> {
    if args.api_version != 0 {
        bail!(
            "`rshooks build` only supports `--api-version 0` for #[hooks] chain builds \
             (gas-type chain builds are not yet supported)"
        );
    }
    let opts = Options {
        api_version: ApiVersion::V0,
        auto_guard: args.auto_guard,
        default_maxiter: args.default_maxiter,
        allow_oversize: args.allow_oversize,
    };

    let cargo = find_cargo()?;
    let plan = BuildPlan::resolve(&cargo, args.manifest_path.as_deref(), args.package.as_deref())?;
    let rustc = detect_rustc_version(&cargo);

    let root = args
        .out
        .clone()
        .unwrap_or_else(|| plan.target_directory.join("rshooks").join(&plan.package_name));
    std::fs::create_dir_all(&root)
        .with_context(|| format!("creating output root {}", root.display()))?;
    // Held for the *entire* chain build (discovery through publish), not
    // just the final publish step: this closes the window where a
    // concurrent `rshooks build` run sharing the same cargo target
    // directory could overwrite the cdylib artifact between this process
    // compiling it and reading it back. Released on every exit path
    // (including early `?` returns) via `LockGuard`'s `Drop` impl.
    let _lock = acquire_lock(&root)?;

    println!("discovery build ({})", plan.package_name);
    let discovery_bytes = plan.run_discovery()?;
    let discovery_carriers = carriers::extract_chain_carriers(&discovery_bytes, &plan.package_name)?;

    let mut entries_sorted = discovery_carriers.hooks.entries.clone();
    entries_sorted.sort_by_key(|e| e.index);

    let mut declared_final: Vec<(u8, EntryDecl, Vec<u8>)> = Vec::new();
    let mut hook_hashes: BTreeMap<u8, String> = BTreeMap::new();
    let mut staged_wasms: Vec<(String, Vec<u8>)> = Vec::new();
    let mut staged_sidecars: Vec<(String, Vec<u8>)> = Vec::new();

    for entry in &entries_sorted {
        let index = entry.index;
        println!("building entry {index} (`{}`)", entry.hook_fn);

        let selected_bytes = plan.run_selected(index)?;
        let selected_carriers =
            carriers::extract_chain_carriers(&selected_bytes, &plan.package_name)?;
        carriers::assert_carriers_match(&discovery_carriers, &selected_carriers, index)?;

        precheck_exports(&selected_bytes, entry)?;

        let (final_bytes, report) = crate::run_pipeline(&selected_bytes, &opts)
            .with_context(|| format!("entry {index} (`{}`)", entry.hook_fn))?;
        for warning in &report.warnings {
            eprintln!("warning: entry {index}: {warning}");
        }

        let uses_emit = metadata::uses_reachable_emit(&final_bytes)?;
        for warning in emit_truth_table_warnings(
            index,
            &entry.hook_fn,
            entry.hook_can_emit.as_deref(),
            uses_emit,
            entry.cbak_fn.is_some(),
        ) {
            eprintln!("warning: {warning}");
        }

        hook_hashes.insert(index, hook_hash(&final_bytes));

        let sidecar = entry_sidecar::build_entry_sidecar(
            entry,
            &discovery_carriers.chain,
            &final_bytes,
            &report,
            rustc.clone(),
        )?;
        for warning in &sidecar.warnings {
            eprintln!("warning: {warning}");
        }

        let base_name = format!("{index}.{}", entry.hook_fn);
        staged_wasms.push((format!("{base_name}.wasm"), final_bytes.clone()));
        staged_sidecars.push((format!("{base_name}.metadata.json"), sidecar.bytes));

        declared_final.push((index, entry.clone(), final_bytes));
    }

    let max_index = declared_final
        .iter()
        .map(|(index, _, _)| *index)
        .max()
        .context("no declared #[hook] entries")?;
    let declared_indices: Vec<u8> = declared_final.iter().map(|(index, _, _)| *index).collect();
    let gap_indices: Vec<u8> = (0..=max_index)
        .filter(|index| !declared_indices.contains(index))
        .collect();

    let template_inputs: Vec<sethook_template::TemplateInput<'_>> = declared_final
        .iter()
        .map(|(index, entry, wasm)| (*index, entry, wasm.as_slice()))
        .collect();
    let template_bytes = sethook_template::build_template_json(
        &template_inputs,
        args.account.as_deref(),
        args.namespace.as_deref(),
        args.override_flag,
    )?;
    let required_amendments = sethook_template::required_amendments(&entries_sorted);
    let meta_bytes = sethook_template::build_template_meta(
        &plan.package_name,
        &plan.package_version,
        &hook_hashes,
        &declared_indices,
        &gap_indices,
        max_index,
        &required_amendments,
    )?;

    let gen_dir = publish(
        &root,
        &staged_wasms,
        &staged_sidecars,
        &template_bytes,
        &meta_bytes,
    )?;

    print_summary(&gen_dir, &staged_wasms, &staged_sidecars);

    Ok(())
}

fn print_summary(
    gen_dir: &Path,
    staged_wasms: &[(String, Vec<u8>)],
    staged_sidecars: &[(String, Vec<u8>)],
) {
    for (name, bytes) in staged_wasms {
        let fee = crate::estimate_fee(bytes.len());
        println!(
            "wrote {} ({} bytes, estimated SetHook fee {} drops)",
            gen_dir.join(name).display(),
            fee.bytes,
            fee.drops
        );
    }
    for (name, _) in staged_sidecars {
        println!("wrote {}", gen_dir.join(name).display());
    }
    println!("wrote {}", gen_dir.join("sethook.template.json").display());
    println!(
        "wrote {}",
        gen_dir.join("sethook.template.meta.json").display()
    );
}

/// Checks the raw (pre-cleaning) export table for exactly the exports the
/// selected build's `#[cfg(rshooks_entry = "i")]` branch should have
/// produced: `hook`, plus `cbak` iff this entry declares a cbak function.
/// Must run before the pipeline, since cleaning strips carrier exports and
/// would otherwise mask a missing/unexpected raw export.
fn precheck_exports(wasm: &[u8], entry: &EntryDecl) -> Result<()> {
    let mut has_hook = false;
    let mut has_cbak = false;

    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let payload = payload.context("parsing raw wasm to check hook/cbak exports")?;
        let wasmparser::Payload::ExportSection(reader) = payload else {
            continue;
        };
        for export in reader {
            let export = export.context("reading raw wasm export")?;
            if export.kind != wasmparser::ExternalKind::Func {
                continue;
            }
            if export.name == "hook" {
                has_hook = true;
            }
            if export.name == "cbak" {
                has_cbak = true;
            }
        }
    }

    if !has_hook {
        bail!(
            "entry {} (`{}`): the selected build does not export `hook`",
            entry.index,
            entry.hook_fn
        );
    }
    let wants_cbak = entry.cbak_fn.is_some();
    if wants_cbak && !has_cbak {
        bail!(
            "entry {} (`{}`): declares a `cbak` function but the selected build does not export \
             `cbak`",
            entry.index,
            entry.hook_fn
        );
    }
    if !wants_cbak && has_cbak {
        bail!(
            "entry {} (`{}`): the selected build exports `cbak`, but this entry declares no \
             cbak function",
            entry.index,
            entry.hook_fn
        );
    }
    Ok(())
}

/// The design §6.3 emit/can_emit/cbak truth table, evaluated against the
/// final (post-pipeline) wasm's reachable `emit` import.
fn emit_truth_table_warnings(
    index: u8,
    hook_fn: &str,
    can_emit: Option<&[String]>,
    uses_emit: bool,
    has_cbak: bool,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let label = format!("entry {index} (`{hook_fn}`)");
    match can_emit {
        None => match (uses_emit, has_cbak) {
            (true, false) => {
                warnings.push(format!("{label}: uses `emit`, but no `cbak` is declared"))
            }
            (false, true) => warnings.push(format!(
                "{label}: declares `cbak`, but the final wasm never reaches `emit`"
            )),
            _ => {}
        },
        Some([]) => {
            if uses_emit {
                warnings.push(format!(
                    "{label}: `can_emit = []` (deny-all), but the final wasm uses `emit`, which \
                     will always fail at runtime"
                ));
            } else if has_cbak {
                warnings.push(format!(
                    "{label}: declares `cbak` even though `can_emit = []` and the wasm never \
                     emits"
                ));
            }
        }
        Some(_list) => {
            if uses_emit && !has_cbak {
                warnings.push(format!("{label}: uses `emit`, but no `cbak` is declared"));
            } else if !uses_emit {
                warnings.push(format!(
                    "{label}: declares `can_emit`, but the final wasm never reaches `emit`"
                ));
            }
        }
    }
    warnings
}

/// The invariant inputs to every `cargo`/`rustc` invocation in one build
/// (discovery and all per-index selected builds): resolved package
/// identity, workspace paths, and the authoritative lockfile digest. See
/// design §7.1.
struct BuildPlan {
    cargo: PathBuf,
    manifest_path: Option<PathBuf>,
    package_id: String,
    package_name: String,
    package_version: String,
    cdylib_target_name: String,
    target_directory: PathBuf,
    private_target_dir: PathBuf,
    lockfile_path: PathBuf,
    lockfile_digest: [u8; 32],
}

impl BuildPlan {
    fn resolve(cargo: &Path, manifest_path: Option<&Path>, package: Option<&str>) -> Result<Self> {
        let metadata_json = run_cargo_metadata(cargo, manifest_path)?;

        let workspace_root = metadata_json
            .get("workspace_root")
            .and_then(Value::as_str)
            .context("`cargo metadata` output missing `workspace_root`")?;
        let target_directory = metadata_json
            .get("target_directory")
            .and_then(Value::as_str)
            .context("`cargo metadata` output missing `target_directory`")?;
        let packages = metadata_json
            .get("packages")
            .and_then(Value::as_array)
            .context("`cargo metadata` output missing `packages`")?;

        let package_value = if let Some(name) = package {
            packages
                .iter()
                .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
                .with_context(|| format!("package `{name}` not found in `cargo metadata` output"))?
        } else {
            let root_id = metadata_json
                .get("resolve")
                .and_then(|r| r.get("root"))
                .and_then(Value::as_str);
            match root_id {
                Some(id) => packages
                    .iter()
                    .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
                    .context("`cargo metadata` `resolve.root` package not found in `packages`")?,
                None => bail!(
                    "could not determine the target package (the workspace has multiple members \
                     or a virtual manifest root); pass `-p <package>`"
                ),
            }
        };

        let package_id = package_value
            .get("id")
            .and_then(Value::as_str)
            .context("package entry missing `id`")?
            .to_string();
        let package_name = package_value
            .get("name")
            .and_then(Value::as_str)
            .context("package entry missing `name`")?
            .to_string();
        let package_version = package_value
            .get("version")
            .and_then(Value::as_str)
            .context("package entry missing `version`")?
            .to_string();

        let targets = package_value
            .get("targets")
            .and_then(Value::as_array)
            .context("package entry missing `targets`")?;
        let cdylib_target_name = targets
            .iter()
            .find(|t| {
                t.get("kind")
                    .and_then(Value::as_array)
                    .is_some_and(|kinds| kinds.iter().any(|k| k.as_str() == Some("cdylib")))
            })
            .and_then(|t| t.get("name"))
            .and_then(Value::as_str)
            .with_context(|| format!("package `{package_name}` has no `cdylib` target"))?
            .to_string();

        let workspace_root = PathBuf::from(workspace_root);
        let target_directory = PathBuf::from(target_directory);
        let private_target_dir = target_directory.join("rshooks-build");

        let lockfile_path = workspace_root.join("Cargo.lock");
        if !lockfile_path.exists() {
            run_generate_lockfile(cargo, manifest_path)?;
        }
        let lockfile_digest = digest_file(&lockfile_path)?;

        Ok(Self {
            cargo: cargo.to_path_buf(),
            manifest_path: manifest_path.map(Path::to_path_buf),
            package_id,
            package_name,
            package_version,
            cdylib_target_name,
            target_directory,
            private_target_dir,
            lockfile_path,
            lockfile_digest,
        })
    }

    fn verify_lockfile_unchanged(&self) -> Result<()> {
        let digest = digest_file(&self.lockfile_path)?;
        if digest != self.lockfile_digest {
            bail!(
                "the workspace lockfile at {} changed during the build; this must not happen \
                 under `--locked` and indicates a concurrent `cargo` invocation or a build \
                 script that rewrites the lockfile",
                self.lockfile_path.display()
            );
        }
        Ok(())
    }

    fn base_cargo_args(&self, subcommand: &str) -> Vec<String> {
        let mut args = vec![
            subcommand.to_string(),
            "--release".to_string(),
            "--locked".to_string(),
            "--target".to_string(),
            "wasm32v1-none".to_string(),
            "--target-dir".to_string(),
        ];
        args.push(self.private_target_dir.display().to_string());
        args
    }

    fn run_discovery(&self) -> Result<Vec<u8>> {
        let mut cmd = Command::new(&self.cargo);
        cmd.args(self.base_cargo_args("build"));
        if let Some(mp) = &self.manifest_path {
            cmd.arg("--manifest-path").arg(mp);
        }
        cmd.args(["-p", &self.package_id]);
        cmd.arg("--message-format=json-render-diagnostics");
        let artifact = self.run_cargo_and_capture_artifact(cmd)?;
        std::fs::read(&artifact)
            .with_context(|| format!("reading discovery build artifact {}", artifact.display()))
    }

    fn run_selected(&self, index: u8) -> Result<Vec<u8>> {
        let mut cmd = Command::new(&self.cargo);
        cmd.args(self.base_cargo_args("rustc"));
        if let Some(mp) = &self.manifest_path {
            cmd.arg("--manifest-path").arg(mp);
        }
        cmd.args(["-p", &self.package_id, "--crate-type", "cdylib"]);
        cmd.arg("--message-format=json-render-diagnostics");
        cmd.arg("--");
        cmd.arg("--cfg").arg(format!("rshooks_entry=\"{index}\""));
        cmd.arg("--check-cfg").arg(
            "cfg(rshooks_entry,values(\"0\",\"1\",\"2\",\"3\",\"4\",\"5\",\"6\",\"7\",\"8\",\"9\"))",
        );
        let artifact = self.run_cargo_and_capture_artifact(cmd)?;
        std::fs::read(&artifact)
            .with_context(|| format!("reading entry {index} build artifact {}", artifact.display()))
    }

    fn run_cargo_and_capture_artifact(&self, mut cmd: Command) -> Result<PathBuf> {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn `{}`", self.cargo.display()))?;
        let stdout = child
            .stdout
            .take()
            .context("internal error: cargo's stdout was not piped")?;
        let reader = std::io::BufReader::new(stdout);

        let mut messages: Vec<Value> = Vec::new();
        for line in reader.lines() {
            let line = line.context("reading cargo output")?;
            if line.trim().is_empty() {
                continue;
            }
            // cargo can emit non-JSON lines on some setups; ignore those.
            if let Ok(msg) = serde_json::from_str(&line) {
                messages.push(msg);
            }
        }

        let status = child.wait().context("waiting for cargo")?;
        self.verify_lockfile_unchanged()?;
        if !status.success() {
            bail!("cargo invocation failed ({status})");
        }
        select_wasm_artifact(&self.package_id, &self.cdylib_target_name, &messages)
    }
}

/// Selects the single `.wasm` artifact cargo produced for `package_id`'s
/// `cdylib_target_name` target from its `--message-format=json` output.
///
/// Collects every candidate path across all matching `compiler-artifact`
/// messages rather than taking the last one seen ("last-wins"): more than
/// one *distinct* path is an unresolvable ambiguity, reported as an error
/// listing every candidate, rather than silently picking whichever happened
/// to be reported last. The same path reported more than once (e.g. an
/// identical message repeated) is not ambiguous.
fn select_wasm_artifact(
    package_id: &str,
    cdylib_target_name: &str,
    messages: &[Value],
) -> Result<PathBuf> {
    let mut artifacts: Vec<PathBuf> = Vec::new();
    for msg in messages {
        if msg.get("reason").and_then(Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let package_matches = msg.get("package_id").and_then(Value::as_str) == Some(package_id);
        let is_cdylib = msg
            .get("target")
            .and_then(|t| t.get("kind"))
            .and_then(Value::as_array)
            .is_some_and(|kinds| kinds.iter().any(|k| k.as_str() == Some("cdylib")));
        let target_name_matches = msg
            .get("target")
            .and_then(|t| t.get("name"))
            .and_then(Value::as_str)
            == Some(cdylib_target_name);
        if !(package_matches && is_cdylib && target_name_matches) {
            continue;
        }
        if let Some(filenames) = msg.get("filenames").and_then(Value::as_array) {
            for f in filenames {
                if let Some(s) = f.as_str()
                    && s.ends_with(".wasm")
                {
                    artifacts.push(PathBuf::from(s));
                }
            }
        }
    }

    let mut distinct: Vec<PathBuf> = Vec::new();
    for path in artifacts {
        if !distinct.contains(&path) {
            distinct.push(path);
        }
    }

    match distinct.len() {
        0 => bail!(
            "cargo did not produce a .wasm artifact for the resolved cdylib target; is this a \
             `cdylib` crate for wasm32v1-none?"
        ),
        1 => distinct
            .into_iter()
            .next()
            .context("internal error: distinct artifact candidate list became empty"),
        _ => {
            let list = distinct
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "cargo produced multiple distinct .wasm artifacts for the resolved cdylib \
                 target ({list}); this is ambiguous and rshooks-build refuses to guess which \
                 one to use"
            );
        }
    }
}

fn run_cargo_metadata(cargo: &Path, manifest_path: Option<&Path>) -> Result<Value> {
    let mut cmd = Command::new(cargo);
    cmd.args(["metadata", "--format-version", "1"]);
    if let Some(mp) = manifest_path {
        cmd.arg("--manifest-path").arg(mp);
    }
    let output = cmd
        .output()
        .with_context(|| format!("failed to spawn `{}`", cargo.display()))?;
    if !output.status.success() {
        bail!(
            "`cargo metadata` failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("parsing `cargo metadata` JSON output")
}

fn run_generate_lockfile(cargo: &Path, manifest_path: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new(cargo);
    cmd.arg("generate-lockfile");
    if let Some(mp) = manifest_path {
        cmd.arg("--manifest-path").arg(mp);
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn `{}`", cargo.display()))?;
    if !status.success() {
        bail!("`cargo generate-lockfile` failed ({status})");
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<[u8; 32]> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

/// Locates the `cargo` executable to invoke. The user's shell PATH is
/// inherited verbatim, so this just needs to find *a* `cargo` on it — the
/// same one the user's shell would run.
fn find_cargo() -> Result<PathBuf> {
    if let Ok(cargo) = std::env::var("CARGO") {
        return Ok(PathBuf::from(cargo));
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("cargo");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "could not find `cargo` on PATH; run `rshooks build` from a shell where `cargo build` \
         already works"
    )
}

/// Maximum time to wait for a `rustc -V` probe before giving up. Guards
/// against a broken/blocking shim (e.g. a rustup proxy that hangs or tries
/// to download a toolchain) hanging the whole build.
const RUSTC_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const RUSTC_PROBE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Detects the `rustc` toolchain that performed this build, for the
/// sidecar's `builder` provenance record. Prefers the `rustc` that sits
/// next to the resolved `cargo`, then falls back to whatever `rustc` is on
/// `PATH`. Never fails the build: any spawn or parse problem returns
/// `None`.
fn detect_rustc_version(cargo: &Path) -> Option<String> {
    let sibling = cargo.parent().map(|dir| dir.join("rustc"));
    sibling
        .into_iter()
        .chain(std::iter::once(PathBuf::from("rustc")))
        .find_map(|candidate| run_rustc_version(&candidate))
}

fn run_rustc_version(rustc: &Path) -> Option<String> {
    let mut child = Command::new(rustc)
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let deadline = Instant::now()
        .checked_add(RUSTC_PROBE_TIMEOUT)
        .unwrap_or_else(Instant::now);
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(RUSTC_PROBE_POLL_INTERVAL),
        }
    };

    if !status.success() {
        return None;
    }

    let mut stdout = String::new();
    child.stdout.take()?.read_to_string(&mut stdout).ok()?;
    let first_line = stdout.lines().next()?.trim();
    (!first_line.is_empty()).then(|| first_line.to_string())
}

/// Guards a staging directory: removes it on `Drop` unless [`commit`] was
/// called. [`commit`]: StagingGuard::commit
struct StagingGuard {
    path: PathBuf,
    committed: bool,
}

impl StagingGuard {
    fn new(path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&path)
            .with_context(|| format!("creating staging directory {}", path.display()))?;
        Ok(Self {
            path,
            committed: false,
        })
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Guards the publish advisory lock: removes the lock file on `Drop`.
#[derive(Debug)]
struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn acquire_lock(root: &Path) -> Result<LockGuard> {
    let lock_path = root.join(".lock");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut file) => {
            let _ = writeln!(file, "{}", std::process::id());
            Ok(LockGuard { path: lock_path })
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => bail!(
            "could not acquire the rshooks-build publish lock at {} (another build may be in \
             progress); if you're certain no build is running, a crashed process may have left \
             this lock behind — remove the file and retry",
            lock_path.display()
        ),
        Err(error) => {
            Err(error).with_context(|| format!("creating lock file {}", lock_path.display()))
        }
    }
}

fn write_staged_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Publishes one build's artifacts to a fresh generation directory under
/// `root`, atomically re-pointing `current`, then prunes old generations.
/// The caller must already hold the publish advisory lock for `root` (see
/// [`acquire_lock`]) for the whole chain build, not just this step — `run`
/// acquires it once, before discovery, and holds it across every entry
/// build and every `publish` call. On any failure, staging is removed and
/// `current` is left untouched (design §7.4, contract §C item 7).
fn publish(
    root: &Path,
    staged_wasms: &[(String, Vec<u8>)],
    staged_sidecars: &[(String, Vec<u8>)],
    template_bytes: &[u8],
    meta_bytes: &[u8],
) -> Result<PathBuf> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("creating output root {}", root.display()))?;

    let staging_path = root.join(format!(".staging-{}", std::process::id()));
    let staging = StagingGuard::new(staging_path.clone())?;

    for (name, bytes) in staged_wasms {
        write_staged_file(&staging_path.join(name), bytes)?;
    }
    for (name, bytes) in staged_sidecars {
        write_staged_file(&staging_path.join(name), bytes)?;
    }
    write_staged_file(&staging_path.join("sethook.template.json"), template_bytes)?;
    write_staged_file(
        &staging_path.join("sethook.template.meta.json"),
        meta_bytes,
    )?;

    let next_n = next_generation_number(root)?;
    let gen_name = format!("gen-{next_n}");
    let gen_dir = root.join(&gen_name);
    std::fs::rename(&staging_path, &gen_dir)
        .with_context(|| format!("publishing staged artifacts to {}", gen_dir.display()))?;
    staging.commit();

    update_current(root, &gen_name)?;
    retain_latest_generations(root, 2);

    Ok(gen_dir)
}

fn next_generation_number(root: &Path) -> Result<u64> {
    let mut max = 0u64;
    for (n, _) in list_generations(root)? {
        if n > max {
            max = n;
        }
    }
    max.checked_add(1).context("generation counter overflow")
}

fn list_generations(root: &Path) -> Result<Vec<(u64, PathBuf)>> {
    let mut gens = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(gens),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", root.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("reading entry in {}", root.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name.strip_prefix("gen-")
            && let Ok(n) = rest.parse::<u64>()
        {
            gens.push((n, entry.path()));
        }
    }
    gens.sort_by_key(|(n, _)| *n);
    Ok(gens)
}

/// Grace period below which a generation directory outside the retained
/// newest-`keep` window is still never pruned — protects a generation from
/// an in-flight or just-finished concurrent/overlapping build.
const PRUNE_GRACE_PERIOD: Duration = Duration::from_secs(10 * 60);

/// Whether a generation directory outside the retained-newest window is
/// actually eligible for pruning, given how long ago it was last modified.
/// Pure so it can be unit tested with synthetic ages, no filesystem or
/// sleeping required.
fn is_prune_eligible(age_since_modified: Duration) -> bool {
    age_since_modified >= PRUNE_GRACE_PERIOD
}

/// Deletes generations older than the newest `keep`, skipping any candidate
/// whose mtime is within [`PRUNE_GRACE_PERIOD`] (see [`is_prune_eligible`]).
/// Best-effort: failures to remove an old generation, or to read its mtime,
/// are not fatal — an unreadable mtime is treated as "not yet eligible"
/// rather than deleted (a resolved `current -> gen-N` consumer may still be
/// reading a pruning candidate; see design §7.4).
fn retain_latest_generations(root: &Path, keep: usize) {
    let Ok(gens) = list_generations(root) else {
        return;
    };
    if gens.len() <= keep {
        return;
    }
    let remove_count = gens.len() - keep;
    let now = SystemTime::now();
    for (_, path) in gens.iter().take(remove_count) {
        let eligible = std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(is_prune_eligible);
        if eligible {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[cfg(unix)]
fn update_current(root: &Path, gen_name: &str) -> Result<()> {
    use std::os::unix::fs::symlink;
    let tmp = root.join("current.tmp");
    let _ = std::fs::remove_file(&tmp);
    symlink(gen_name, &tmp).with_context(|| format!("creating symlink {}", tmp.display()))?;
    std::fs::rename(&tmp, root.join("current"))
        .with_context(|| format!("publishing `current` symlink under {}", root.display()))
}

#[cfg(not(unix))]
fn update_current(root: &Path, gen_name: &str) -> Result<()> {
    let tmp = root.join("current.tmp");
    std::fs::write(&tmp, gen_name).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, root.join("current"))
        .with_context(|| format!("publishing `current` marker under {}", root.display()))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::carriers::OnDecl;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rshooks-build-chain-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time moves forward")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated test directory");
        dir
    }

    fn entry_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (func $hook (param i32) (result i64) i64.const 0)
              (export "hook" (func $hook)))
            "#,
        )
        .expect("valid hook-only fixture")
    }

    fn entry_wasm_with_cbak() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (func $hook (param i32) (result i64) i64.const 0)
              (func $cbak (param i32) (result i64) i64.const 0)
              (export "hook" (func $hook))
              (export "cbak" (func $cbak)))
            "#,
        )
        .expect("valid hook+cbak fixture")
    }

    fn omitted_on() -> OnDecl {
        OnDecl {
            form: "omitted".to_string(),
            hook_on: None,
            hook_on_incoming: None,
            hook_on_outgoing: None,
        }
    }

    fn entry(index: u8, cbak_fn: Option<&str>) -> EntryDecl {
        EntryDecl {
            index,
            hook_fn: "deposit".to_string(),
            cbak_fn: cbak_fn.map(str::to_string),
            hook_name: None,
            on: omitted_on(),
            hook_can_emit: None,
            description: None,
        }
    }

    #[test]
    fn precheck_exports_requires_hook_and_matches_cbak_declaration() {
        assert!(precheck_exports(&entry_wasm(), &entry(0, None)).is_ok());
        assert!(precheck_exports(&entry_wasm(), &entry(0, Some("deposit_cbak"))).is_err());
        assert!(precheck_exports(&entry_wasm_with_cbak(), &entry(0, None)).is_err());
        assert!(precheck_exports(&entry_wasm_with_cbak(), &entry(0, Some("deposit_cbak"))).is_ok());
    }

    #[test]
    fn precheck_exports_rejects_missing_hook_export() {
        let wasm = wat::parse_str(r#"(module (func $noop))"#).expect("valid noop fixture");
        let err = precheck_exports(&wasm, &entry(0, None)).expect_err("must fail");
        assert!(format!("{err:#}").contains("does not export `hook`"));
    }

    #[test]
    fn emit_truth_table_matches_design_section_6_3() {
        // (can_emit, uses_emit, has_cbak) -> expect a warning
        let cases: &[(Option<&[&str]>, bool, bool, bool)] = &[
            (None, true, true, false),
            (None, true, false, true),
            (None, false, true, true),
            (None, false, false, false),
            (Some(&[]), true, true, true),
            (Some(&[]), true, false, true),
            (Some(&[]), false, true, true),
            (Some(&[]), false, false, false),
            (Some(&["Payment"]), true, true, false),
            (Some(&["Payment"]), true, false, true),
            (Some(&["Payment"]), false, true, true),
            (Some(&["Payment"]), false, false, true),
        ];
        for (can_emit, uses_emit, has_cbak, expect_warning) in cases {
            let owned: Option<Vec<String>> =
                can_emit.map(|list| list.iter().map(|s| (*s).to_string()).collect());
            let warnings =
                emit_truth_table_warnings(0, "deposit", owned.as_deref(), *uses_emit, *has_cbak);
            assert_eq!(
                !warnings.is_empty(),
                *expect_warning,
                "can_emit={can_emit:?} uses_emit={uses_emit} has_cbak={has_cbak}: got {warnings:?}"
            );
        }
    }

    #[test]
    fn next_generation_number_starts_at_one_and_increments() {
        let root = temp_dir("gen-numbering");
        assert_eq!(next_generation_number(&root).expect("compute next"), 1);
        std::fs::create_dir(root.join("gen-1")).expect("create gen-1");
        std::fs::create_dir(root.join("gen-3")).expect("create gen-3");
        std::fs::create_dir(root.join("not-a-gen")).expect("create unrelated dir");
        assert_eq!(next_generation_number(&root).expect("compute next"), 4);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    /// Backdates a directory's mtime by `age`, so retention tests can
    /// exercise the grace-period boundary without sleeping.
    fn set_mtime(path: &Path, age: Duration) {
        let target = SystemTime::now()
            .checked_sub(age)
            .expect("age is representable as a past SystemTime");
        let file = std::fs::File::open(path).expect("open directory for mtime adjustment");
        file.set_modified(target).expect("set directory mtime");
    }

    #[test]
    fn prune_eligibility_respects_grace_period() {
        assert!(!is_prune_eligible(Duration::from_secs(0)));
        assert!(!is_prune_eligible(Duration::from_secs(9 * 60)));
        assert!(is_prune_eligible(PRUNE_GRACE_PERIOD));
        assert!(is_prune_eligible(Duration::from_secs(20 * 60)));
    }

    #[test]
    fn retention_keeps_only_the_newest_two_generations() {
        let root = temp_dir("retention");
        for n in 1..=4 {
            let dir = root.join(format!("gen-{n}"));
            std::fs::create_dir(&dir).expect("create generation");
            // Old enough to clear the grace period, so eligibility here is
            // driven purely by the newest-2 retention window.
            set_mtime(&dir, Duration::from_secs(20 * 60));
        }
        retain_latest_generations(&root, 2);
        assert!(!root.join("gen-1").exists());
        assert!(!root.join("gen-2").exists());
        assert!(root.join("gen-3").exists());
        assert!(root.join("gen-4").exists());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn retention_protects_recent_generations_within_grace_period() {
        let root = temp_dir("retention-grace");
        for n in 1..=4 {
            std::fs::create_dir(root.join(format!("gen-{n}"))).expect("create generation");
        }
        // Freshly created: even gen-1/gen-2 (outside the newest-2 window)
        // are within the grace period and must survive.
        retain_latest_generations(&root, 2);
        assert!(
            root.join("gen-1").exists(),
            "recent generation protected by grace period"
        );
        assert!(
            root.join("gen-2").exists(),
            "recent generation protected by grace period"
        );
        assert!(root.join("gen-3").exists());
        assert!(root.join("gen-4").exists());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn publish_creates_generation_and_current_pointer_and_prunes_old() {
        let root = temp_dir("publish");
        let wasms = vec![("0.deposit.wasm".to_string(), b"AA".to_vec())];
        let sidecars = vec![("0.deposit.metadata.json".to_string(), b"{}".to_vec())];

        // `publish` no longer acquires the lock itself — `run` acquires it
        // once for the whole chain build and holds it across every
        // `publish` call; simulate that here.
        let lock = acquire_lock(&root).expect("acquire lock for the whole build, as `run` does");

        let gen1 = publish(&root, &wasms, &sidecars, b"{}", b"{}").expect("first publish");
        assert!(gen1.join("0.deposit.wasm").exists());
        assert!(gen1.join("sethook.template.json").exists());
        assert!(gen1.join("sethook.template.meta.json").exists());

        let gen2 = publish(&root, &wasms, &sidecars, b"{}", b"{}").expect("second publish");
        assert_ne!(gen1, gen2);
        assert!(gen1.exists(), "generations are pruned only beyond keep=2");

        #[cfg(unix)]
        {
            let resolved = std::fs::read_link(root.join("current")).expect("current is a symlink");
            assert_eq!(resolved, PathBuf::from(gen2.file_name().expect("gen2 name")));
        }
        #[cfg(not(unix))]
        {
            let pointer = std::fs::read_to_string(root.join("current")).expect("current marker");
            assert_eq!(pointer, gen2.file_name().expect("gen2 name").to_string_lossy());
        }

        assert!(
            root.join(".lock").exists(),
            "lock stays held across multiple publishes within one build"
        );
        drop(lock);
        assert!(
            !root.join(".lock").exists(),
            "lock is released once the whole build completes"
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    fn artifact_message(package_id: &str, target_name: &str, kind: &str, filenames: &[&str]) -> Value {
        serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": package_id,
            "target": { "kind": [kind], "name": target_name },
            "filenames": filenames,
        })
    }

    #[test]
    fn select_wasm_artifact_picks_the_single_matching_candidate() {
        let messages = vec![
            artifact_message("pkg", "vault", "lib", &["target/wasm32v1-none/release/vault.rlib"]),
            artifact_message("pkg", "vault", "cdylib", &["target/wasm32v1-none/release/vault.wasm"]),
        ];
        let path =
            select_wasm_artifact("pkg", "vault", &messages).expect("selects the one candidate");
        assert_eq!(path, PathBuf::from("target/wasm32v1-none/release/vault.wasm"));
    }

    #[test]
    fn select_wasm_artifact_rejects_no_candidates() {
        let messages = vec![artifact_message("other-pkg", "vault", "cdylib", &["x.wasm"])];
        let err = select_wasm_artifact("pkg", "vault", &messages).expect_err("must fail");
        assert!(format!("{err:#}").contains("did not produce a .wasm artifact"));
    }

    #[test]
    fn select_wasm_artifact_treats_a_repeated_identical_path_as_unambiguous() {
        let messages = vec![
            artifact_message("pkg", "vault", "cdylib", &["target/dir/vault.wasm"]),
            artifact_message("pkg", "vault", "cdylib", &["target/dir/vault.wasm"]),
        ];
        let path = select_wasm_artifact("pkg", "vault", &messages)
            .expect("an identical path repeated is not ambiguous");
        assert_eq!(path, PathBuf::from("target/dir/vault.wasm"));
    }

    #[test]
    fn select_wasm_artifact_rejects_distinct_candidates_as_ambiguous() {
        let messages = vec![
            artifact_message("pkg", "vault", "cdylib", &["target/a/vault.wasm"]),
            artifact_message("pkg", "vault", "cdylib", &["target/b/vault.wasm"]),
        ];
        let err = select_wasm_artifact("pkg", "vault", &messages).expect_err("must fail");
        let message = format!("{err:#}");
        assert!(message.contains("ambiguous"));
        assert!(message.contains("target/a/vault.wasm"));
        assert!(message.contains("target/b/vault.wasm"));
    }

    #[test]
    fn acquire_lock_rejects_concurrent_holder() {
        let root = temp_dir("lock");
        let guard = acquire_lock(&root).expect("first lock succeeds");
        let err = acquire_lock(&root).expect_err("second lock must fail while held");
        assert!(format!("{err:#}").contains("could not acquire"));
        drop(guard);
        let _ = acquire_lock(&root).expect("lock is released after drop");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn staging_guard_removes_directory_unless_committed() {
        let root = temp_dir("staging");
        let staging_path = root.join(".staging-test");
        {
            let _guard = StagingGuard::new(staging_path.clone()).expect("create staging");
            assert!(staging_path.exists());
        }
        assert!(!staging_path.exists(), "uncommitted staging is removed on drop");

        let guard = StagingGuard::new(staging_path.clone()).expect("create staging");
        guard.commit();
        assert!(staging_path.exists(), "committed staging survives drop");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
