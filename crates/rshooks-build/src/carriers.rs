//! v2 `#[hooks]` carrier extraction and validation.
//!
//! `#[hooks] struct` and `#[hooks] impl` each embed a JSON declaration in the
//! name of a deliberately dead wasm export, using the same
//! prefix-plus-uppercase-hex mechanism as the `metadata!` carrier. This
//! module extracts and validates both carriers, and exposes the shared
//! trigger/mask logic used by the sidecar and SetHook template generators.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::metadata;

/// Prefix used by `#[hooks] struct` carrier exports.
pub const CHAIN_EXPORT_PREFIX: &str = "__rshooks_chain_v2_";
/// Prefix used by `#[hooks] impl` carrier exports.
pub const HOOKS_EXPORT_PREFIX: &str = "__rshooks_hooks_v2_";

/// The `#[hooks] struct` carrier: shared state/param schema declarations.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChainCarrier {
    /// Schema version tag; currently always `"rshooks-chain-v2"`.
    pub schema: String,
    /// The annotated struct's identifier.
    #[serde(rename = "struct")]
    pub struct_name: String,
    /// The struct-level `description = "..."` attribute value, if any.
    pub description: Option<String>,
    /// The struct's field declarations, grouped by kind.
    pub decls: ChainDecls,
}

/// The `decls` object of a [`ChainCarrier`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ChainDecls {
    /// `#[state(...)]` field declarations.
    #[serde(default)]
    pub state: Vec<StateDecl>,
    /// `#[hook_param(...)]` field declarations.
    #[serde(default)]
    pub hook_params: Vec<ParamDecl>,
    /// `#[otxn_param(...)]` field declarations.
    #[serde(default)]
    pub otxn_params: Vec<ParamDecl>,
    /// `#[state_interface(...)]` field declarations
    /// (`docs/STATE_INTERFACE_DESIGN.md`). `#[serde(default)]` so a carrier
    /// built without any (older `rshooks`, or simply no such field on the
    /// struct) still parses, as an empty list — the macro itself only emits
    /// the `state_interface` key when non-empty (byte-identity with a
    /// pre-`unstable-state-interface` build).
    #[serde(default)]
    pub state_interface: Vec<SiDecl>,
}

/// One `#[state_interface(...)]` field declaration
/// (`docs/STATE_INTERFACE_DESIGN.md` §5). Chain-level (like [`StateDecl`]),
/// not per-entry: all entries of the struct share the account/namespace
/// state, so [`crate::sethook_template`] emits this declaration's
/// `HookParameters` entry on every non-gap entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SiDecl {
    /// The struct field name.
    pub field: String,
    /// The State ID, `0..=255`.
    pub id: u8,
    /// The full declared `HookParameterName`, uppercase hex — resolved at
    /// macro time.
    pub name_hex: String,
    /// The full declared `HookParameterValue` (the value schema, not a
    /// `"00"` marker), uppercase hex — resolved at macro time.
    pub value_hex: String,
    /// Human-readable `(name: Type, ..)` display form of `key(..)` (`"()"`
    /// for a singleton), matching [`StateDecl`]'s display-string convention.
    pub key: String,
    /// Human-readable `(name: Type, ..)` display form of `value(..)`.
    pub value: String,
}

/// One `#[state(...)]` field declaration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateDecl {
    /// The struct field name.
    pub field: String,
    /// `"const"` (declared via `key = ...`) or `"keyed"` (via `key_by = ...`).
    pub kind: String,
    /// Normalized token string of the `key`/`key_by` expression or type.
    pub key: String,
    /// The declared value type, as written in source.
    pub value: String,
}

/// One `#[hook_param(...)]` / `#[otxn_param(...)]` field declaration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParamDecl {
    /// The struct field name.
    pub field: String,
    /// The literal `name = b"..."`, if declared.
    pub name: Option<String>,
    /// The `name_by = <Type>` type path, if declared.
    pub name_by: Option<String>,
    /// The declared value type, as written in source.
    pub value: String,
    /// Whether `required` was declared.
    pub required: bool,
    /// Normalized token text of the `default = ...` expression, or `None`
    /// if not declared. Kept as text (not just presence) so it's covered by
    /// `assert_carriers_match`'s byte-equal check and can appear in the
    /// per-entry sidecar's transcribed `chain.decls`.
    pub default: Option<String>,
}

