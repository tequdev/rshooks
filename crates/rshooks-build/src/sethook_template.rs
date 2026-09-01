//! Generates `sethook.template.json` (the SetHook "occupied-position patch")
//! and its `sethook.template.meta.json` provenance sidecar.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::carriers::{EntryDecl, SiDecl, SigParamDecl, resolve_trigger_masks};
use crate::metadata::{hook_mask, utf8_hex};

/// `hsfOVERRIDE` (vendor/xahaud Enum.h `HookSetFlags`): permits replacing an
/// existing installed Hook at a declared (non-gap) position.
pub const HSF_OVERRIDE: u32 = 1;

/// Base58 alphabet used by XRPL/Xahau addresses (excludes `0`, `O`, `I`, `l`
/// to avoid visual ambiguity).
const BASE58_ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Validates a `--namespace` CLI value: exactly 64 ASCII hex characters.
/// Normalizes the result to uppercase (the conventional `HookNamespace`
/// casing) on success. Intended for use as a clap `value_parser`.
pub fn validate_namespace(s: &str) -> Result<String, String> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "--namespace must be exactly 64 ASCII hex characters (got {} characters)",
            s.chars().count()
        ));
    }
    Ok(s.to_ascii_uppercase())
}

/// Validates a `--account` CLI value: a superficial (non-checksum) sanity
/// check that it looks like an XRPL/Xahau classic address — non-empty,
/// starts with `'r'`, and every remaining character is in the base58
/// alphabet. Intended for use as a clap `value_parser`.
pub fn validate_account(s: &str) -> Result<String, String> {
    if !s.starts_with('r') {
        return Err("--account must start with 'r'".to_string());
    }
    let rest = s.get(1..).unwrap_or_default();
    if rest.is_empty() {
        return Err("--account must not be empty".to_string());
    }
    if !rest.chars().all(|c| BASE58_ALPHABET.contains(c)) {
        return Err(
            "--account must contain only base58 characters after the leading 'r' \
             (no '0', 'O', 'I', or 'l')"
                .to_string(),
        );
    }
    Ok(s.to_string())
}

/// One `Hooks[i]` array element: either an untouched-position gap
/// (`{"Hook": {}}`) or a fully specified declared entry.
#[derive(Serialize)]
struct HooksArrayItem {
    #[serde(rename = "Hook")]
    hook: HookSlot,
}

#[derive(Serialize)]
#[serde(untagged)]
enum HookSlot {
    /// A position-preserving no-op. MUST stay exactly `{}` — adding any
    /// field (even under `--override`) turns it into a different, non-op
    /// SetHook operation.
    Gap(GapMarker),
    Entry(Box<HookEntryTemplate>),
}

#[derive(Serialize)]
struct GapMarker {}

/// Field declaration order is the emitted JSON key order (serde preserves
/// struct-field order); reordering these fields changes the generated
/// template's byte-for-byte output, so this order must match design §9.1 /
/// contract §C item 6 exactly.
#[derive(Serialize)]
struct HookEntryTemplate {
    #[serde(rename = "CreateCode")]
    create_code: String,
    #[serde(rename = "HookOn", skip_serializing_if = "Option::is_none")]
    hook_on: Option<String>,
    #[serde(rename = "HookOnIncoming", skip_serializing_if = "Option::is_none")]
    hook_on_incoming: Option<String>,
    #[serde(rename = "HookOnOutgoing", skip_serializing_if = "Option::is_none")]
    hook_on_outgoing: Option<String>,
    #[serde(rename = "HookCanEmit", skip_serializing_if = "Option::is_none")]
    hook_can_emit: Option<String>,
    #[serde(rename = "HookNamespace")]
    hook_namespace: String,
    #[serde(rename = "HookApiVersion")]
    hook_api_version: u8,
    #[serde(rename = "HookParameters", skip_serializing_if = "Option::is_none")]
    hook_parameters: Option<Vec<HookParameterItem>>,
    #[serde(rename = "HookName", skip_serializing_if = "Option::is_none")]
    hook_name: Option<String>,
    #[serde(rename = "Flags", skip_serializing_if = "Option::is_none")]
    flags: Option<u32>,
}

