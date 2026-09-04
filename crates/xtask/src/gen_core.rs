//! Orchestrates `cargo xtask gen-core`: reads the vendored xahaud headers
//! (`crates/rshooks-core/vendor/xahaud-hook/`), parses them once into a single
//! [`crate::ir::HookApiSpec`], round-trips that spec through
//! `crates/rshooks-core/hook_api.json`, runs each per-file generator in
//! [`crate::codegen`] against the round-tripped spec, formats the output
//! with `rustfmt` under the repo's `rustfmt.toml`, and either writes the
//! result into `crates/rshooks-core/` or (`--check`) compares it against
//! what's already there without touching the working tree.
//!
//! It does the same for the second vendor group and its own artifact: the
//! protocol format definitions in
//! `crates/rshooks-core/vendor/xahaud-protocol/` are parsed into a
//! [`crate::protocol_ir::ProtocolFormats`] and round-tripped through
//! `crates/rshooks-core/protocol_formats.json`, which the ledger-entry
//! generators then read the same way the header generators read
//! `hook_api.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::availability::FormatAvailability;
use crate::codegen;
use crate::ir::{self, HookApiSpec};
use crate::protocol_ir::{self, ProtocolFormats};

/// The set of `rshooks-core/src/`-relative `.rs` files this generator owns.
/// `lib.rs` is deliberately excluded (`docs/DESIGN.md` §4): it's hand-wired
/// module/re-export plumbing, not a header translation, and the spec calls
/// it out as NOT generated.
const GENERATED_FILES: &[&str] = &[
    "error.rs",
    "tts.rs",
    "lets.rs",
    "ls_flags.rs",
    "tx_flags.rs",
    "sfcodes.rs",
    "consts.rs",
    "api.rs",
    "host.rs",
];

/// The set of `rshooks/src/`-relative `.rs` files this generator owns —
/// disjoint from [`GENERATED_FILES`] (all `rshooks-core/src/`-relative):
/// [`codegen::tx_type`]'s typed `TxType` enum,
/// [`codegen::ledger_entry_type`]'s typed `LedgerEntryType` enum,
/// [`codegen::sfield`]'s typed `SField` constants, and [`codegen::views`]'s
/// three view modules.
///
/// The three `views/` entries are the only paths here with a directory
/// component; `views/mod.rs` and `views/source.rs` are hand-written and
/// deliberately absent, as `rshooks-core`'s `lib.rs` is absent from
/// [`GENERATED_FILES`].
const GENERATED_FILES_HOOKS_LIB: &[&str] = &[
    "sfield.rs",
    "tx_type.rs",
    "ledger_entry_type.rs",
    "views/tx.rs",
    "views/ledger.rs",
    "views/inner.rs",
];

/// The set of `rshooks-build/src/`-relative `.rs` files this generator owns:
/// [`codegen::tx_type_table`]'s build-side transaction-type name/code table,
/// generated from the same `tts.h` constants as
/// [`codegen::tx_type`]'s typed `TxType` enum so the two can never drift
/// apart.
const GENERATED_FILES_BUILD: &[&str] = &["tx_type_table.rs"];

/// The generated intermediate-representation file, checked in at the
/// `rshooks-core` crate root (not under `src/`, since it isn't Rust source):
/// the pipeline's `hook_api.json` artifact (module docs on [`crate::ir`]).
const HOOK_API_JSON: &str = "hook_api.json";

/// The second generated intermediate-representation file, checked in beside
/// [`HOOK_API_JSON`]: the protocol format artifact (module docs on
/// [`crate::protocol_ir`]).
const PROTOCOL_FORMATS_JSON: &str = "protocol_formats.json";

/// The curated availability classification checked in beside
/// [`PROTOCOL_FORMATS_JSON`]: which formats a hook may actually use on Xahau
/// (module docs on [`crate::availability`]). Unlike every other file this
/// module writes, it is **not** derived — `gen-core` only appends newly
/// declared formats as `dormant`; a human curates the rest.
const FORMAT_AVAILABILITY_JSON: &str = "format_availability.json";