/// The `#[hooks] impl` carrier: the entry table (`#[hook]`/`#[cbak]` fns).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntriesCarrier {
    /// Schema version tag; currently always `"rshooks-hooks-v2"`.
    pub schema: String,
    /// The annotated impl block's target type identifier.
    #[serde(rename = "impl")]
    pub impl_name: String,
    /// The chain's entries, sorted ascending by `index`.
    pub entries: Vec<EntryDecl>,
}

/// One chain entry (a `#[hook(i, ...)]` function, plus its optional
/// `#[cbak(i)]` twin).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntryDecl {
    /// The chain position, `0..=9`.
    pub index: u8,
    /// The `#[hook(i, ...)]` function's identifier.
    pub hook_fn: String,
    /// The paired `#[cbak(i)]` function's identifier, if declared.
    pub cbak_fn: Option<String>,
    /// The `name = "..."` attribute value, if declared.
    #[serde(rename = "HookName")]
    pub hook_name: Option<String>,
    /// The trigger (`on` / `on_incoming`+`on_outgoing`) declaration.
    pub on: OnDecl,
    /// The `can_emit = [...]` declaration: `None` when omitted, `Some(list)`
    /// (possibly empty) when declared.
    #[serde(rename = "HookCanEmit")]
    pub hook_can_emit: Option<Vec<String>>,
    /// The `description = "..."` attribute value, if declared.
    pub description: Option<String>,
    /// Declared signature parameters (`docs/PARAM_SIGNATURE_DESIGN.md`
    /// §1/§4) — extra `ident: Type` arguments on the `#[hook(..)]` fn, in
    /// wire-index order. `#[serde(default)]` so a carrier with no
    /// `sig_params` key still parses, as an empty list.
    #[serde(default)]
    pub sig_params: Vec<SigParamDecl>,
}

/// One declared signature parameter (`docs/PARAM_SIGNATURE_DESIGN.md`
/// §1/§4). Index is the position within [`EntryDecl::sig_params`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SigParamDecl {
    /// The declared name — the entry fn argument's own identifier.
    pub field: String,
    /// The `STI_*` type byte (`docs/PARAM_SIGNATURE_DESIGN.md` §2's table).
    pub type_byte: u8,
    /// The full declared `HookParameterName`, uppercase hex — resolved at
    /// macro time (`docs/PARAM_SIGNATURE_DESIGN.md` §4), so the build tool
    /// never re-derives it.
    pub name_hex: String,
}

/// The trigger declaration for one entry, in the three-state-preserving wire
/// shape the macro emits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OnDecl {
    /// One of `"omitted"`, `"all"`, `"list"`, `"directional"`.
    pub form: String,
    /// Present (as `Some`) only when `form == "list"`.
    #[serde(rename = "HookOn")]
    pub hook_on: Option<Vec<String>>,
    /// Present (as `Some`) only when `form == "directional"`.
    #[serde(rename = "HookOnIncoming")]
    pub hook_on_incoming: Option<Vec<String>>,
    /// Present (as `Some`) only when `form == "directional"`.
    #[serde(rename = "HookOnOutgoing")]
    pub hook_on_outgoing: Option<Vec<String>>,
}

/// Both v2 carriers extracted from one raw wasm artifact, plus their raw
/// (decoded, undecoded-further) JSON bytes for byte-equal consistency
/// checking between the discovery build and each selected build.
#[derive(Debug, Clone)]
pub struct ChainCarriers {
    /// The parsed struct carrier.
    pub chain: ChainCarrier,
    /// The exact decoded JSON bytes of the struct carrier.
    pub chain_raw: Vec<u8>,
    /// The parsed impl carrier.
    pub hooks: EntriesCarrier,
    /// The exact decoded JSON bytes of the impl carrier.
    pub hooks_raw: Vec<u8>,
}

