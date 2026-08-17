//! Builds the per-entry Hook metadata sidecar
//! (`<index>.<hook_fn>.metadata.json`) for the v2 chain build.
//!
//! Reuses `metadata`'s shared fields (`WorstCaseExecution`, `BuilderInfo`,
//! `hook_hash`, `hook_mask`, `utf8_hex`), extended with entry identity
//! (`index`/`hook_fn`/`cbak_fn`) and a transcribed `chain` summary.

use anyhow::{Context, Result};
use serde::{Serialize, Serializer, ser::SerializeMap};

use crate::ValidationReport;
use crate::carriers::{ChainCarrier, ChainDecls, EntryDecl, resolve_trigger_masks};
use crate::metadata::{BuilderInfo, WorstCaseExecution, hook_hash, hook_mask, utf8_hex};

/// The built sidecar bytes plus non-fatal warnings (currently: `HookName`
/// byte length outside the protocol's 4..=16 byte window — the 2..=8
/// *character* rule is a hard error, already enforced at carrier
/// validation time).
pub struct EntrySidecarBuild {
    /// Pretty-printed JSON bytes, with a trailing newline.
    pub bytes: Vec<u8>,
    /// Non-fatal warnings to surface to the user.
    pub warnings: Vec<String>,
}

/// Builds one entry's sidecar document from its carrier declaration, the
/// shared chain summary, and the final (post-pipeline) wasm bytes/report.
/// `rustc` is the already-detected `rustc -V` first line (`None` if
/// detection failed or wasn't attempted).
pub fn build_entry_sidecar(
    entry: &EntryDecl,
    chain: &ChainCarrier,
    final_wasm: &[u8],
    report: &ValidationReport,
    rustc: Option<String>,
) -> Result<EntrySidecarBuild> {
    let mut warnings = Vec::new();
    if let Some(name) = &entry.hook_name {
        let byte_count = name.len();
        if !(4..=16).contains(&byte_count) {
            warnings.push(format!(
                "entry {} (`{}`): HookName is {byte_count} UTF-8 bytes, but the current Xahau \
                 protocol requires 4 to 16 bytes",
                entry.index, entry.hook_fn
            ));
        }
    }

    let wce = report.guard_verdict.map_or(
        WorstCaseExecution {
            hook: None,
            cbak: None,
        },
        |verdict| WorstCaseExecution {
            hook: Some(verdict.hook_cost),
            cbak: Some(verdict.cbak_cost),
        },
    );

    let document = EntrySidecarDocument {
        entry: entry.clone(),
        chain: chain.clone(),
        hook_hash: hook_hash(final_wasm),
        wce,
        builder: BuilderInfo::current(rustc),
    };

    let mut bytes =
        serde_json::to_vec_pretty(&document).context("serializing entry Hook metadata sidecar")?;
    bytes.push(b'\n');

    Ok(EntrySidecarBuild { bytes, warnings })
}

struct EntrySidecarDocument {
    entry: EntryDecl,
    chain: ChainCarrier,
    hook_hash: String,
    wce: WorstCaseExecution,
    builder: BuilderInfo,
}

impl Serialize for EntrySidecarDocument {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("index", &self.entry.index)?;
        map.serialize_entry("hook_fn", &self.entry.hook_fn)?;
        map.serialize_entry("cbak_fn", &self.entry.cbak_fn)?;
        // `name` mirrors `hook_fn` per contract §C item 5 ("name := hook_fn").
        map.serialize_entry("name", &self.entry.hook_fn)?;
        map.serialize_entry("description", &self.entry.description)?;

        let masks = resolve_trigger_masks(&self.entry.on).map_err(serde::ser::Error::custom)?;
        if self.entry.on.form == "directional" {
            map.serialize_entry("HookOnIncoming", &masks.hook_on_incoming)?;
            map.serialize_entry("HookOnOutgoing", &masks.hook_on_outgoing)?;
        } else {
            map.serialize_entry("HookOn", &masks.hook_on)?;
        }

        map.serialize_entry(
            "HookCanEmit",
            &hook_mask(self.entry.hook_can_emit.as_deref()).map_err(serde::ser::Error::custom)?,
        )?;
        map.serialize_entry(
            "HookName",
            &self.entry.hook_name.as_deref().map(utf8_hex),
        )?;
        map.serialize_entry("HookHash", &self.hook_hash)?;
        map.serialize_entry("WCE", &self.wce)?;
        map.serialize_entry("builder", &self.builder)?;
        map.serialize_entry(
            "human",
            &HumanEntry {
                on: &self.entry.on,
                hook_can_emit: self.entry.hook_can_emit.as_deref(),
                hook_name: self.entry.hook_name.as_deref(),
            },
        )?;
        map.serialize_entry(
            "chain",
            &ChainSummary {
                struct_name: &self.chain.struct_name,
                description: self.chain.description.as_deref(),
                decls: &self.chain.decls,
            },
        )?;
        map.end()
    }
}

