//! Build-time Hook metadata extraction and sidecar generation.
//!
//! Hook crates carry source metadata in the name of a deliberately dead wasm
//! export. The cleaner removes that carrier from the deployable module; this
//! module reads it from cargo's raw artifact first, then combines it with facts
//! derived from the final cleaned bytes.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use sha2::{Digest, Sha512};

use crate::ValidationReport;

/// Prefix used by `metadata!` carrier exports in raw Hook wasm artifacts.
pub const METADATA_EXPORT_PREFIX: &str = "__rshooks_metadata_v1_";

/// Canonical Xahau JSON spellings emitted by the current `metadata!` macro
/// for every known `TxType` variant (excluding the data-carrying `Unknown`).
pub(crate) const TRANSACTION_TYPES: &[&str] = &[
    "Payment",
    "EscrowCreate",
    "EscrowFinish",
    "AccountSet",
    "EscrowCancel",
    "SetRegularKey",
    "OfferCreate",
    "OfferCancel",
    "TicketCreate",
    "SignerListSet",
    "PaymentChannelCreate",
    "PaymentChannelFund",
    "PaymentChannelClaim",
    "CheckCreate",
    "CheckCash",
    "CheckCancel",
    "DepositPreauth",
    "TrustSet",
    "AccountDelete",
    "SetHook",
    "NFTokenMint",
    "NFTokenBurn",
    "NFTokenCreateOffer",
    "NFTokenCancelOffer",
    "NFTokenAcceptOffer",
    "Clawback",
    "AMMClawback",
    "AMMCreate",
    "AMMDeposit",
    "AMMWithdraw",
    "AMMVote",
    "AMMBid",
    "AMMDelete",
    "URITokenMint",
    "URITokenBurn",
    "URITokenBuy",
    "URITokenCreateSellOffer",
    "URITokenCancelSellOffer",
    "XChainCreateClaimID",
    "XChainCommit",
    "XChainClaim",
    "XChainAccountCreateCommit",
    "XChainAddClaimAttestation",
    "XChainAddAccountCreateAttestation",
    "XChainModifyBridge",
    "XChainCreateBridge",
    "DIDSet",
    "DIDDelete",
    "OracleSet",
    "OracleDelete",
    "LedgerStateFix",
    "MPTokenIssuanceCreate",
    "MPTokenIssuanceDestroy",
    "MPTokenIssuanceSet",
    "MPTokenAuthorize",
    "CredentialCreate",
    "CredentialAccept",
    "CredentialDelete",
    "NFTokenModify",
    "PermissionedDomainSet",
    "PermissionedDomainDelete",
    "Cron",
    "CronSet",
    "SetRemarks",
    "Remit",
    "GenesisMint",
    "Import",
    "ClaimReward",
    "Invoke",
    "EnableAmendment",
    "SetFee",
    "UNLModify",
    "EmitFailure",
    "UNLReport",
];

/// `tt*` codes corresponding position-for-position to [`TRANSACTION_TYPES`].
pub(crate) const TRANSACTION_TYPE_CODES: &[u8] = &[
    0, 1, 2, 3, 4, 5, 7, 8, 10, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 25, 26, 27, 28, 29, 30,
    31, 35, 36, 37, 38, 39, 40, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103,
    104,
];

/// Source-authored metadata recovered from a `metadata!` carrier export.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookMetadata {
    /// Human-readable Hook name.
    pub name: String,
    /// Optional longer description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Transaction types which trigger the Hook in both directions.
    #[serde(default, rename = "HookOn", skip_serializing_if = "Option::is_none")]
    pub hook_on: Option<Vec<String>>,
    /// Transaction types which trigger the Hook for incoming transactions.
    #[serde(
        default,
        rename = "IncomingHookOn",
        skip_serializing_if = "Option::is_none"
    )]
    pub incoming_hook_on: Option<Vec<String>>,
    /// Transaction types which trigger the Hook for outgoing transactions.
    #[serde(
        default,
        rename = "OutgoingHookOn",
        skip_serializing_if = "Option::is_none"
    )]
    pub outgoing_hook_on: Option<Vec<String>>,
    /// Optional transaction types the Hook declares it may emit.
    #[serde(
        default,
        rename = "HookCanEmit",
        skip_serializing_if = "Option::is_none"
    )]
    pub hook_can_emit: Option<Vec<String>>,
    /// Optional UTF-8 name placed in SetHook's `HookName` field.
    #[serde(default, rename = "HookName", skip_serializing_if = "Option::is_none")]
    pub hook_name: Option<String>,
}