/// Repo root, resolved from this crate's own manifest directory
/// (`crates/xtask`, two levels below the workspace root) at compile time —
/// this works regardless of the caller's current directory, since `cargo
/// xtask` (the `.cargo/config.toml` alias) is just `cargo run -p xtask`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn vendor_dir() -> PathBuf {
    repo_root().join("crates/rshooks-core/vendor/xahaud-hook")
}

/// The second vendor group's directory: xahaud's protocol format
/// definitions (`VENDOR.md` there).
fn protocol_vendor_dir() -> PathBuf {
    repo_root().join("crates/rshooks-core/vendor/xahaud-protocol")
}

/// `crates/rshooks-core`'s crate root — where `hook_api.json` lives, one level
/// above `src/`.
fn crate_dir() -> PathBuf {
    repo_root().join("crates/rshooks-core")
}

fn src_dir() -> PathBuf {
    crate_dir().join("src")
}

/// `crates/rshooks`'s `src/` directory — where [`GENERATED_FILES_HOOKS_LIB`]
/// lands (the generated files outside `rshooks-core`; see each generator's
/// own module doc comment for why).
fn rshooks_src_dir() -> PathBuf {
    repo_root().join("crates/rshooks/src")
}

/// `crates/rshooks-build`'s `src/` directory — where
/// [`GENERATED_FILES_BUILD`] lands.
fn rshooks_build_src_dir() -> PathBuf {
    repo_root().join("crates/rshooks-build/src")
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

/// Parses the eight vendored headers into a [`HookApiSpec`] and renders it
/// as pretty-printed, canonical JSON (trailing newline, no ambiguity in
/// key/array order — struct field order is derive-stable, and every
/// sequence here is already in header order).
fn build_hook_api_json() -> Result<String> {
    let vendor = vendor_dir();
    let error_h = read(&vendor.join("error.h"))?;
    let tts_h = read(&vendor.join("tts.h"))?;
    let ls_flags_h = read(&vendor.join("ls_flags.h"))?;
    let tx_flags_h = read(&vendor.join("tx_flags.h"))?;
    let sfcodes_h = read(&vendor.join("sfcodes.h"))?;
    let hookapi_h = read(&vendor.join("hookapi.h"))?;
    let macro_h = read(&vendor.join("macro.h"))?;
    let extern_h = read(&vendor.join("extern.h"))?;

    let spec = ir::build(
        &error_h,
        &tts_h,
        &ls_flags_h,
        &tx_flags_h,
        &sfcodes_h,
        &hookapi_h,
        &macro_h,
        &extern_h,
    )?;
    let mut json = serde_json::to_string_pretty(&spec).context("serializing HookApiSpec")?;
    json.push('\n');
    Ok(json)
}

/// Parses the six vendored protocol format definitions into a
/// [`ProtocolFormats`] and renders it as pretty-printed, canonical JSON
/// (trailing newline; struct field order is derive-stable and every sequence
/// is in file order, so the output is deterministic).
///
/// `hook_api_json` is the text [`build_hook_api_json`] just produced: the
/// `sfcodes.h` constants are read back out of it so
/// [`protocol_ir::build`]'s cross-validation gate compares against the
/// constants `rshooks-core` actually ships, not a second interpretation of
/// the header.
///
/// The result is deserialized back into a [`ProtocolFormats`] and
/// re-serialized here, so every `gen-core` run exercises the round trip a
/// later renderer depends on, not only tests.
fn build_protocol_formats_json(hook_api_json: &str) -> Result<String> {
    let vendor = protocol_vendor_dir();
    let sfields_macro = read(&vendor.join("sfields.macro"))?;
    let transactions_macro = read(&vendor.join("transactions.macro"))?;
    let ledger_entries_macro = read(&vendor.join("ledger_entries.macro"))?;
    let tx_formats_cpp = read(&vendor.join("TxFormats.cpp"))?;
    let ledger_formats_cpp = read(&vendor.join("LedgerFormats.cpp"))?;
    let inner_object_formats_cpp = read(&vendor.join("InnerObjectFormats.cpp"))?;

    let hook_api: HookApiSpec =
        serde_json::from_str(hook_api_json).context("deserializing hook_api.json")?;

    let formats = protocol_ir::build(
        &sfields_macro,
        &transactions_macro,
        &ledger_entries_macro,
        &tx_formats_cpp,
        &ledger_formats_cpp,
        &inner_object_formats_cpp,
        &hook_api.sfcodes,
    )?;

    let json = serde_json::to_string_pretty(&formats).context("serializing ProtocolFormats")?;
    let round_tripped: ProtocolFormats =
        serde_json::from_str(&json).context("deserializing protocol_formats.json")?;
    let mut again =
        serde_json::to_string_pretty(&round_tripped).context("re-serializing ProtocolFormats")?;
    if again != json {
        bail!("internal error: {PROTOCOL_FORMATS_JSON} does not round-trip byte-identically");
    }
    again.push('\n');
    Ok(again)
}

/// Reads the curated availability classification, or an empty one if the
/// file does not exist yet (the first `gen-core` run creates it).
fn read_format_availability() -> Result<FormatAvailability> {
    let path = crate_dir().join(FORMAT_AVAILABILITY_JSON);
    if !path.exists() {
        return Ok(FormatAvailability::empty());
    }
    let text = read(&path)?;
    serde_json::from_str(&text).with_context(|| format!("deserializing {}", path.display()))
}

/// Renders the classification back to canonical JSON (trailing newline,
/// key-sorted maps, so a re-tiering is a one-line diff).
fn render_format_availability(a: &FormatAvailability) -> Result<String> {
    let mut json = serde_json::to_string_pretty(a).context("serializing FormatAvailability")?;
    json.push('\n');
    Ok(json)
}

/// Generates every target `.rs` file's *unformatted* content, keyed by its
/// `src/`-relative filename, from the two artifact texts that get written to
/// (or checked against) `crates/rshooks-core/hook_api.json` and
/// `crates/rshooks-core/protocol_formats.json`. Both are deserialized back
/// here (rather than reusing the in-memory values that produced them) so
/// every generator consumes the intermediate representation, not the
/// parser's output directly.
fn generate_rust_files(
    hook_api_json: &str,
    protocol_formats_json: &str,
) -> Result<BTreeMap<&'static str, String>> {
    let spec: HookApiSpec =
        serde_json::from_str(hook_api_json).context("deserializing hook_api.json")?;
    let formats: ProtocolFormats = serde_json::from_str(protocol_formats_json)
        .context("deserializing protocol_formats.json")?;

    let mut out = BTreeMap::new();
    out.insert("error.rs", codegen::error::generate(&spec.error_codes)?);
    out.insert("tts.rs", codegen::tts::generate(&spec.tts)?);
    out.insert("lets.rs", codegen::lets::generate(&formats.ledger_entries)?);
    out.insert("ls_flags.rs", codegen::ls_flags::generate(&spec.ls_flags)?);
    out.insert("tx_flags.rs", codegen::tx_flags::generate(&spec.tx_flags)?);
    out.insert("sfcodes.rs", codegen::sfcodes::generate(&spec.sfcodes)?);
    out.insert(
        "consts.rs",
        codegen::consts::generate(
            &spec.keylet,
            &spec.compare,
            &spec.canonical,
            &spec.at_family,
            &spec.am_family,
        )?,
    );
    out.insert("api.rs", codegen::api::generate(&spec.functions)?);
    out.insert("host.rs", codegen::host::generate(&spec.functions)?);

    for name in GENERATED_FILES {
        if !out.contains_key(name) {
            bail!("internal error: generator produced no content for {name}");
        }
    }
    Ok(out)
}