/// Extracts and validates both v2 carriers from a raw cargo-produced wasm
/// artifact. `crate_label` is used only in error messages.
///
/// Errors when either carrier is duplicated, malformed, or missing. If only
/// the legacy `metadata!` (v1) carrier is present, the error carries a
/// migration hint instead of silently falling back to the v1 build path.
pub fn extract_chain_carriers(wasm: &[u8], crate_label: &str) -> Result<ChainCarriers> {
    let mut chain_raw: Option<Vec<u8>> = None;
    let mut hooks_raw: Option<Vec<u8>> = None;
    let mut chain_count = 0u32;
    let mut hooks_count = 0u32;
    let mut has_v1 = false;

    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let payload = payload.context("parsing raw wasm while looking for #[hooks] carriers")?;
        let wasmparser::Payload::ExportSection(reader) = payload else {
            continue;
        };
        for export in reader {
            let export =
                export.context("reading raw wasm export while looking for #[hooks] carriers")?;

            if export.name.starts_with(metadata::METADATA_EXPORT_PREFIX) {
                has_v1 = true;
                continue;
            }

            if let Some(encoded) = export.name.strip_prefix(CHAIN_EXPORT_PREFIX) {
                chain_count += 1;
                if export.kind != wasmparser::ExternalKind::Func {
                    bail!("#[hooks] struct carrier export must refer to a function");
                }
                chain_raw = Some(
                    metadata::decode_upper_hex(encoded)
                        .context("decoding #[hooks] struct carrier")?,
                );
                continue;
            }

            if let Some(encoded) = export.name.strip_prefix(HOOKS_EXPORT_PREFIX) {
                hooks_count += 1;
                if export.kind != wasmparser::ExternalKind::Func {
                    bail!("#[hooks] impl carrier export must refer to a function");
                }
                hooks_raw = Some(
                    metadata::decode_upper_hex(encoded)
                        .context("decoding #[hooks] impl carrier")?,
                );
            }
        }
    }

    if chain_count > 1 {
        bail!("raw wasm contains multiple #[hooks] struct carrier exports");
    }
    if hooks_count > 1 {
        bail!("raw wasm contains multiple #[hooks] impl carrier exports");
    }

    let (chain_raw, hooks_raw) = match (chain_raw, hooks_raw) {
        (Some(c), Some(h)) => (c, h),
        (None, None) if has_v1 => bail!(
            "crate `{crate_label}` uses the old `metadata!` carrier; rshooks-build 0.2 requires \
             the `#[hooks]` struct/impl model. Migrate the crate to `#[hooks] struct ...` plus \
             `#[hooks] impl ...` (see docs/MULTI_HOOK_STRUCT_DESIGN.md) before building with this \
             version of `rshooks`."
        ),
        (None, None) => bail!(
            "no #[hooks] chain found in {crate_label} (rshooks 0.2 requires the #[hooks] \
             struct/impl model)"
        ),
        (Some(_), None) => bail!(
            "crate `{crate_label}` declares a `#[hooks]` struct but no `#[hooks]` impl block; \
             add `#[hooks] impl <Struct> {{ ... }}`"
        ),
        (None, Some(_)) => bail!(
            "crate `{crate_label}` declares a `#[hooks]` impl block but no `#[hooks]` struct; \
             add `#[hooks] struct <Struct> {{ ... }}`"
        ),
    };

    let chain: ChainCarrier =
        serde_json::from_slice(&chain_raw).context("parsing JSON from #[hooks] struct carrier")?;
    let hooks: EntriesCarrier =
        serde_json::from_slice(&hooks_raw).context("parsing JSON from #[hooks] impl carrier")?;

    validate_carrier_identity(&chain, &hooks)
        .with_context(|| format!("validating #[hooks] carrier identity in {crate_label}"))?;
    validate_entries(&hooks)
        .with_context(|| format!("validating #[hooks] chain in {crate_label}"))?;

    Ok(ChainCarriers {
        chain,
        chain_raw,
        hooks,
        hooks_raw,
    })
}

/// Schema tag every `#[hooks] struct` carrier must carry.
const CHAIN_SCHEMA: &str = "rshooks-chain-v2";
/// Schema tag every `#[hooks] impl` carrier must carry.
const HOOKS_SCHEMA: &str = "rshooks-hooks-v2";

