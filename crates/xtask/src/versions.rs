//! `cargo xtask check-versions`: the crate version string has exactly one
//! source of truth — the root `Cargo.toml`'s `[workspace.package] version`
//! — and this verifies every other version reference in the repository
//! still matches it: the root workspace's `[workspace.dependencies]`
//! version entries, the examples workspace's own `[workspace.package]
//! version`, and every `rshooks*` dependency snippet or `rshooks-build`
//! builder-block version quoted in `book/src/**/*.md`, `docs/**/*.md`, and
//! `README.md`.
//!
//! Purely line-based text parsing (no toml/regex crate) — this only ever
//! reads back literal version strings this repo's own files already spell
//! out verbatim.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Repo root, resolved from this crate's own manifest directory
/// (`crates/xtask`, two levels below the workspace root), the same way as
/// `gen_core::repo_root`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// One version reference that disagrees with the source of truth.
struct Mismatch {
    /// Repo-root-relative path of the file the reference was found in.
    path: String,
    /// 1-based line number within that file.
    line: usize,
    /// The source-of-truth version every reference must match.
    expected: String,
    /// The version string actually found on that line.
    found: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: expected \"{}\", found \"{}\"",
            self.path, self.line, self.expected, self.found
        )
    }
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

/// `path`, rendered relative to the repo root for display (falls back to
/// the absolute path if it isn't rooted there).
fn relative(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// True when `s` is exactly three dot-separated ASCII-digit groups
/// (`"0.1.1"`) — the shape of a semver `MAJOR.MINOR.PATCH` string.
fn is_three_part_semver(s: &str) -> bool {
    let mut parts = s.split('.');
    let (Some(a), Some(b), Some(c), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    [a, b, c]
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Every quoted-string token on `line` (the content between each pair of
/// `"` characters), in order.
fn quoted_tokens(line: &str) -> Vec<&str> {
    line.split('"')
        .enumerate()
        .filter_map(|(i, s)| if i % 2 == 1 { Some(s) } else { None })
        .collect()
}

/// Every value assigned to a bare (unquoted) TOML `version = "..."` key on
/// `line`, skipping `rust-version` (and any other `*-version`/`*_version`
/// key) by requiring the character before `version` not be an identifier
/// character or `-`.
fn bare_key_versions(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(pos) = line[i..].find("version") {
        let start = i + pos;
        let end = start + "version".len();
        let before_ok = start == 0
            || !(bytes[start - 1].is_ascii_alphanumeric()
                || bytes[start - 1] == b'_'
                || bytes[start - 1] == b'-');
        if before_ok {
            let after = line[end..].trim_start();
            if let Some(rest) = after.strip_prefix('=')
                && let Some(v) = quoted_tokens(rest).first()
            {
                out.push((*v).to_string());
            }
        }
        i = end;
    }
    out
}

/// Every value assigned to a JSON `"version": "..."` key on `line`.
fn json_key_versions(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(pos) = line[i..].find("\"version\"") {
        let start = i + pos;
        let end = start + "\"version\"".len();
        let after = line[end..].trim_start();
        if let Some(rest) = after.strip_prefix(':')
            && let Some(v) = quoted_tokens(rest).first()
        {
            out.push((*v).to_string());
        }
        i = end;
    }
    out
}

/// Finds the `version = "..."` line inside a named TOML section (e.g.
/// `[workspace.package]`) in `content`, tracking `[section]` headers to
/// know when the target section has been left. Returns the 1-based line
/// number and the version string.
fn section_version(content: &str, section: &str) -> Option<(usize, String)> {
    let mut in_section = false;
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == section;
            continue;
        }
        if in_section && let Some(v) = bare_key_versions(line).into_iter().next() {
            return Some((idx + 1, v));
        }
    }
    None
}

/// Scans every line of `content` within `section` for `version = "..."`
/// dependency entries and records any that disagree with `source_version`.
fn collect_dependency_mismatches(
    content: &str,
    path_label: &str,
    section: &str,
    source_version: &str,
    mismatches: &mut Vec<Mismatch>,
) {
    let mut in_section = false;
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == section;
            continue;
        }
        if !in_section {
            continue;
        }
        for found in bare_key_versions(line) {
            if found != source_version {
                mismatches.push(Mismatch {
                    path: path_label.to_string(),
                    line: idx + 1,
                    expected: source_version.to_string(),
                    found,
                });
            }
        }
    }
}