/// Generates every `rshooks`-targeted file's *unformatted* content, keyed
/// by its `rshooks/src/`-relative filename, from the same two artifact texts
/// [`generate_rust_files`] consumes — [`codegen::sfield`]'s `sfield.rs`,
/// [`codegen::tx_type`]'s `tx_type.rs`,
/// [`codegen::ledger_entry_type`]'s `ledger_entry_type.rs` and
/// [`codegen::views`]'s three `views/*.rs` modules.
fn generate_rshooks_files(
    hook_api_json: &str,
    protocol_formats_json: &str,
    availability: &FormatAvailability,
) -> Result<BTreeMap<&'static str, String>> {
    let spec: HookApiSpec =
        serde_json::from_str(hook_api_json).context("deserializing hook_api.json")?;
    let formats: ProtocolFormats = serde_json::from_str(protocol_formats_json)
        .context("deserializing protocol_formats.json")?;

    let mut out = BTreeMap::new();
    // `sfield.rs` follows availability (a dormant constant isn't rendered, a
    // pending one is feature-gated); `sfcodes.rs` stays a complete 1:1
    // mirror — see `codegen::sfield`'s module docs for why.
    out.insert(
        "sfield.rs",
        codegen::sfield::generate(&spec.sfcodes, &availability.field_tiers(&formats))?,
    );
    out.insert("tx_type.rs", codegen::tx_type::generate(&spec.tts)?);
    out.insert(
        "ledger_entry_type.rs",
        codegen::ledger_entry_type::generate(&formats.ledger_entries)?,
    );
    out.insert(
        "views/tx.rs",
        codegen::views::generate_tx(&formats, availability)?,
    );
    out.insert(
        "views/ledger.rs",
        codegen::views::generate_ledger(&formats, availability)?,
    );
    out.insert(
        "views/inner.rs",
        codegen::views::generate_inner(&formats, availability)?,
    );

    for name in GENERATED_FILES_HOOKS_LIB {
        if !out.contains_key(name) {
            bail!("internal error: generator produced no content for {name}");
        }
    }
    Ok(out)
}