/// Validates the two carriers' declared identity: exact schema tags, and
/// that the impl carrier's target type matches the struct carrier's
/// annotated struct. Both carriers are untrusted input (hand-decoded from
/// wasm export names), so a mismatch here signals version skew or a
/// malformed/foreign carrier, reported distinctly from
/// [`validate_entries`]'s content-level checks.
fn validate_carrier_identity(chain: &ChainCarrier, hooks: &EntriesCarrier) -> Result<()> {
    if chain.schema != CHAIN_SCHEMA {
        bail!(
            "#[hooks] struct carrier has schema `{}`, expected `{CHAIN_SCHEMA}` — this rshooks-build \
             version cannot read a carrier from a different rshooks version",
            chain.schema
        );
    }
    if hooks.schema != HOOKS_SCHEMA {
        bail!(
            "#[hooks] impl carrier has schema `{}`, expected `{HOOKS_SCHEMA}` — this rshooks-build \
             version cannot read a carrier from a different rshooks version",
            hooks.schema
        );
    }
    if chain.struct_name != hooks.impl_name {
        bail!(
            "#[hooks] struct carrier declares struct `{}` but the #[hooks] impl carrier targets \
             `{}` — a crate may declare exactly one #[hooks] struct/impl pair, and they must name \
             the same type",
            chain.struct_name,
            hooks.impl_name
        );
    }
    Ok(())
}

/// Validates a parsed impl carrier: non-empty, unique indices in `0..=9`
/// (re-checked here since carriers are untrusted input), known
/// transaction-type names, and `HookName` character-count rules.
pub fn validate_entries(entries: &EntriesCarrier) -> Result<()> {
    if entries.entries.is_empty() {
        bail!("no #[hook] entries declared in the #[hooks] impl block");
    }

    let mut seen_index = std::collections::BTreeSet::new();
    for entry in &entries.entries {
        if entry.index > 9 {
            bail!(
                "entry {} (`{}`): index is out of range 0..=9",
                entry.index,
                entry.hook_fn
            );
        }
        if !seen_index.insert(entry.index) {
            bail!("duplicate hook entry index {}", entry.index);
        }

        validate_on(entry)?;
        metadata::validate_transaction_types("HookCanEmit", entry.hook_can_emit.as_deref())
            .with_context(|| format!("entry {} (`{}`)", entry.index, entry.hook_fn))?;

        if let Some(name) = &entry.hook_name {
            let char_count = name.chars().count();
            if !(2..=8).contains(&char_count) {
                bail!(
                    "entry {} (`{}`): `name` must contain 2 to 8 UTF-8 characters",
                    entry.index,
                    entry.hook_fn
                );
            }
        }
    }

    Ok(())
}

fn validate_on(entry: &EntryDecl) -> Result<()> {
    let on = &entry.on;
    let context_label = || format!("entry {} (`{}`)", entry.index, entry.hook_fn);
    match on.form.as_str() {
        "omitted" | "all" => Ok(()),
        "list" => {
            let list = on.hook_on.as_deref().with_context(|| {
                format!(
                    "{}: `on` form \"list\" must carry `HookOn`",
                    context_label()
                )
            })?;
            metadata::validate_transaction_types("HookOn", Some(list)).with_context(context_label)
        }
        "directional" => {
            let incoming = on.hook_on_incoming.as_deref().with_context(|| {
                format!(
                    "{}: `on` form \"directional\" must carry `HookOnIncoming`",
                    context_label()
                )
            })?;
            let outgoing = on.hook_on_outgoing.as_deref().with_context(|| {
                format!(
                    "{}: `on` form \"directional\" must carry `HookOnOutgoing`",
                    context_label()
                )
            })?;
            metadata::validate_transaction_types("HookOnIncoming", Some(incoming))
                .with_context(context_label)?;
            metadata::validate_transaction_types("HookOnOutgoing", Some(outgoing))
                .with_context(context_label)
        }
        other => bail!("{}: unknown trigger form `{other}`", context_label()),
    }
}

/// Compares the v2 carriers extracted from a per-index selected build
/// against the discovery build's carriers. Requires byte-equal decoded JSON
/// payloads (§C item 4c): cleaning removes the carriers, so this check must
/// run before the pipeline.
pub fn assert_carriers_match(
    discovery: &ChainCarriers,
    selected: &ChainCarriers,
    index: u8,
) -> Result<()> {
    if discovery.chain_raw != selected.chain_raw {
        bail!(
            "entry {index}: the #[hooks] struct carrier differs between the discovery build and \
             the selected build for this entry; this should be impossible under a fixed \
             BuildPlan — check for build-script or environment-dependent macro output"
        );
    }
    if discovery.hooks_raw != selected.hooks_raw {
        bail!(
            "entry {index}: the #[hooks] impl carrier differs between the discovery build and \
             the selected build for this entry; this should be impossible under a fixed \
             BuildPlan — check for build-script or environment-dependent macro output"
        );
    }
    Ok(())
}