/// Scans every line of a doc file for quoted 3-part semver tokens that are
/// expected to track `source_version`: a line is checked when it names a
/// `rshooks*` dependency (`rshooks = "0.1.1"`, `rshooks-testenv = { version
/// = "0.1.1", .. }`), or when it is the JSON `"version"` line of a
/// `"name": "rshooks-build"` builder block in a metadata sample. A hook
/// crate's own `version` (a `[package]` snippet, a sidecar's `"crate"`
/// version) is independent of the library version and is left alone, as
/// are lines mentioning `rustc`, `channel`, or `xahaud`.
fn collect_doc_mismatches(
    content: &str,
    path_label: &str,
    source_version: &str,
    mismatches: &mut Vec<Mismatch>,
) {
    let mut after_builder_name = false;
    for (idx, line) in content.lines().enumerate() {
        let follows_builder_name = after_builder_name;
        after_builder_name = line.contains("\"name\"") && line.contains("\"rshooks-build\"");
        if line.contains("rustc") || line.contains("channel") || line.contains("xahaud") {
            continue;
        }
        let applicable = line.contains("rshooks")
            || (follows_builder_name && !json_key_versions(line).is_empty());
        if !applicable {
            continue;
        }
        for token in quoted_tokens(line) {
            if is_three_part_semver(token) && token != source_version {
                mismatches.push(Mismatch {
                    path: path_label.to_string(),
                    line: idx + 1,
                    expected: source_version.to_string(),
                    found: token.to_string(),
                });
            }
        }
    }
}

/// Recursively collects every `*.md` file under `dir` into `out`. A
/// missing `dir` (e.g. an optional doc tree) contributes nothing.
fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("reading dir {}", dir.display()))?
        .collect::<std::io::Result<_>>()
        .with_context(|| format!("reading dir {}", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_md_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Every doc file the version check scans: `book/src/**/*.md`,
/// `docs/**/*.md`, and `README.md`.
fn doc_files() -> Result<Vec<PathBuf>> {
    let root = repo_root();
    let mut files = Vec::new();
    collect_md_files(&root.join("book/src"), &mut files)?;
    collect_md_files(&root.join("docs"), &mut files)?;
    let readme = root.join("README.md");
    if readme.is_file() {
        files.push(readme);
    }
    Ok(files)
}

/// `cargo xtask check-versions`: verifies every version reference in the
/// repository matches the root workspace's `[workspace.package] version`.
/// Collects every mismatch, prints each as `path:line: expected "X", found
/// "Y"` to stderr, and returns `Err` if any were found; otherwise prints a
/// one-line success summary and returns `Ok(())`.
pub fn run_check() -> Result<()> {
    let root = repo_root();

    let root_cargo_path = root.join("Cargo.toml");
    let root_cargo = read(&root_cargo_path)?;
    let (_, source_version) =
        section_version(&root_cargo, "[workspace.package]").with_context(|| {
            format!(
                "no [workspace.package] version in {}",
                root_cargo_path.display()
            )
        })?;

    let mut mismatches = Vec::new();

    collect_dependency_mismatches(
        &root_cargo,
        "Cargo.toml",
        "[workspace.dependencies]",
        &source_version,
        &mut mismatches,
    );

    let examples_cargo_path = root.join("examples/Cargo.toml");
    let examples_cargo = read(&examples_cargo_path)?;
    let (examples_line, examples_version) = section_version(&examples_cargo, "[workspace.package]")
        .with_context(|| {
            format!(
                "no [workspace.package] version in {}",
                examples_cargo_path.display()
            )
        })?;
    if examples_version != source_version {
        mismatches.push(Mismatch {
            path: "examples/Cargo.toml".to_string(),
            line: examples_line,
            expected: source_version.clone(),
            found: examples_version,
        });
    }

    for path in doc_files()? {
        let content = read(&path)?;
        collect_doc_mismatches(&content, &relative(&path), &source_version, &mut mismatches);
    }

    if mismatches.is_empty() {
        println!("versions: all references match {source_version}");
        return Ok(());
    }

    for mismatch in &mismatches {
        eprintln!("{mismatch}");
    }
    bail!(
        "check-versions: {} version reference(s) do not match {source_version}",
        mismatches.len()
    );
}
