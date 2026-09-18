//! Shared metadata facts reused by the v2 `#[hooks]` build path, plus the
//! legacy `metadata!` (v1) carrier export-name prefix — kept only so
//! [`crate::carriers`] can detect a stale v1-only crate and report a
//! migration hint (see `docs/MULTI_HOOK_STRUCT_DESIGN.md` §7).

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha512};

use crate::tx_type_table::{TRANSACTION_TYPE_CODES, TRANSACTION_TYPES};

/// Prefix used by `metadata!` (v1) carrier exports in raw Hook wasm
/// artifacts. Detected — never parsed — so a crate that has not migrated to
/// `#[hooks]` gets a migration hint instead of a generic "no chain found"
/// error.
pub const METADATA_EXPORT_PREFIX: &str = "__rshooks_metadata_v1_";

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
    /// Static WCE for `hook`.
    pub hook: Option<u64>,
    /// Static WCE for `cbak`.
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
    Ok(Some(encode_upper_hex(&bytes)))
}

pub(crate) fn utf8_hex(value: &str) -> String {
    encode_upper_hex(value.as_bytes())
}

/// Computes Xahau's HookHash: the uppercase first 32 bytes of SHA-512.
#[must_use]
pub fn hook_hash(wasm: &[u8]) -> String {
    let digest = Sha512::digest(wasm);
    let bytes: Vec<u8> = digest.iter().take(32).copied().collect();
    encode_upper_hex(&bytes)
}

pub(crate) fn uses_reachable_emit(wasm: &[u8]) -> Result<bool> {
    let module = crate::ir::parse(wasm).context("parsing final wasm for `emit` usage")?;
    Ok(module.find_func_import("env", "emit").is_some())
}

/// Encodes `bytes` as an uppercase hex string, two digits per byte.
pub(crate) fn encode_upper_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

pub(crate) fn decode_upper_hex(encoded: &str) -> Result<Vec<u8>> {
    if encoded.is_empty() {
        bail!("metadata carrier payload is empty");
    }
    if encoded.len() % 2 != 0 {
        bail!("metadata carrier payload has an odd number of hex digits");
    }
    if let Some(bad) = encoded
        .bytes()
        .find(|b| !matches!(b, b'0'..=b'9' | b'A'..=b'F'))
    {
        bail!("metadata carrier contains non-uppercase-hex byte 0x{bad:02X}");
    }

    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            // Every byte was already verified ASCII uppercase-hex above.
            let pair = str::from_utf8(pair).context("internal error: hex pair is not ASCII")?;
            u8::from_str_radix(pair, 16)
                .context("internal error: verified hex pair failed to parse")
        })
        .collect()
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