/// The resolved `HookOn`/`HookOnIncoming`/`HookOnOutgoing` wire values for
/// one entry's trigger declaration. At most one of `hook_on` or the
/// incoming/outgoing pair is ever `Some`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TriggerMasks {
    /// The `HookOn` field value, when `form` is `"all"` or `"list"`.
    pub hook_on: Option<String>,
    /// The `HookOnIncoming` field value, when `form` is `"directional"`.
    pub hook_on_incoming: Option<String>,
    /// The `HookOnOutgoing` field value, when `form` is `"directional"`.
    pub hook_on_outgoing: Option<String>,
}

/// The `on = all` `HookOn` mask: 64 zero hex digits.
///
/// `hook_mask` cannot express this: `None` means "field omitted" (the `on`
/// attribute was never written), and `Some(&[])` yields the deny-all base
/// mask (`FF..BF..FF`, "fires on nothing") — neither is "fires on everything
/// except SetHook", which is what `on = all` means. Ordinary (active-low)
/// HookOn bits: 0 = do fire. The special-cased SetHook bit (byte 29, tt22)
/// is active-high: 0 = do NOT fire. So the all-zero mask is exactly "fire on
/// every transaction type except SetHook" — see the rshooks v0.2
/// implementation contract §D for the derivation.
#[must_use]
pub fn hook_on_all_mask() -> String {
    "0".repeat(64)
}