/// Generates every `rshooks-build`-targeted file's *unformatted* content,
/// keyed by its `rshooks-build/src/`-relative filename —
/// [`codegen::tx_type_table`]'s build-side transaction-type name/code
/// table, derived from the same `hook_api.json` artifact as
/// [`generate_rshooks_files`]'s `tx_type.rs`.
fn generate_build_files(hook_api_json: &str) -> Result<BTreeMap<&'static str, String>> {
    let spec: HookApiSpec =
        serde_json::from_str(hook_api_json).context("deserializing hook_api.json")?;

    let mut out = BTreeMap::new();
    out.insert(
        "tx_type_table.rs",
        codegen::tx_type_table::generate(&spec.tts)?,
    );

    for name in GENERATED_FILES_BUILD {
        if !out.contains_key(name) {
            bail!("internal error: generator produced no content for {name}");
        }
    }
    Ok(out)
}

/// A scratch directory, auto-removed on drop, carrying a copy of the repo's
/// `rustfmt.toml` so `rustfmt` (run directly, not through `cargo fmt`)
/// discovers the same style config it would inside the real tree.
struct FmtScratch(PathBuf);

impl FmtScratch {
    fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "xtask-gen-core-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let rustfmt_toml = repo_root().join("rustfmt.toml");
        fs::copy(&rustfmt_toml, dir.join("rustfmt.toml"))
            .with_context(|| format!("copying {}", rustfmt_toml.display()))?;
        Ok(Self(dir))
    }

    /// Writes `content` under `filename` in the scratch dir and runs
    /// `rustfmt` on it in place, returning the formatted text.
    ///
    /// The scratch dir is flat: a `filename` carrying a directory component
    /// (`views/tx.rs`) is flattened to `views_tx.rs` instead of creating the
    /// directory, since `rustfmt` cares only about the extension and the
    /// `--edition` flag; flattening keeps names unique since the target
    /// lists are themselves unique paths.
    fn format(&self, filename: &str, content: &str) -> Result<String> {
        let path = self.0.join(filename.replace('/', "_"));
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;

        // `unsafe extern "C" { ... }` (in api.rs) requires 2024-edition
        // parsing; standalone `rustfmt` can't infer that from a Cargo.toml.
        let status = Command::new("rustfmt")
            .args(["--edition", "2024"])
            .arg(&path)
            .status()
            .context("running rustfmt (is it installed? `rustup component add rustfmt`)")?;
        if !status.success() {
            bail!("rustfmt exited with failure formatting {filename}");
        }
        read(&path)
    }
}

