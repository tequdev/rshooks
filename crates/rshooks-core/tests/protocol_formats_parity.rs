//! Parity test: `vendor/xahaud-protocol/*.macro` vs the generated
//! `protocol_formats.json` artifact (`crates/xtask/src/protocol_ir.rs`).
//!
//! The parser below is deliberately independent of `xtask`'s, for the same
//! reason every other parity test in this directory keeps its own: `xtask`'s
//! parser is the thing under test, and a bug in a parser shared with it would
//! be invisible here. It is also independent *in technique* — a line-oriented
//! state machine rather than a comment-stripped token stream with
//! balanced-delimiter scanning — so the two are unlikely to be wrong the same
//! way. It is strict: anything it does not recognize panics rather than being
//! skipped, so a shape change upstream fails this test instead of quietly
//! shrinking its coverage.
//!
//! `serde_json` reads the artifact. That is not a second parse of the
//! upstream sources — it is reading the generator's output in its own
//! declared format — so it does not compromise the independence above.
//!
//! Test code is exempt from the workspace's panic-freedom lints (per
//! `docs/DESIGN.md` §8).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use serde_json::Value;

const TRANSACTIONS: &str = include_str!("../vendor/xahaud-protocol/transactions.macro");
const LEDGER_ENTRIES: &str = include_str!("../vendor/xahaud-protocol/ledger_entries.macro");
const SFIELDS: &str = include_str!("../vendor/xahaud-protocol/sfields.macro");
const ARTIFACT: &str = include_str!("../protocol_formats.json");

/// One format as this file's own parser sees it: the header arguments before
/// the field list, then `{sfX, soeY, ...}` entries flattened to
/// `["sfX", "soeY", ...]`.
#[derive(Debug, PartialEq, Eq)]
struct Format {
    head: Vec<String>,
    fields: Vec<Vec<String>>,
}

/// Returns each source line with `//` and `/* */` comments removed. Block
/// comments are the doc comments above every format; line comments annotate
/// individual field entries.
fn uncommented(src: &str) -> Vec<String> {
    let mut in_block = false;
    let mut lines = Vec::new();
    for raw in src.lines() {
        let mut out = String::new();
        let mut rest = raw;
        loop {
            if in_block {
                match rest.find("*/") {
                    Some(end) => {
                        in_block = false;
                        rest = &rest[end + 2..];
                    }
                    None => break,
                }
                continue;
            }
            let line_at = rest.find("//");
            let block_at = rest.find("/*");
            match (line_at, block_at) {
                (Some(l), None) => {
                    out.push_str(&rest[..l]);
                    break;
                }
                (Some(l), Some(b)) if l < b => {
                    out.push_str(&rest[..l]);
                    break;
                }
                (_, Some(b)) => {
                    out.push_str(&rest[..b]);
                    in_block = true;
                    rest = &rest[b + 2..];
                }
                (None, None) => {
                    out.push_str(rest);
                    break;
                }
            }
        }
        lines.push(out.trim().to_string());
    }
    lines
}

/// Splits a comma-separated argument list, trimming each part.
fn split_args(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Parses one `{sfX, soeY[, soeZ]}` field line, with or without a trailing
/// comma.
fn parse_field_line(line: &str) -> Vec<String> {
    let body = line
        .trim_end_matches(',')
        .trim()
        .strip_prefix('{')
        .unwrap_or_else(|| panic!("field line does not start with `{{`: {line:?}"))
        .strip_suffix('}')
        .unwrap_or_else(|| panic!("field line does not end with `}}`: {line:?}"));
    let parts = split_args(body);
    assert!(
        parts.len() >= 2,
        "field line {line:?} has fewer than two tokens"
    );
    assert!(parts[0].starts_with("sf"), "{line:?}");
    assert!(parts[1].starts_with("soe"), "{line:?}");
    parts
}

/// Line-oriented scan for `head_count`-argument invocations of any macro in
/// `names`, each followed by a `({` … `}))` field list (or the single-line
/// `({})` empty form).
fn scan_formats(src: &str, names: &[&str], head_count: usize) -> Vec<Format> {
    let mut out: Vec<Format> = Vec::new();
    let mut open: Option<Format> = None;

    for line in uncommented(src) {
        if line.is_empty() {
            continue;
        }
        if let Some(current) = open.as_mut() {
            if line == "}))" {
                out.push(open.take().expect("a format is open"));
            } else {
                current.fields.push(parse_field_line(&line));
            }
            continue;
        }
        // Directives and the `#ifndef` block's macro definitions.
        if line.starts_with('#') {
            continue;
        }
        let Some(name) = names.iter().find(|n| line.starts_with(&format!("{n}("))) else {
            panic!("unrecognized line outside a field list: {line:?}");
        };
        let args = &line[name.len() + 1..];
        let (head_text, tail) = {
            let mut split_at = None;
            let mut seen = 0usize;
            for (i, c) in args.char_indices() {
                if c == ',' {
                    seen += 1;
                    if seen == head_count {
                        split_at = Some(i);
                        break;
                    }
                }
            }
            let at = split_at.unwrap_or_else(|| panic!("too few arguments in {line:?}"));
            (&args[..at], args[at + 1..].trim())
        };
        let mut head = split_args(head_text);
        head.insert(0, (*name).to_string());
        assert_eq!(head.len(), head_count + 1, "{line:?}");

        match tail {
            "({" => {
                open = Some(Format {
                    head,
                    fields: Vec::new(),
                })
            }
            "({}))" => out.push(Format {
                head,
                fields: Vec::new(),
            }),
            other => panic!("unrecognized field-list opener {other:?} in {line:?}"),
        }
    }
    assert!(open.is_none(), "unterminated field list");
    out
}

/// Normalizes a type value written as decimal, hex, or a character literal.
fn value_of(text: &str) -> u64 {
    if let Some(body) = text.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        let mut chars = body.chars();
        let c = chars.next().expect("empty character literal");
        assert!(chars.next().is_none() && c.is_ascii(), "{text:?}");
        return u64::from(c as u32);
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).unwrap_or_else(|e| panic!("{text:?}: {e}"));
    }
    text.parse().unwrap_or_else(|e| panic!("{text:?}: {e}"))
}