/// Readable source values corresponding to the raw SetHook fields above.
#[derive(Serialize)]
struct HumanEntry<'a> {
    on: &'a crate::carriers::OnDecl,
    #[serde(rename = "HookCanEmit")]
    hook_can_emit: Option<&'a [String]>,
    #[serde(rename = "HookName")]
    hook_name: Option<&'a str>,
}

/// The shared struct-level schema, transcribed into every entry's sidecar
/// (this is intentionally NOT per-entry usage information — see contract
/// §C item 5).
#[derive(Serialize)]
struct ChainSummary<'a> {
    #[serde(rename = "struct")]
    struct_name: &'a str,
    description: Option<&'a str>,
    decls: &'a ChainDecls,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::carriers::{ChainDecls, OnDecl};

    fn chain() -> ChainCarrier {
        ChainCarrier {
            schema: "rshooks-chain-v2".to_string(),
            struct_name: "Vault".to_string(),
            description: Some("test chain".to_string()),
            decls: ChainDecls::default(),
        }
    }

    fn entry(on: OnDecl) -> EntryDecl {
        EntryDecl {
            index: 0,
            hook_fn: "deposit".to_string(),
            cbak_fn: Some("deposit_cbak".to_string()),
            hook_name: Some("dep".to_string()),
            on,
            hook_can_emit: Some(vec!["Payment".to_string()]),
            description: Some("records a deposit".to_string()),
        }
    }

    fn omitted_on() -> OnDecl {
        OnDecl {
            form: "omitted".to_string(),
            hook_on: None,
            hook_on_incoming: None,
            hook_on_outgoing: None,
        }
    }

    #[test]
    fn sidecar_schema_carries_identity_chain_and_mapped_metadata_fields() {
        let report = ValidationReport::default();
        let built = build_entry_sidecar(&entry(omitted_on()), &chain(), b"AAAA", &report, None)
            .expect("sidecar builds");
        let value: serde_json::Value = serde_json::from_slice(&built.bytes).expect("valid json");

        assert_eq!(value["index"], 0);
        assert_eq!(value["hook_fn"], "deposit");
        assert_eq!(value["cbak_fn"], "deposit_cbak");
        assert_eq!(value["name"], "deposit");
        assert_eq!(value["description"], "records a deposit");
        assert!(value["HookOn"].is_null());
        assert!(value.get("HookOnIncoming").is_none());
        assert_eq!(value["HookCanEmit"], "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFBFFFFE");
        assert_eq!(value["HookName"], "646570");
        assert!(
            value["HookHash"]
                .as_str()
                .is_some_and(|s| s.len() == 64)
        );
        assert_eq!(value["chain"]["struct"], "Vault");
        assert_eq!(value["chain"]["description"], "test chain");
        assert!(value["chain"]["decls"]["state"].as_array().expect("state array").is_empty());
        assert_eq!(value["human"]["HookCanEmit"][0], "Payment");
        assert_eq!(value["human"]["HookName"], "dep");
    }

    #[test]
    fn directional_form_emits_incoming_outgoing_not_hook_on() {
        let on = OnDecl {
            form: "directional".to_string(),
            hook_on: None,
            hook_on_incoming: Some(vec!["Payment".to_string()]),
            hook_on_outgoing: Some(Vec::new()),
        };
        let report = ValidationReport::default();
        let built = build_entry_sidecar(&entry(on), &chain(), b"AAAA", &report, None)
            .expect("sidecar builds");
        let value: serde_json::Value = serde_json::from_slice(&built.bytes).expect("valid json");

        assert!(value.get("HookOn").is_none());
        assert!(value["HookOnIncoming"].is_string());
        assert!(value["HookOnOutgoing"].is_string());
    }

    #[test]
    fn hook_name_byte_length_warning_matches_protocol_window() {
        let mut short_named = entry(omitted_on());
        short_named.hook_name = Some("ab".to_string());
        let report = ValidationReport::default();
        let built = build_entry_sidecar(&short_named, &chain(), b"AAAA", &report, None)
            .expect("sidecar builds");
        assert_eq!(built.warnings.len(), 1);
        assert!(built.warnings[0].contains("2 UTF-8 bytes"));
    }
}