impl Drop for FmtScratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn format_all(
    generated: &BTreeMap<&'static str, String>,
) -> Result<BTreeMap<&'static str, String>> {
    let scratch = FmtScratch::new()?;
    let mut formatted = BTreeMap::new();
    for (name, content) in generated {
        formatted.insert(*name, scratch.format(name, content)?);
    }
    Ok(formatted)
}

/// `cargo xtask gen-core`: writes `hook_api.json`, then the generated +
/// `rustfmt`-formatted `.rs` files, into `crates/rshooks-core/`, (for
/// [`codegen::sfield`]'s and [`codegen::tx_type`]'s output) `crates/rshooks/`,
/// and (for [`codegen::tx_type_table`]'s output) `crates/rshooks-build/`,
/// then runs `cargo fmt -p rshooks-core -p rshooks -p rshooks-build` as a
/// belt-and-braces final pass over the real files.
pub fn run_update() -> Result<()> {
    let hook_api_json = build_hook_api_json()?;
    let protocol_formats_json = build_protocol_formats_json(&hook_api_json)?;

    // The one automatic edit this file gets: an unclassified format is
    // appended as `dormant` (see `crate::availability` module docs).
    let formats: ProtocolFormats = serde_json::from_str(&protocol_formats_json)
        .context("deserializing protocol_formats.json")?;
    let mut availability = read_format_availability()?;
    let added = availability.auto_add(&formats);
    availability.refresh_doc();
    availability.validate(&formats)?;
    let availability_json = render_format_availability(&availability)?;

    let generated = generate_rust_files(&hook_api_json, &protocol_formats_json)?;
    let formatted = format_all(&generated)?;
    let generated_rshooks =
        generate_rshooks_files(&hook_api_json, &protocol_formats_json, &availability)?;
    let formatted_rshooks = format_all(&generated_rshooks)?;
    let generated_build = generate_build_files(&hook_api_json)?;
    let formatted_build = format_all(&generated_build)?;

    let json_path = crate_dir().join(HOOK_API_JSON);
    fs::write(&json_path, &hook_api_json)
        .with_context(|| format!("writing {}", json_path.display()))?;
    println!("wrote {}", json_path.display());

    let protocol_json_path = crate_dir().join(PROTOCOL_FORMATS_JSON);
    fs::write(&protocol_json_path, &protocol_formats_json)
        .with_context(|| format!("writing {}", protocol_json_path.display()))?;
    println!("wrote {}", protocol_json_path.display());

    let availability_path = crate_dir().join(FORMAT_AVAILABILITY_JSON);
    fs::write(&availability_path, &availability_json)
        .with_context(|| format!("writing {}", availability_path.display()))?;
    println!("wrote {}", availability_path.display());
    for name in &added {
        println!("  classified {name} as `dormant` (newly declared upstream)");
    }
    if !added.is_empty() {
        println!(
            "  {} format(s) added as `dormant`; move any that should be usable to \
             `pending` or `active` in {FORMAT_AVAILABILITY_JSON}",
            added.len()
        );
    }

    let dir = src_dir();
    for (name, content) in &formatted {
        let path = dir.join(name);
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        println!("wrote {}", path.display());
    }

    let rshooks_dir = rshooks_src_dir();
    for (name, content) in &formatted_rshooks {
        let path = rshooks_dir.join(name);
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        println!("wrote {}", path.display());
    }

    let rshooks_build_dir = rshooks_build_src_dir();
    for (name, content) in &formatted_build {
        let path = rshooks_build_dir.join(name);
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        println!("wrote {}", path.display());
    }

    let status = Command::new("cargo")
        .args([
            "fmt",
            "-p",
            "rshooks-core",
            "-p",
            "rshooks",
            "-p",
            "rshooks-build",
        ])
        .current_dir(repo_root())
        .status()
        .context("running `cargo fmt -p rshooks-core -p rshooks -p rshooks-build`")?;
    if !status.success() {
        bail!("`cargo fmt -p rshooks-core -p rshooks -p rshooks-build` failed");
    }
    Ok(())
}