fn artifact() -> Value {
    serde_json::from_str(ARTIFACT).expect("parsing protocol_formats.json")
}

fn array<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("protocol_formats.json has no array `{key}`"))
}

/// The artifact's field list, flattened to the same
/// `["sfX", "soeY", ...]` shape [`parse_field_line`] produces.
fn artifact_fields(format: &Value) -> Vec<Vec<String>> {
    array(format, "fields")
        .iter()
        .map(|f| {
            let presence = match f["presence"].as_str().expect("presence") {
                "required" => "soeREQUIRED",
                "optional" => "soeOPTIONAL",
                "default" => "soeDEFAULT",
                other => panic!("unknown presence {other:?}"),
            };
            let mut out = vec![
                f["sfield"].as_str().expect("sfield").to_string(),
                presence.to_string(),
            ];
            for extra in array(f, "extras") {
                out.push(extra.as_str().expect("extra").to_string());
            }
            out
        })
        .collect()
}

#[test]
fn transactions_macro_matches_the_artifact() {
    let parsed = scan_formats(TRANSACTIONS, &["TRANSACTION"], 3);
    let artifact = artifact();
    let generated = array(&artifact, "transactions");

    assert_eq!(
        parsed.len(),
        generated.len(),
        "transactions.macro declares {} formats, protocol_formats.json has {}",
        parsed.len(),
        generated.len()
    );
    assert!(parsed.len() >= 70, "only {} transactions", parsed.len());

    for (mine, theirs) in parsed.iter().zip(generated) {
        assert_eq!(mine.head[1], theirs["tag"].as_str().expect("tag"));
        assert_eq!(
            value_of(&mine.head[2]),
            theirs["value"].as_u64().expect("value")
        );
        assert_eq!(mine.head[3], theirs["name"].as_str().expect("name"));
        assert_eq!(
            mine.fields,
            artifact_fields(theirs),
            "field list of {}",
            mine.head[1]
        );
    }
}

#[test]
fn ledger_entries_macro_matches_the_artifact() {
    let parsed = scan_formats(
        LEDGER_ENTRIES,
        &["LEDGER_ENTRY_DUPLICATE", "LEDGER_ENTRY"],
        4,
    );
    let artifact = artifact();
    let generated = array(&artifact, "ledger_entries");

    assert_eq!(
        parsed.len(),
        generated.len(),
        "ledger_entries.macro declares {} formats, protocol_formats.json has {}",
        parsed.len(),
        generated.len()
    );
    assert!(parsed.len() >= 30, "only {} ledger entries", parsed.len());

    for (mine, theirs) in parsed.iter().zip(generated) {
        assert_eq!(mine.head[1], theirs["tag"].as_str().expect("tag"));
        assert_eq!(
            value_of(&mine.head[2]),
            theirs["value"].as_u64().expect("value")
        );
        assert_eq!(mine.head[3], theirs["name"].as_str().expect("name"));
        assert_eq!(mine.head[4], theirs["rpc_name"].as_str().expect("rpc_name"));
        assert_eq!(
            mine.head[0] == "LEDGER_ENTRY_DUPLICATE",
            theirs["duplicate"].as_bool().expect("duplicate"),
            "duplicate flag of {}",
            mine.head[1]
        );
        assert_eq!(
            mine.fields,
            artifact_fields(theirs),
            "field list of {}",
            mine.head[1]
        );
    }
}