/// One `HookParameters[i]` array element — a declaration entry.
/// `HookParameterValue` carries either the signature-parameter interface's
/// literal `"00"` placeholder byte (`docs/PARAM_SIGNATURE_DESIGN.md` §4's
/// declaration-entry convention) or the state interface's real value schema
/// (`docs/STATE_INTERFACE_DESIGN.md` §5), depending on which declaration
/// produced this entry.
#[derive(Serialize)]
struct HookParameterItem {
    #[serde(rename = "HookParameter")]
    hook_parameter: HookParameterFields,
}

#[derive(Serialize)]
struct HookParameterFields {
    #[serde(rename = "HookParameterName")]
    hook_parameter_name: String,
    #[serde(rename = "HookParameterValue")]
    hook_parameter_value: String,
}

#[derive(Serialize)]
struct SetHookTemplate {
    #[serde(rename = "TransactionType")]
    transaction_type: &'static str,
    #[serde(rename = "Account")]
    account: String,
    #[serde(rename = "Hooks")]
    hooks: Vec<HooksArrayItem>,
}

/// A resolved (index, entry declaration, final wasm bytes) triple used to
/// build the template; not necessarily contiguous — gaps are filled in by
/// [`build_template_json`] between `0` and the maximum index present.
pub type TemplateInput<'a> = (u8, &'a EntryDecl, &'a [u8]);