/// `cargo xtask gen-core --check`: regenerates `hook_api.json` and formats
/// the `.rs` files in a scratch directory, then byte-compares both against
/// `crates/rshooks-core/hook_api.json`, `crates/rshooks-core/src/*.rs`,
/// [`codegen::sfield`]'s and [`codegen::tx_type`]'s `crates/rshooks/src/`
/// output, and [`codegen::tx_type_table`]'s
/// `crates/rshooks-build/src/tx_type_table.rs` output, without writing
/// anything there. Returns an error naming every mismatched file if any
/// differ (the CI-facing exit-1 path); prints a confirmation and returns
/// `Ok(())` when everything matches.
pub fn run_check() -> Result<()> {
    let hook_api_json = build_hook_api_json()?;
    let protocol_formats_json = build_protocol_formats_json(&hook_api_json)?;

    // Unlike the derived artifacts, a stale classification is an *error*,
    // not a diff to regenerate: only a human can decide a tier.
    let formats: ProtocolFormats = serde_json::from_str(&protocol_formats_json)
        .context("deserializing protocol_formats.json")?;
    let mut availability = read_format_availability()?;
    availability.validate(&formats)?;
    availability.refresh_doc();
    let availability_json = render_format_availability(&availability)?;

    let generated = generate_rust_files(&hook_api_json, &protocol_formats_json)?;
    let formatted = format_all(&generated)?;
    let generated_rshooks =
        generate_rshooks_files(&hook_api_json, &protocol_formats_json, &availability)?;
    let formatted_rshooks = format_all(&generated_rshooks)?;
    let generated_build = generate_build_files(&hook_api_json)?;
    let formatted_build = format_all(&generated_build)?;

    let mut mismatched = Vec::new();

    let json_on_disk = read(&crate_dir().join(HOOK_API_JSON)).unwrap_or_default();
    if hook_api_json != json_on_disk {
        mismatched.push(HOOK_API_JSON);
    }

    // `unwrap_or_default` makes a missing artifact a mismatch, not an I/O
    // error: "not generated yet" and "generated but stale" are the same
    // failure to a CI job. Formatting drift in the curated file counts the
    // same way, since `gen-core` rewrites it canonically.
    let availability_on_disk =
        read(&crate_dir().join(FORMAT_AVAILABILITY_JSON)).unwrap_or_default();
    if availability_json != availability_on_disk {
        mismatched.push(FORMAT_AVAILABILITY_JSON);
    }

    let protocol_json_on_disk = read(&crate_dir().join(PROTOCOL_FORMATS_JSON)).unwrap_or_default();
    if protocol_formats_json != protocol_json_on_disk {
        mismatched.push(PROTOCOL_FORMATS_JSON);
    }

    let dir = src_dir();
    for (name, content) in &formatted {
        let on_disk = read(&dir.join(name)).unwrap_or_default();
        if *content != on_disk {
            mismatched.push(*name);
        }
    }

    let rshooks_dir = rshooks_src_dir();
    for (name, content) in &formatted_rshooks {
        let on_disk = read(&rshooks_dir.join(name)).unwrap_or_default();
        if *content != on_disk {
            mismatched.push(*name);
        }
    }

    let rshooks_build_dir = rshooks_build_src_dir();
    for (name, content) in &formatted_build {
        let on_disk = read(&rshooks_build_dir.join(name)).unwrap_or_default();
        if *content != on_disk {
            mismatched.push(*name);
        }
    }

    if mismatched.is_empty() {
        println!(
            "cargo xtask gen-core --check: crates/rshooks-core/hook_api.json, crates/rshooks-core/protocol_formats.json, crates/rshooks-core/src/*.rs, crates/rshooks/src/sfield.rs + tx_type.rs + ledger_entry_type.rs + views/{{tx,ledger,inner}}.rs, and crates/rshooks-build/src/tx_type_table.rs are up to date"
        );
        Ok(())
    } else {
        bail!(
            "cargo xtask gen-core --check: out of date: {}\n\
             run `cargo xtask gen-core` and commit the result",
            mismatched.join(", ")
        );
    }
}