impl HookMetadata {
    fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            bail!("metadata field `name` must not be empty");
        }

        match (
            self.hook_on.is_some(),
            self.incoming_hook_on.is_some(),
            self.outgoing_hook_on.is_some(),
        ) {
            (true, false, false) | (false, true, true) | (false, false, false) => {}
            _ => bail!(
                "metadata must contain either `HookOn` or both `IncomingHookOn` and \
                 `OutgoingHookOn`, but never both forms"
            ),
        }

        validate_transaction_types("HookOn", self.hook_on.as_deref())?;
        validate_transaction_types("IncomingHookOn", self.incoming_hook_on.as_deref())?;
        validate_transaction_types("OutgoingHookOn", self.outgoing_hook_on.as_deref())?;
        validate_transaction_types("HookCanEmit", self.hook_can_emit.as_deref())?;

        if let (Some(incoming), Some(outgoing)) = (&self.incoming_hook_on, &self.outgoing_hook_on) {
            let incoming: BTreeSet<&str> = incoming.iter().map(String::as_str).collect();
            let outgoing: BTreeSet<&str> = outgoing.iter().map(String::as_str).collect();
            if incoming == outgoing {
                bail!(
                    "metadata fields `IncomingHookOn` and `OutgoingHookOn` must not contain the \
                     same transaction-type set; use `HookOn` instead"
                );
            }
        }

        if let Some(name) = &self.hook_name {
            let char_count = name.chars().count();
            if !(2..=8).contains(&char_count) {
                bail!("metadata field `HookName` must contain 2 to 8 UTF-8 characters");
            }
        }

        Ok(())
    }
}

pub(crate) fn validate_transaction_types(field: &str, values: Option<&[String]>) -> Result<()> {
    let Some(values) = values else {
        return Ok(());
    };
    let mut seen = BTreeSet::new();
    for value in values {
        if value.is_empty() {
            bail!("metadata field `{field}` contains an empty transaction type");
        }
        if !TRANSACTION_TYPES.contains(&value.as_str()) {
            bail!("metadata field `{field}` contains unknown transaction type `{value}`");
        }
        if !seen.insert(value) {
            bail!("metadata field `{field}` contains duplicate transaction type `{value}`");
        }
    }
    Ok(())
}

/// Worst-case instruction counts for the Hook entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WorstCaseExecution {
    /// Static WCE for `hook`, or `null` for gas-type Hooks.
    pub hook: Option<u64>,
    /// Static WCE for `cbak`, or `null` for gas-type Hooks.
    pub cbak: Option<u64>,
}

/// Toolchain provenance for a sidecar, recorded so a build can be
/// reproduced deterministically even after tool updates change
/// optimization behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuilderInfo {
    /// Package name of the tool that produced this sidecar; always
    /// `"rshooks-build"`.
    pub name: String,
    /// Package version of the tool that produced this sidecar.
    pub version: String,
    /// Full first line of `rustc -V` from the toolchain that performed the
    /// build, or `None` if it could not be determined. Detection failures
    /// never fail the build.
    pub rustc: Option<String>,
}

impl BuilderInfo {
    /// Builds this package's provenance record. `rustc` is the
    /// already-detected `rustc -V` first line (detection itself spawns a
    /// process, so it lives in the CLI, not here).
    #[must_use]
    pub fn current(rustc: Option<String>) -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            rustc,
        }
    }
}

/// Complete JSON sidecar document written next to a built Hook wasm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataDocument {
    /// Metadata authored in the Hook crate.
    pub metadata: HookMetadata,
    /// SHA512-Half of the exact final cleaned wasm bytes.
    pub hook_hash: String,
    /// Worst-case instruction counts, when statically available.
    pub wce: WorstCaseExecution,
    /// Toolchain provenance for reproducing this exact build.
    pub builder: BuilderInfo,
}