/// Builds the pretty-printed `sethook.template.json` bytes (with trailing
/// newline). `declared` need not be sorted or contiguous; gaps between `0`
/// and the maximum declared index are filled with `{"Hook": {}}`.
///
/// `account`/`namespace` fill the `Account`/`HookNamespace` placeholders
/// when given, else the literal placeholder strings `<ACCOUNT>` /
/// `<NAMESPACE>` are emitted. When `override_flag` is set, `Flags: 1`
/// (`hsfOVERRIDE`) is added to every declared (non-gap) entry only.
///
/// `si_decls` is the chain's `#[state_interface(..)]` declarations
/// (`docs/STATE_INTERFACE_DESIGN.md` §5) — chain-level, not per-entry, so
/// the same list is emitted on every declared (non-gap) entry's
/// `HookParameters`, after that entry's own signature-parameter
/// declarations if any.
pub fn build_template_json(
    declared: &[TemplateInput<'_>],
    si_decls: &[SiDecl],
    account: Option<&str>,
    namespace: Option<&str>,
    override_flag: bool,
) -> Result<Vec<u8>> {
    let max_index = declared
        .iter()
        .map(|(index, _, _)| *index)
        .max()
        .context("a SetHook template requires at least one declared entry")?;

    let mut hooks = Vec::with_capacity(usize::from(max_index) + 1);
    for i in 0..=max_index {
        let found = declared.iter().find(|(index, _, _)| *index == i);
        let slot = match found {
            Some((_, entry, wasm)) => HookSlot::Entry(Box::new(build_entry_template(
                entry,
                wasm,
                si_decls,
                namespace,
                override_flag,
            )?)),
            None => HookSlot::Gap(GapMarker {}),
        };
        hooks.push(HooksArrayItem { hook: slot });
    }

    let doc = SetHookTemplate {
        transaction_type: "SetHook",
        account: account.map_or_else(|| "<ACCOUNT>".to_string(), str::to_string),
        hooks,
    };

    let mut bytes = serde_json::to_vec_pretty(&doc).context("serializing sethook.template.json")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn build_entry_template(
    entry: &EntryDecl,
    wasm: &[u8],
    si_decls: &[SiDecl],
    namespace: Option<&str>,
    override_flag: bool,
) -> Result<HookEntryTemplate> {
    let masks = resolve_trigger_masks(&entry.on)
        .with_context(|| format!("entry {} (`{}`)", entry.index, entry.hook_fn))?;
    let hook_can_emit = hook_mask(entry.hook_can_emit.as_deref())
        .with_context(|| format!("entry {} (`{}`)", entry.index, entry.hook_fn))?;

    Ok(HookEntryTemplate {
        create_code: wasm.iter().map(|byte| format!("{byte:02X}")).collect(),
        hook_on: masks.hook_on,
        hook_on_incoming: masks.hook_on_incoming,
        hook_on_outgoing: masks.hook_on_outgoing,
        hook_can_emit,
        hook_namespace: namespace.map_or_else(|| "<NAMESPACE>".to_string(), str::to_string),
        hook_api_version: 0,
        hook_parameters: build_hook_parameters(
            entry.index,
            entry.sig_params.as_deref().unwrap_or(&[]),
            si_decls,
        )
        .with_context(|| format!("entry {} (`{}`)", entry.index, entry.hook_fn))?,
        hook_name: entry.hook_name.as_deref().map(utf8_hex),
        flags: override_flag.then_some(HSF_OVERRIDE),
    })
}

/// The protocol's maximum `HookParameters` entries per `Hook` object —
/// xahaud `validateHookParams`
/// (`src/xrpld/app/tx/detail/SetHook.cpp`): `paramCount > 16` is rejected
/// as malformed. Both declaration sources below share this one limit, so
/// it's enforced where they're merged, not per-source.
const MAX_HOOK_PARAMETERS_PER_ENTRY: usize = 16;

/// Builds the `HookParameters` declaration array for one entry: its own
/// declared signature parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §4,
/// `HookParameterValue = "00"`), then the chain's state interface
/// declarations (`docs/STATE_INTERFACE_DESIGN.md` §5, `HookParameterValue`
/// = the real value schema) — `None` (key omitted) only when both are
/// empty. This is the sole exception to the "`HookParameters` is never
/// generated" rule (`docs/MULTI_HOOK_STRUCT_DESIGN.md` §9 point #10).
/// `sig_params` is already carrier-order (= wire index) ascending, so no
/// re-sorting needed; `si_decls` is chain-level and identical across every
/// entry.
///
/// # Errors
///
/// If the combined count exceeds [`MAX_HOOK_PARAMETERS_PER_ENTRY`] — the
/// generated template would otherwise be rejected on submit
/// (`temMALFORMED`) by the same xahaud check this constant cites.
fn build_hook_parameters(
    index: u8,
    sig_params: &[SigParamDecl],
    si_decls: &[SiDecl],
) -> Result<Option<Vec<HookParameterItem>>> {
    if sig_params.is_empty() && si_decls.is_empty() {
        return Ok(None);
    }
    let total = sig_params.len().wrapping_add(si_decls.len());
    if total > MAX_HOOK_PARAMETERS_PER_ENTRY {
        bail!(
            "entry {index}: {total} HookParameters ({} signature-parameter declaration(s) + \
             {} state interface declaration(s)) exceeds the protocol's \
             {MAX_HOOK_PARAMETERS_PER_ENTRY}-per-entry limit (xahaud `validateHookParams`, \
             src/xrpld/app/tx/detail/SetHook.cpp)",
            sig_params.len(),
            si_decls.len(),
        );
    }
    let sig_items = sig_params.iter().map(|p| HookParameterItem {
        hook_parameter: HookParameterFields {
            hook_parameter_name: p.name_hex.clone(),
            hook_parameter_value: "00".to_string(),
        },
    });
    let si_items = si_decls.iter().map(|d| HookParameterItem {
        hook_parameter: HookParameterFields {
            hook_parameter_name: d.name_hex.clone(),
            hook_parameter_value: d.value_hex.clone(),
        },
    });
    Ok(Some(sig_items.chain(si_items).collect()))
}

/// Derives `required_amendments` from what the template's own fields
/// structurally require. Always includes `"Hooks"`. Does NOT account for
/// Hook API surface used inside the wasm itself (documented in the
/// sidecar's own field).
#[must_use]
pub fn required_amendments(entries: &[EntryDecl]) -> Vec<String> {
    let mut amendments = vec!["Hooks".to_string()];
    if entries.iter().any(|e| e.hook_name.is_some()) {
        amendments.push("NamedHooks".to_string());
    }
    if entries.iter().any(|e| e.on.form == "directional") {
        amendments.push("HookOnV2".to_string());
    }
    if entries.iter().any(|e| e.hook_can_emit.is_some()) {
        amendments.push("HookCanEmit".to_string());
    }
    amendments
}

#[derive(Serialize)]
struct Positions {
    declared: Vec<u8>,
    gaps: Vec<u8>,
    untouched_beyond: u16,
}

#[derive(Serialize)]
struct TemplateMetaDoc {
    #[serde(rename = "crate")]
    crate_name: String,
    version: String,
    generated_at: String,
    hook_hashes: BTreeMap<String, String>,
    positions: Positions,
    required_amendments: Vec<String>,
}

/// Builds the pretty-printed `sethook.template.meta.json` bytes (with
/// trailing newline). `declared`/`gaps` are the entry indices in ascending
/// order; `max_index` is the highest declared index (so
/// `untouched_beyond == max_index + 1`).
#[allow(clippy::too_many_arguments)]
pub fn build_template_meta(
    crate_name: &str,
    version: &str,
    hook_hashes: &BTreeMap<u8, String>,
    declared: &[u8],
    gaps: &[u8],
    max_index: u8,
    required_amendments: &[String],
) -> Result<Vec<u8>> {
    let hook_hashes = hook_hashes
        .iter()
        .map(|(index, hash)| (index.to_string(), hash.clone()))
        .collect();

    let doc = TemplateMetaDoc {
        crate_name: crate_name.to_string(),
        version: version.to_string(),
        generated_at: rfc3339_now(),
        hook_hashes,
        positions: Positions {
            declared: declared.to_vec(),
            gaps: gaps.to_vec(),
            untouched_beyond: u16::from(max_index) + 1,
        },
        required_amendments: required_amendments.to_vec(),
    };

    let mut bytes =
        serde_json::to_vec_pretty(&doc).context("serializing sethook.template.meta.json")?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Formats the current wall-clock time as RFC 3339 UTC with second
/// precision (e.g. `2026-08-18T09:00:00Z`), without a datetime dependency.
fn rfc3339_now() -> String {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_rfc3339_utc(since_epoch.as_secs())
}

fn format_rfc3339_utc(total_secs: u64) -> String {
    let days = total_secs / 86_400;
    let secs_of_day = total_secs % 86_400;
    let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(0));
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Converts a day count since the Unix epoch (1970-01-01) into a
/// proleptic-Gregorian `(year, month, day)` triple. Howard Hinnant's
/// `civil_from_days` algorithm (public domain; see
/// <https://howardhinnant.github.io/date_algorithms.html>).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        u32::try_from(m).unwrap_or_default(),
        u32::try_from(d).unwrap_or_default(),
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::carriers::OnDecl;

    fn entry(index: u8, hook_fn: &str, on: OnDecl, hook_name: Option<&str>) -> EntryDecl {
        EntryDecl {
            index,
            hook_fn: hook_fn.to_string(),
            cbak_fn: None,
            hook_name: hook_name.map(str::to_string),
            on,
            hook_can_emit: None,
            description: None,
            sig_params: None,
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
    fn validate_namespace_accepts_exact_64_hex_and_uppercases() {
        let mixed = "aB".repeat(32);
        assert_eq!(mixed.len(), 64);
        let result = validate_namespace(&mixed).expect("valid namespace");
        assert_eq!(result, mixed.to_ascii_uppercase());
    }

    #[test]
    fn validate_namespace_rejects_wrong_length() {
        assert!(validate_namespace(&"AB".repeat(31)).is_err());
        assert!(validate_namespace(&"AB".repeat(33)).is_err());
        assert!(validate_namespace("").is_err());
    }

    #[test]
    fn validate_namespace_rejects_non_hex_chars() {
        let mut s = "0".repeat(64);
        s.replace_range(0..1, "g");
        assert!(validate_namespace(&s).is_err());
    }

    #[test]
    fn validate_account_accepts_plausible_address() {
        assert_eq!(
            validate_account("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh").expect("valid account"),
            "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh"
        );
    }

    #[test]
    fn validate_account_rejects_missing_r_prefix() {
        assert!(validate_account("xHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh").is_err());
        assert!(validate_account("").is_err());
    }

    #[test]
    fn validate_account_rejects_excluded_base58_chars() {
        assert!(validate_account("r0b9CJAWyB4rj91VRWn96DkukG4bwdtyTh").is_err());
        assert!(validate_account("rOb9CJAWyB4rj91VRWn96DkukG4bwdtyTh").is_err());
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(30), (1970, 1, 31));
        assert_eq!(civil_from_days(31), (1970, 2, 1));
        // 2000-03-01 is a well-known reference point for this algorithm.
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
    }

    #[test]
    fn template_fills_gaps_exactly_empty_and_declared_entries_in_order() {
        let e0 = entry(0, "deposit", omitted_on(), None);
        let e2 = entry(2, "sweep", omitted_on(), None);
        let wasm0 = b"AA".as_slice();
        let wasm2 = b"BB".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e0, wasm0), (2, &e2, wasm2)];

        let bytes =
            build_template_json(&declared, &[], None, None, false).expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");

        let hooks = value["Hooks"].as_array().expect("Hooks array");
        assert_eq!(hooks.len(), 3);
        assert_eq!(hooks[1], serde_json::json!({"Hook": {}}));
        assert_eq!(hooks[0]["Hook"]["CreateCode"], "4141");
        assert_eq!(hooks[2]["Hook"]["CreateCode"], "4242");
        assert!(hooks[1]["Hook"].as_object().expect("gap object").is_empty());
        assert_eq!(value["Account"], "<ACCOUNT>");
        assert_eq!(hooks[0]["Hook"]["HookNamespace"], "<NAMESPACE>");
    }

    #[test]
    fn override_flag_only_touches_declared_entries_never_gaps() {
        let e0 = entry(0, "deposit", omitted_on(), None);
        let wasm0 = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e0, wasm0)];
        // Force a gap by declaring only index 1.
        let e1 = entry(1, "sweep", omitted_on(), None);
        let wasm1 = b"BB".as_slice();
        let declared_with_gap: Vec<TemplateInput<'_>> = vec![(1, &e1, wasm1)];

        let bytes = build_template_json(
            &declared,
            &[],
            Some("rACCOUNT"),
            Some("00".repeat(32).as_str()),
            true,
        )
        .expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(value["Hooks"][0]["Hook"]["Flags"], 1);
        assert_eq!(value["Account"], "rACCOUNT");

        let bytes = build_template_json(&declared_with_gap, &[], None, None, true)
            .expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert!(
            value["Hooks"][0]["Hook"]
                .as_object()
                .expect("gap object")
                .is_empty()
        );
        assert_eq!(value["Hooks"][1]["Hook"]["Flags"], 1);
    }

    #[test]
    fn can_emit_three_states_round_trip_into_wire_field() {
        let mut declared_none = entry(0, "a", omitted_on(), None);
        declared_none.hook_can_emit = None;
        let mut declared_empty = entry(0, "b", omitted_on(), None);
        declared_empty.hook_can_emit = Some(Vec::new());
        let mut declared_list = entry(0, "c", omitted_on(), None);
        declared_list.hook_can_emit = Some(vec!["Payment".to_string()]);

        for (e, expect_field) in [
            (&declared_none, false),
            (&declared_empty, true),
            (&declared_list, true),
        ] {
            let wasm = b"AA".as_slice();
            let declared: Vec<TemplateInput<'_>> = vec![(0, e, wasm)];
            let bytes =
                build_template_json(&declared, &[], None, None, false).expect("template builds");
            let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
            assert_eq!(
                value["Hooks"][0]["Hook"].get("HookCanEmit").is_some(),
                expect_field
            );
        }
    }

    #[test]
    fn trigger_forms_map_to_expected_wire_fields() {
        let all_on = OnDecl {
            form: "all".to_string(),
            hook_on: None,
            hook_on_incoming: None,
            hook_on_outgoing: None,
        };
        let directional_on = OnDecl {
            form: "directional".to_string(),
            hook_on: None,
            hook_on_incoming: Some(vec!["Payment".to_string()]),
            hook_on_outgoing: Some(Vec::new()),
        };

        for (on, expect_hook_on, expect_directional) in [
            (omitted_on(), false, false),
            (all_on, true, false),
            (directional_on, false, true),
        ] {
            let e = entry(0, "a", on, None);
            let wasm = b"AA".as_slice();
            let declared: Vec<TemplateInput<'_>> = vec![(0, &e, wasm)];
            let bytes =
                build_template_json(&declared, &[], None, None, false).expect("template builds");
            let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
            let hook = &value["Hooks"][0]["Hook"];
            assert_eq!(hook.get("HookOn").is_some(), expect_hook_on);
            assert_eq!(hook.get("HookOnIncoming").is_some(), expect_directional);
            assert_eq!(hook.get("HookOnOutgoing").is_some(), expect_directional);
        }
    }

    #[test]
    fn hook_name_only_present_when_declared() {
        let named = entry(0, "a", omitted_on(), Some("dep"));
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &named, wasm)];
        let bytes =
            build_template_json(&declared, &[], None, None, false).expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(value["Hooks"][0]["Hook"]["HookName"], "646570");
    }

    #[test]
    fn required_amendments_covers_hooks_always_and_derived_fields_conditionally() {
        let plain = entry(0, "a", omitted_on(), None);
        assert_eq!(
            required_amendments(std::slice::from_ref(&plain)),
            vec!["Hooks"]
        );

        let mut named = plain.clone();
        named.hook_name = Some("dep".to_string());
        assert_eq!(required_amendments(&[named]), vec!["Hooks", "NamedHooks"]);

        let directional = entry(
            0,
            "a",
            OnDecl {
                form: "directional".to_string(),
                hook_on: None,
                hook_on_incoming: Some(vec![]),
                hook_on_outgoing: Some(vec![]),
            },
            None,
        );
        assert_eq!(
            required_amendments(&[directional]),
            vec!["Hooks", "HookOnV2"]
        );

        let mut can_emit = plain;
        can_emit.hook_can_emit = Some(vec!["Payment".to_string()]);
        assert_eq!(
            required_amendments(&[can_emit]),
            vec!["Hooks", "HookCanEmit"]
        );
    }

    #[test]
    fn positions_and_required_amendments_serialize_into_meta_json() {
        let mut hashes = BTreeMap::new();
        hashes.insert(0u8, "AA".repeat(32));
        hashes.insert(2u8, "BB".repeat(32));

        let bytes = build_template_meta(
            "vault",
            "0.2.0",
            &hashes,
            &[0, 2],
            &[1],
            2,
            &["Hooks".to_string(), "HookOnV2".to_string()],
        )
        .expect("meta builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");

        assert_eq!(value["crate"], "vault");
        assert_eq!(value["version"], "0.2.0");
        assert_eq!(value["hook_hashes"]["0"], "AA".repeat(32));
        assert_eq!(value["hook_hashes"]["2"], "BB".repeat(32));
        assert_eq!(value["positions"]["declared"], serde_json::json!([0, 2]));
        assert_eq!(value["positions"]["gaps"], serde_json::json!([1]));
        assert_eq!(value["positions"]["untouched_beyond"], 3);
        assert_eq!(
            value["required_amendments"],
            serde_json::json!(["Hooks", "HookOnV2"])
        );
        assert!(
            value["generated_at"]
                .as_str()
                .is_some_and(|s| s.ends_with('Z') && s.contains('T'))
        );
    }

    // --- `HookParameters` (docs/PARAM_SIGNATURE_DESIGN.md §4) ---

    fn sig_param(field: &str, type_byte: u8, name_hex: &str) -> SigParamDecl {
        SigParamDecl {
            field: field.to_string(),
            type_byte,
            name_hex: name_hex.to_string(),
        }
    }

    #[test]
    fn entry_without_sig_params_emits_no_hook_parameters_key() {
        let e = entry(0, "deposit", omitted_on(), None);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e, wasm)];
        let bytes =
            build_template_json(&declared, &[], None, None, false).expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert!(value["Hooks"][0]["Hook"].get("HookParameters").is_none());
    }

    #[test]
    fn entry_with_sig_params_emits_hook_parameters_in_index_order_with_zero_value() {
        let mut e = entry(0, "increment", omitted_on(), None);
        e.sig_params = Some(vec![
            sig_param("account", 0x08, "5F5053000008076163636F756E74"),
            sig_param("count", 0x01, "5F505300010105636F756E74"),
        ]);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e, wasm)];
        let bytes =
            build_template_json(&declared, &[], None, None, false).expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        let params = value["Hooks"][0]["Hook"]["HookParameters"]
            .as_array()
            .expect("HookParameters array");
        assert_eq!(params.len(), 2);
        assert_eq!(
            params[0]["HookParameter"]["HookParameterName"],
            "5F5053000008076163636F756E74"
        );
        assert_eq!(params[0]["HookParameter"]["HookParameterValue"], "00");
        assert_eq!(
            params[1]["HookParameter"]["HookParameterName"],
            "5F505300010105636F756E74"
        );
        assert_eq!(params[1]["HookParameter"]["HookParameterValue"], "00");
    }

    #[test]
    fn hook_parameters_key_sits_between_hook_api_version_and_hook_name() {
        let mut e = entry(0, "increment", omitted_on(), Some("inc"));
        e.sig_params = Some(vec![sig_param("count", 0x01, "5F505300010105636F756E74")]);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e, wasm)];
        let bytes =
            build_template_json(&declared, &[], None, None, false).expect("template builds");
        let text = String::from_utf8(bytes).expect("utf8");
        let api_version_pos = text
            .find("\"HookApiVersion\"")
            .expect("HookApiVersion present");
        let params_pos = text
            .find("\"HookParameters\"")
            .expect("HookParameters present");
        let name_pos = text.find("\"HookName\"").expect("HookName present");
        assert!(api_version_pos < params_pos);
        assert!(params_pos < name_pos);
    }

    #[test]
    fn mixed_chain_gap_and_no_sig_and_sig_entries_only_the_sig_entry_gets_hook_parameters() {
        // A gap at 1, a plain entry at 0, a sig-param entry at 2 — the gap
        // must stay exactly `{"Hook": {}}` and the plain entry must not
        // gain `HookParameters` just because a sibling entry has one.
        let e0 = entry(0, "deposit", omitted_on(), None);
        let mut e2 = entry(2, "increment", omitted_on(), None);
        e2.sig_params = Some(vec![sig_param("count", 0x01, "5F505300010105636F756E74")]);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e0, wasm), (2, &e2, wasm)];
        let bytes =
            build_template_json(&declared, &[], None, None, false).expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");

        assert!(
            value["Hooks"][1]["Hook"]
                .as_object()
                .expect("gap object")
                .is_empty()
        );
        assert!(value["Hooks"][0]["Hook"].get("HookParameters").is_none());
        assert_eq!(
            value["Hooks"][2]["Hook"]["HookParameters"]
                .as_array()
                .expect("HookParameters array")
                .len(),
            1
        );
    }

    // --- `HookParameters` (docs/STATE_INTERFACE_DESIGN.md §5) ---

    fn si_decl(field: &str, id: u8, name_hex: &str, value_hex: &str) -> SiDecl {
        SiDecl {
            field: field.to_string(),
            id,
            name_hex: name_hex.to_string(),
            value_hex: value_hex.to_string(),
            key: "(account: AccountId, token: u32)".to_string(),
            value: "(amount: u64, updated: u32)".to_string(),
        }
    }

    #[test]
    fn si_declarations_are_chain_level_and_emitted_on_every_declared_entry() {
        // Two declared entries, neither with sig_params — the same chain-level
        // `si_decls` list must appear on both.
        let e0 = entry(0, "a", omitted_on(), None);
        let e1 = entry(1, "b", omitted_on(), None);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e0, wasm), (1, &e1, wasm)];
        let si = vec![si_decl(
            "balances",
            0,
            "5F534900000208076163636F756E740205746F6B656E",
            "020306616D6F756E74020775706461746564",
        )];
        let bytes =
            build_template_json(&declared, &si, None, None, false).expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");

        for i in 0..2 {
            let params = value["Hooks"][i]["Hook"]["HookParameters"]
                .as_array()
                .expect("HookParameters array");
            assert_eq!(params.len(), 1);
            assert_eq!(
                params[0]["HookParameter"]["HookParameterName"],
                "5F534900000208076163636F756E740205746F6B656E"
            );
            assert_eq!(
                params[0]["HookParameter"]["HookParameterValue"],
                "020306616D6F756E74020775706461746564"
            );
        }
    }

    #[test]
    fn si_declarations_follow_sig_param_declarations_on_the_same_entry() {
        let mut e = entry(0, "increment", omitted_on(), None);
        e.sig_params = Some(vec![sig_param("count", 0x01, "5F5F015F015F636F756E74")]);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e, wasm)];
        let si = vec![si_decl("balances", 0, "AABB", "CCDD")];
        let bytes =
            build_template_json(&declared, &si, None, None, false).expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");

        let params = value["Hooks"][0]["Hook"]["HookParameters"]
            .as_array()
            .expect("HookParameters array");
        assert_eq!(params.len(), 2);
        assert_eq!(
            params[0]["HookParameter"]["HookParameterName"],
            "5F5F015F015F636F756E74"
        );
        assert_eq!(params[0]["HookParameter"]["HookParameterValue"], "00");
        assert_eq!(params[1]["HookParameter"]["HookParameterName"], "AABB");
        assert_eq!(params[1]["HookParameter"]["HookParameterValue"], "CCDD");
    }

    #[test]
    fn no_sig_params_and_no_si_decls_emits_no_hook_parameters_key() {
        let e = entry(0, "deposit", omitted_on(), None);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e, wasm)];
        let bytes =
            build_template_json(&declared, &[], None, None, false).expect("template builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert!(value["Hooks"][0]["Hook"].get("HookParameters").is_none());
    }

    /// `n` distinct single-value-field SI declarations, `id`s `0..n`.
    fn si_decls(n: u8) -> Vec<SiDecl> {
        (0..n)
            .map(|i| si_decl(&format!("f{i}"), i, &format!("{i:02X}"), "00"))
            .collect()
    }

    #[test]
    fn sixteen_combined_hook_parameters_is_the_boundary_that_still_builds() {
        let mut e = entry(0, "deposit", omitted_on(), None);
        e.sig_params = Some(vec![sig_param("count", 0x01, "5F5F015F015F636F756E74")]);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e, wasm)];
        // 1 sig param + 15 SI declarations = 16, exactly at the limit.
        let si = si_decls(15);
        let bytes = build_template_json(&declared, &si, None, None, false).expect("16 builds");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(
            value["Hooks"][0]["Hook"]["HookParameters"]
                .as_array()
                .expect("HookParameters array")
                .len(),
            16
        );
    }

    #[test]
    fn seventeen_combined_hook_parameters_is_rejected_naming_entry_counts_and_limit() {
        let mut e = entry(0, "deposit", omitted_on(), None);
        e.sig_params = Some(vec![sig_param("count", 0x01, "5F5F015F015F636F756E74")]);
        let wasm = b"AA".as_slice();
        let declared: Vec<TemplateInput<'_>> = vec![(0, &e, wasm)];
        // 1 sig param + 16 SI declarations = 17, one over the limit.
        let si = si_decls(16);
        let err = build_template_json(&declared, &si, None, None, false).expect_err("must fail");
        let message = format!("{err:#}");
        assert!(message.contains("entry 0"), "{message}");
        assert!(message.contains("17"), "{message}");
        assert!(message.contains("16-per-entry"), "{message}");
    }
}