/// Derives the wire-shape `HookOn`/`HookOnIncoming`/`HookOnOutgoing` values
/// for one entry's trigger declaration, mapping the three-state carrier
/// shape onto the protocol's mask fields.
pub fn resolve_trigger_masks(on: &OnDecl) -> Result<TriggerMasks> {
    match on.form.as_str() {
        "omitted" => Ok(TriggerMasks::default()),
        "all" => Ok(TriggerMasks {
            hook_on: Some(hook_on_all_mask()),
            ..TriggerMasks::default()
        }),
        "list" => {
            let list = on.hook_on.as_deref().unwrap_or(&[]);
            let mask = metadata::hook_mask(Some(list))?
                .context("internal error: `on = [...]` produced an empty (all-zero) mask")?;
            Ok(TriggerMasks {
                hook_on: Some(mask),
                ..TriggerMasks::default()
            })
        }
        "directional" => {
            let incoming = on.hook_on_incoming.as_deref().unwrap_or(&[]);
            let outgoing = on.hook_on_outgoing.as_deref().unwrap_or(&[]);
            let incoming_mask = metadata::hook_mask(Some(incoming))?
                .context("internal error: `on_incoming = [...]` produced an all-zero mask")?;
            let outgoing_mask = metadata::hook_mask(Some(outgoing))?
                .context("internal error: `on_outgoing = [...]` produced an all-zero mask")?;
            Ok(TriggerMasks {
                hook_on_incoming: Some(incoming_mask),
                hook_on_outgoing: Some(outgoing_mask),
                ..TriggerMasks::default()
            })
        }
        other => bail!("unknown trigger form `{other}`"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn upper_hex(input: &[u8]) -> String {
        input.iter().map(|byte| format!("{byte:02X}")).collect()
    }

    fn wasm_with_carriers(chain_json: &str, hooks_json: &str) -> Vec<u8> {
        let chain_marker = format!("{CHAIN_EXPORT_PREFIX}{}", upper_hex(chain_json.as_bytes()));
        let hooks_marker = format!("{HOOKS_EXPORT_PREFIX}{}", upper_hex(hooks_json.as_bytes()));
        wat::parse_str(format!(
            r#"
            (module
              (func $hook (param i32) (result i64) i64.const 0)
              (export "hook" (func $hook))
              (export "{chain_marker}" (func $hook))
              (export "{hooks_marker}" (func $hook)))
            "#
        ))
        .expect("valid v2 carrier fixture")
    }

    const CHAIN_JSON: &str = concat!(
        r#"{"schema":"rshooks-chain-v2","struct":"Vault","description":null,"#,
        r#""decls":{"state":[],"hook_params":[],"otxn_params":[]}}"#
    );

    fn entries_json(on: &str, can_emit: &str) -> String {
        format!(
            r#"{{"schema":"rshooks-hooks-v2","impl":"Vault","entries":[{{"index":0,"hook_fn":"deposit","cbak_fn":null,"HookName":null,"on":{on},"HookCanEmit":{can_emit},"description":null}}]}}"#
        )
    }

    #[test]
    fn extracts_both_v2_carriers_from_synthetic_wasm() {
        let on =
            r#"{"form":"list","HookOn":["Payment"],"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let wasm = wasm_with_carriers(CHAIN_JSON, &entries_json(on, "null"));

        let carriers = extract_chain_carriers(&wasm, "vault").expect("extraction succeeds");
        assert_eq!(carriers.chain.struct_name, "Vault");
        assert_eq!(carriers.hooks.entries.len(), 1);
        assert_eq!(carriers.hooks.entries[0].hook_fn, "deposit");
        assert_eq!(carriers.hooks.entries[0].on.form, "list");
    }

    #[test]
    fn wrong_chain_schema_is_rejected() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let bad_chain_json = concat!(
            r#"{"schema":"rshooks-chain-v1","struct":"Vault","description":null,"#,
            r#""decls":{"state":[],"hook_params":[],"otxn_params":[]}}"#
        );
        let wasm = wasm_with_carriers(bad_chain_json, &entries_json(on, "null"));
        let err = extract_chain_carriers(&wasm, "vault").expect_err("must fail");
        let message = format!("{err:#}");
        assert!(message.contains("rshooks-chain-v2"), "{message}");
    }

    #[test]
    fn wrong_hooks_schema_is_rejected() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let bad_hooks_json = format!(
            r#"{{"schema":"rshooks-hooks-v1","impl":"Vault","entries":[{{"index":0,"hook_fn":"deposit","cbak_fn":null,"HookName":null,"on":{on},"HookCanEmit":null,"description":null}}]}}"#
        );
        let wasm = wasm_with_carriers(CHAIN_JSON, &bad_hooks_json);
        let err = extract_chain_carriers(&wasm, "vault").expect_err("must fail");
        let message = format!("{err:#}");
        assert!(message.contains("rshooks-hooks-v2"), "{message}");
    }

    #[test]
    fn mismatched_struct_and_impl_names_are_rejected() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let mismatched_hooks_json = format!(
            r#"{{"schema":"rshooks-hooks-v2","impl":"NotVault","entries":[{{"index":0,"hook_fn":"deposit","cbak_fn":null,"HookName":null,"on":{on},"HookCanEmit":null,"description":null}}]}}"#
        );
        let wasm = wasm_with_carriers(CHAIN_JSON, &mismatched_hooks_json);
        let err = extract_chain_carriers(&wasm, "vault").expect_err("must fail");
        let message = format!("{err:#}");
        assert!(message.contains("Vault"), "{message}");
        assert!(message.contains("NotVault"), "{message}");
    }

    #[test]
    fn missing_both_carriers_reports_no_chain_found() {
        let wasm = wat::parse_str(
            r#"(module (func $hook (param i32) (result i64) i64.const 0) (export "hook" (func $hook)))"#,
        )
        .expect("valid empty fixture");
        let err = extract_chain_carriers(&wasm, "vault").expect_err("must fail");
        assert!(format!("{err:#}").contains("no #[hooks] chain found"));
    }

    #[test]
    fn v1_only_carrier_gets_migration_hint() {
        let marker = format!(
            "{}{}",
            metadata::METADATA_EXPORT_PREFIX,
            upper_hex(br#"{"name":"Probe"}"#)
        );
        let wasm = wat::parse_str(format!(
            r#"
            (module
              (func $hook (param i32) (result i64) i64.const 0)
              (export "hook" (func $hook))
              (export "{marker}" (func $hook)))
            "#
        ))
        .expect("valid v1 fixture");
        let err = extract_chain_carriers(&wasm, "vault").expect_err("must fail");
        assert!(format!("{err:#}").contains("Migrate the crate"));
    }

    #[test]
    fn duplicate_indices_are_rejected() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let hooks = format!(
            r#"{{"schema":"rshooks-hooks-v2","impl":"Vault","entries":[{{"index":0,"hook_fn":"a","cbak_fn":null,"HookName":null,"on":{on},"HookCanEmit":null,"description":null}},{{"index":0,"hook_fn":"b","cbak_fn":null,"HookName":null,"on":{on},"HookCanEmit":null,"description":null}}]}}"#
        );
        let wasm = wasm_with_carriers(CHAIN_JSON, &hooks);
        let err = extract_chain_carriers(&wasm, "vault").expect_err("must fail");
        assert!(format!("{err:#}").contains("duplicate hook entry index"));
    }

    #[test]
    fn byte_equal_consistency_check_flags_divergence() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let discovery_wasm = wasm_with_carriers(CHAIN_JSON, &entries_json(on, "null"));
        let selected_wasm = wasm_with_carriers(CHAIN_JSON, &entries_json(on, "[]"));

        let discovery = extract_chain_carriers(&discovery_wasm, "vault").expect("extract");
        let selected = extract_chain_carriers(&selected_wasm, "vault").expect("extract");

        let err = assert_carriers_match(&discovery, &selected, 0).expect_err("must diverge");
        assert!(format!("{err:#}").contains("entry 0"));
    }

    #[test]
    fn on_all_mask_is_all_zero_and_fires_on_everything_but_sethook() {
        let mask = hook_on_all_mask();
        assert_eq!(mask.len(), 64);
        assert!(mask.chars().all(|c| c == '0'));

        // Cross-check against `hook_mask`: the deny-all base has every
        // ordinary bit set to 1 (do NOT fire) and the SetHook bit (byte 29,
        // low nibble) cleared to 0 within 0xBF (do NOT fire on SetHook
        // either). `on = all` inverts every ordinary bit to 0 (DO fire)
        // while leaving the SetHook bit at 0 — exactly the all-zero mask.
        let deny_all = metadata::hook_mask(Some(&[]))
            .expect("hook_mask never fails for an empty list")
            .expect("deny-all base mask is never all-zero");
        assert_eq!(&deny_all[58..60], "BF");
        assert_ne!(deny_all, mask);
    }

    #[test]
    fn resolve_trigger_masks_covers_all_four_forms() {
        let omitted = OnDecl {
            form: "omitted".to_string(),
            hook_on: None,
            hook_on_incoming: None,
            hook_on_outgoing: None,
        };
        let resolved = resolve_trigger_masks(&omitted).expect("omitted resolves");
        assert_eq!(resolved, TriggerMasks::default());

        let all = OnDecl {
            form: "all".to_string(),
            hook_on: None,
            hook_on_incoming: None,
            hook_on_outgoing: None,
        };
        let resolved = resolve_trigger_masks(&all).expect("all resolves");
        assert_eq!(
            resolved.hook_on.as_deref(),
            Some(hook_on_all_mask().as_str())
        );

        let list = OnDecl {
            form: "list".to_string(),
            hook_on: Some(vec!["Payment".to_string()]),
            hook_on_incoming: None,
            hook_on_outgoing: None,
        };
        let resolved = resolve_trigger_masks(&list).expect("list resolves");
        assert!(resolved.hook_on.is_some());
        assert!(resolved.hook_on_incoming.is_none());

        let directional = OnDecl {
            form: "directional".to_string(),
            hook_on: None,
            hook_on_incoming: Some(vec!["Payment".to_string()]),
            hook_on_outgoing: Some(vec![]),
        };
        let resolved = resolve_trigger_masks(&directional).expect("directional resolves");
        assert!(resolved.hook_on.is_none());
        assert!(resolved.hook_on_incoming.is_some());
        assert!(resolved.hook_on_outgoing.is_some());
    }

    // --- `sig_params` (docs/PARAM_SIGNATURE_DESIGN.md §1/§4) ---

    #[test]
    fn entry_decl_without_sig_params_key_defaults_to_empty() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let wasm = wasm_with_carriers(CHAIN_JSON, &entries_json(on, "null"));
        let carriers = extract_chain_carriers(&wasm, "vault").expect("extraction succeeds");
        assert!(carriers.hooks.entries[0].sig_params.is_empty());
    }

    #[test]
    fn entry_decl_round_trips_sig_params() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let hooks = format!(
            r#"{{"schema":"rshooks-hooks-v2","impl":"Vault","entries":[{{"index":0,"hook_fn":"increment","cbak_fn":null,"HookName":null,"on":{on},"HookCanEmit":null,"description":null,"sig_params":[{{"field":"account","type_byte":8,"name_hex":"5F5053000008076163636F756E74"}},{{"field":"count","type_byte":1,"name_hex":"5F505300010105636F756E74"}}]}}]}}"#
        );
        let wasm = wasm_with_carriers(CHAIN_JSON, &hooks);
        let carriers = extract_chain_carriers(&wasm, "vault").expect("extraction succeeds");
        let sig_params = &carriers.hooks.entries[0].sig_params;
        assert_eq!(sig_params.len(), 2);
        assert_eq!(sig_params[0].field, "account");
        assert_eq!(sig_params[0].type_byte, 8);
        assert_eq!(sig_params[0].name_hex, "5F5053000008076163636F756E74");
        assert_eq!(sig_params[1].field, "count");
        assert_eq!(sig_params[1].type_byte, 1);
    }

    #[test]
    fn entry_decl_rejects_unknown_sig_param_field() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let hooks = format!(
            r#"{{"schema":"rshooks-hooks-v2","impl":"Vault","entries":[{{"index":0,"hook_fn":"increment","cbak_fn":null,"HookName":null,"on":{on},"HookCanEmit":null,"description":null,"sig_params":[{{"field":"account","type_byte":8,"name_hex":"AA","unknown":1}}]}}]}}"#
        );
        let wasm = wasm_with_carriers(CHAIN_JSON, &hooks);
        assert!(extract_chain_carriers(&wasm, "vault").is_err());
    }

    // --- `state_interface` (docs/STATE_INTERFACE_DESIGN.md §5) ---

    #[test]
    fn chain_carrier_without_state_interface_key_defaults_to_empty() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let wasm = wasm_with_carriers(CHAIN_JSON, &entries_json(on, "null"));
        let carriers = extract_chain_carriers(&wasm, "vault").expect("extraction succeeds");
        assert!(carriers.chain.decls.state_interface.is_empty());
    }

    #[test]
    fn chain_carrier_round_trips_state_interface_decls() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let chain_json = concat!(
            r#"{"schema":"rshooks-chain-v2","struct":"Vault","description":null,"#,
            r#""decls":{"state":[],"hook_params":[],"otxn_params":[],"state_interface":[{"#,
            r#""field":"balances","id":0,"#,
            r#""name_hex":"5F534900000208076163636F756E740205746F6B656E","#,
            r#""value_hex":"020306616D6F756E74020775706461746564","#,
            r#""key":"(account: AccountId, token: u32)","value":"(amount: u64, updated: u32)"}]}}"#
        );
        let wasm = wasm_with_carriers(chain_json, &entries_json(on, "null"));
        let carriers = extract_chain_carriers(&wasm, "vault").expect("extraction succeeds");
        let si = &carriers.chain.decls.state_interface;
        assert_eq!(si.len(), 1);
        assert_eq!(si[0].field, "balances");
        assert_eq!(si[0].id, 0);
        assert_eq!(
            si[0].name_hex,
            "5F534900000208076163636F756E740205746F6B656E"
        );
        assert_eq!(si[0].value_hex, "020306616D6F756E74020775706461746564");
        assert_eq!(si[0].key, "(account: AccountId, token: u32)");
        assert_eq!(si[0].value, "(amount: u64, updated: u32)");
    }

    #[test]
    fn chain_carrier_rejects_unknown_state_interface_field() {
        let on = r#"{"form":"omitted","HookOn":null,"HookOnIncoming":null,"HookOnOutgoing":null}"#;
        let chain_json = concat!(
            r#"{"schema":"rshooks-chain-v2","struct":"Vault","description":null,"#,
            r#""decls":{"state":[],"hook_params":[],"otxn_params":[],"state_interface":[{"#,
            r#""field":"balances","id":0,"name_hex":"AA","value_hex":"BB","key":"()","#,
            r#""value":"()","unknown":1}]}}"#
        );
        let wasm = wasm_with_carriers(chain_json, &entries_json(on, "null"));
        assert!(extract_chain_carriers(&wasm, "vault").is_err());
    }
}