impl Serialize for MetadataDocument {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &self.metadata.name)?;
        if let Some(description) = &self.metadata.description {
            map.serialize_entry("description", description)?;
        }
        match (
            self.metadata.hook_on.as_deref(),
            self.metadata.incoming_hook_on.as_deref(),
            self.metadata.outgoing_hook_on.as_deref(),
        ) {
            (Some(hook_on), None, None) => {
                let mask = hook_mask(Some(hook_on)).map_err(serde::ser::Error::custom)?;
                map.serialize_entry("HookOn", &mask)?;
            }
            (None, Some(incoming), Some(outgoing)) => {
                let incoming_mask = hook_mask(Some(incoming)).map_err(serde::ser::Error::custom)?;
                let outgoing_mask = hook_mask(Some(outgoing)).map_err(serde::ser::Error::custom)?;
                map.serialize_entry("HookOnIncoming", &incoming_mask)?;
                map.serialize_entry("HookOnOutgoing", &outgoing_mask)?;
            }
            (None, None, None) => map.serialize_entry("HookOn", &Option::<String>::None)?,
            // `HookMetadata::validate` rejects every other combination before a
            // document is constructed.
            _ => unreachable!("invalid HookOn metadata passed to sidecar serializer"),
        }
        map.serialize_entry(
            "HookCanEmit",
            &hook_mask(self.metadata.hook_can_emit.as_deref())
                .map_err(serde::ser::Error::custom)?,
        )?;
        map.serialize_entry(
            "HookName",
            &self.metadata.hook_name.as_deref().map(utf8_hex),
        )?;
        map.serialize_entry("HookHash", &self.hook_hash)?;
        map.serialize_entry("WCE", &self.wce)?;
        map.serialize_entry("builder", &self.builder)?;
        map.serialize_entry("human", &HumanMetadata::from(&self.metadata))?;
        map.end()
    }
}

/// Readable source values corresponding to the raw SetHook fields above.
#[derive(Serialize)]
struct HumanMetadata<'a> {
    #[serde(rename = "HookOn", skip_serializing_if = "Option::is_none")]
    hook_on: Option<&'a [String]>,
    #[serde(rename = "HookOnIncoming", skip_serializing_if = "Option::is_none")]
    incoming_hook_on: Option<&'a [String]>,
    #[serde(rename = "HookOnOutgoing", skip_serializing_if = "Option::is_none")]
    outgoing_hook_on: Option<&'a [String]>,
    #[serde(rename = "HookCanEmit")]
    hook_can_emit: Option<&'a [String]>,
    #[serde(rename = "HookName")]
    hook_name: Option<&'a str>,
}

impl<'a> From<&'a HookMetadata> for HumanMetadata<'a> {
    fn from(metadata: &'a HookMetadata) -> Self {
        Self {
            hook_on: metadata.hook_on.as_deref(),
            incoming_hook_on: metadata.incoming_hook_on.as_deref(),
            outgoing_hook_on: metadata.outgoing_hook_on.as_deref(),
            hook_can_emit: metadata.hook_can_emit.as_deref(),
            hook_name: metadata.hook_name.as_deref(),
        }
    }
}

/// Encodes Xahau's inverted transaction-type bitmask used by HookOn and
/// HookCanEmit. An omitted declaration is the all-zero protocol value and is
/// represented as `null` in the sidecar.
pub(crate) fn hook_mask(values: Option<&[String]>) -> Result<Option<String>> {
    let Some(values) = values else {
        return Ok(None);
    };
    let mut bytes = [u8::MAX; 32];
    bytes[29] = 0xBF;

    for value in values {
        let code = TRANSACTION_TYPES
            .iter()
            .zip(TRANSACTION_TYPE_CODES)
            .find_map(|(known, code)| (*known == value).then_some(*code))
            .context("validated transaction type is in the canonical table")?;
        let byte_index = 31usize
            .checked_sub(usize::from(code) / 8)
            .context("transaction code must fit the HookOn bitmask")?;
        let byte = bytes
            .get_mut(byte_index)
            .context("transaction code must fit the HookOn bitmask")?;
        *byte ^= 1u8 << (code % 8);
    }

    if bytes.iter().all(|byte| *byte == 0) {
        return Ok(None);
    }
    Ok(Some(
        bytes.iter().map(|byte| format!("{byte:02X}")).collect(),
    ))
}

pub(crate) fn utf8_hex(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

/// Generated metadata document and non-fatal source/binary consistency warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataBuild {
    /// The serializable sidecar document.
    pub document: MetadataDocument,
    /// Warnings comparing `HookCanEmit` to final reachable `env::emit` use.
    pub warnings: Vec<String>,
}