#[test]
fn sfields_macro_matches_the_artifact() {
    let mut parsed: Vec<(String, String, u64)> = Vec::new();
    for line in uncommented(SFIELDS) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let body = line
            .strip_prefix("TYPED_SFIELD(")
            .or_else(|| line.strip_prefix("UNTYPED_SFIELD("))
            .unwrap_or_else(|| panic!("unrecognized line in sfields.macro: {line:?}"));
        let args = split_args(body.strip_suffix(')').unwrap_or_else(|| panic!("{line:?}")));
        assert!(args.len() >= 3, "{line:?}");
        parsed.push((args[0].clone(), args[1].clone(), value_of(&args[2])));
    }

    let artifact = artifact();
    let generated = array(&artifact, "sfields");
    assert_eq!(parsed.len(), generated.len());
    assert!(parsed.len() >= 325, "only {} sfields", parsed.len());

    for ((name, sti, code), theirs) in parsed.iter().zip(generated) {
        assert_eq!(name, theirs["name"].as_str().expect("name"));
        assert_eq!(sti, theirs["sti"].as_str().expect("sti"));
        assert_eq!(*code, theirs["field_code"].as_u64().expect("field_code"));
        // The packed code the artifact promises is derivable by sorting.
        assert_eq!(
            theirs["code"].as_u64().expect("code"),
            (theirs["sti_code"].as_u64().expect("sti_code") << 16) | code
        );
    }
}

#[test]
fn known_formats_read_the_way_the_upstream_sources_do() {
    let artifact = artifact();

    let find = |key: &str, id_key: &str, id: &str| {
        array(&artifact, key)
            .iter()
            .find(|f| f[id_key].as_str() == Some(id))
            .unwrap_or_else(|| panic!("no {key} entry {id}"))
            .clone()
    };

    assert_eq!(artifact["version"].as_u64(), Some(1));

    // A transaction with a `soeMPTSupported` extra and a `soeDEFAULT` field.
    let payment = find("transactions", "tag", "ttPAYMENT");
    assert_eq!(payment["value"].as_u64(), Some(0));
    assert_eq!(
        artifact_fields(&payment)[1],
        vec!["sfAmount", "soeREQUIRED", "soeMPTSupported"]
    );
    assert!(
        artifact_fields(&payment)
            .iter()
            .any(|f| f[0] == "sfPaths" && f[1] == "soeDEFAULT")
    );

    // The empty field list.
    assert!(
        artifact_fields(&find("transactions", "tag", "ttDID_DELETE")).is_empty(),
        "DIDDelete declares ({{}})"
    );

    // A character-literal ledger entry value, and a hex one.
    assert_eq!(
        find("ledger_entries", "tag", "ltHOOK")["value"].as_u64(),
        Some(0x48)
    );
    assert_eq!(
        find("ledger_entries", "tag", "ltRIPPLE_STATE")["value"].as_u64(),
        Some(0x0072)
    );

    // The common-field lists, which live in the two .cpp files rather than
    // in the .macro files this file's parser reads.
    let tx_common: Vec<String> = array(&artifact, "tx_common")
        .iter()
        .map(|f| f["sfield"].as_str().expect("sfield").to_string())
        .collect();
    assert_eq!(
        tx_common.first().map(String::as_str),
        Some("sfTransactionType")
    );
    assert!(tx_common.iter().any(|f| f == "sfEmitDetails"));
    let le_common: Vec<String> = array(&artifact, "le_common")
        .iter()
        .map(|f| f["sfield"].as_str().expect("sfield").to_string())
        .collect();
    assert_eq!(
        le_common,
        vec!["sfLedgerIndex", "sfLedgerEntryType", "sfFlags", "sfRemarks"]
    );

    // An inner object whose `add(...)` call uses the compact `{{...}}` form.
    let emit = array(&artifact, "inner_objects")
        .iter()
        .find(|i| i["sfield"].as_str() == Some("sfEmitDetails"))
        .expect("no sfEmitDetails inner object");
    assert_eq!(
        artifact_fields(emit).first().map(Vec::as_slice),
        Some(["sfEmitGeneration".to_string(), "soeREQUIRED".to_string()].as_slice())
    );
}

#[test]
fn every_referenced_field_resolves_in_the_sfields_table() {
    let artifact = artifact();
    let known: std::collections::BTreeSet<&str> = array(&artifact, "sfields")
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();

    let mut checked = 0usize;
    for key in ["transactions", "ledger_entries", "inner_objects"] {
        for format in array(&artifact, key) {
            for field in array(format, "fields") {
                let name = field["sfield"].as_str().expect("sfield");
                assert!(known.contains(name), "{key}: unknown field {name}");
                checked += 1;
            }
        }
    }
    for key in ["tx_common", "le_common"] {
        for field in array(&artifact, key) {
            let name = field["sfield"].as_str().expect("sfield");
            assert!(known.contains(name), "{key}: unknown field {name}");
            checked += 1;
        }
    }
    assert!(checked > 500, "only {checked} field references checked");
}