/// Extracts a single metadata carrier from a raw cargo-produced wasm artifact.
///
/// Returns `Ok(None)` for Hooks that do not use `metadata!`. Multiple carriers,
/// non-function carriers, malformed uppercase hex, invalid JSON, and invalid
/// metadata field combinations are hard errors.
pub fn extract_metadata(wasm: &[u8]) -> Result<Option<HookMetadata>> {
    let mut metadata = None;

    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let payload = payload.context("parsing raw wasm while looking for Hook metadata")?;
        let wasmparser::Payload::ExportSection(reader) = payload else {
            continue;
        };
        for export in reader {
            let export = export.context("reading raw wasm export while looking for metadata")?;
            let Some(encoded) = export.name.strip_prefix(METADATA_EXPORT_PREFIX) else {
                continue;
            };
            if metadata.is_some() {
                bail!("raw wasm contains multiple Hook metadata carrier exports");
            }
            if export.kind != wasmparser::ExternalKind::Func {
                bail!("Hook metadata carrier export must refer to a function");
            }

            let json = decode_upper_hex(encoded).context("decoding Hook metadata carrier")?;
            let parsed: HookMetadata =
                serde_json::from_slice(&json).context("parsing JSON from Hook metadata carrier")?;
            parsed.validate().context("validating Hook metadata")?;
            metadata = Some(parsed);
        }
    }

    Ok(metadata)
}

/// Creates the final sidecar document from source metadata and final wasm
/// facts. `rustc` is the already-detected `rustc -V` first line for the
/// [`BuilderInfo`] provenance record (`None` if detection failed or wasn't
/// attempted).
pub fn build_metadata(
    metadata: HookMetadata,
    final_wasm: &[u8],
    report: &ValidationReport,
    rustc: Option<String>,
) -> Result<MetadataBuild> {
    let uses_emit = uses_reachable_emit(final_wasm)?;
    let mut warnings = Vec::new();
    match (metadata.hook_can_emit.is_some(), uses_emit) {
        (true, false) => warnings.push(
            "metadata specifies `HookCanEmit`, but the final wasm does not use the `emit` API"
                .to_string(),
        ),
        (false, true) => warnings.push(
            "the final wasm uses the `emit` API, but metadata does not specify `HookCanEmit`"
                .to_string(),
        ),
        _ => {}
    }

    if let Some(name) = &metadata.hook_name {
        let byte_count = name.len();
        if !(4..=16).contains(&byte_count) {
            warnings.push(format!(
                "metadata `HookName` is {byte_count} UTF-8 bytes, but the current Xahau protocol \
                 requires 4 to 16 bytes"
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

    Ok(MetadataBuild {
        document: MetadataDocument {
            metadata,
            hook_hash: hook_hash(final_wasm),
            wce,
            builder: BuilderInfo::current(rustc),
        },
        warnings,
    })
}

/// Computes Xahau's HookHash: the uppercase first 32 bytes of SHA-512.
#[must_use]
pub fn hook_hash(wasm: &[u8]) -> String {
    let digest = Sha512::digest(wasm);
    let mut out = String::with_capacity(64);
    for byte in digest.iter().take(32) {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02X}");
    }
    out
}

pub(crate) fn uses_reachable_emit(wasm: &[u8]) -> Result<bool> {
    let module = crate::ir::parse(wasm).context("parsing final wasm for `emit` usage")?;
    Ok(module.find_func_import("env", "emit").is_some())
}

pub(crate) fn decode_upper_hex(encoded: &str) -> Result<Vec<u8>> {
    if encoded.is_empty() {
        bail!("metadata carrier payload is empty");
    }
    if encoded.len() % 2 != 0 {
        bail!("metadata carrier payload has an odd number of hex digits");
    }

    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = pair
            .first()
            .copied()
            .context("metadata carrier hex pair is missing its first digit")?;
        let low = pair
            .get(1)
            .copied()
            .context("metadata carrier hex pair is missing its second digit")?;
        decoded.push((upper_hex_value(high)? << 4) | upper_hex_value(low)?);
    }
    Ok(decoded)
}

fn upper_hex_value(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("metadata carrier contains non-uppercase-hex byte 0x{byte:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_transaction_type_whitelist_has_all_current_variants() {
        assert_eq!(TRANSACTION_TYPES.len(), 74);
        assert_eq!(TRANSACTION_TYPES.len(), TRANSACTION_TYPE_CODES.len());
        assert_eq!(
            TRANSACTION_TYPES
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len(),
            TRANSACTION_TYPES.len(),
            "canonical TransactionType names must be unique"
        );
    }
}
