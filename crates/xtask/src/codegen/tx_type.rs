//! Generates the typed `rshooks::TxType` model from `tts.h` constants.

use std::fmt::Write as _;

use anyhow::{Context, Result, anyhow};

use super::with_generated_marker;
use crate::ir::ConstSpec;
use crate::render::expect_decimal;

const MODULE_DOC: &str = "\
//! Transaction type (`TxType`) model.
//!
//! [`TxType`] is a typed, exhaustive-by-construction mirror of the raw
//! `tt*` transaction-type codes in `rshooks_core::tts` (plus
//! [`TxType::Unknown`] for forward-compatibility with codes this crate
//! does not yet know about) — the same pattern [`crate::error::HookError`]
//! uses for the Hook API's negative error-code channel, applied here to
//! [`crate::api::otxn::otxn_type`]'s `u16` transaction-type channel.
";

/// Converts a C `tt*` constant name (e.g. `ttNFTOKEN_MINT`) into Xahau's
/// canonical Rust spelling (`NFTokenMint`). Also used by
/// [`super::tx_type_table`], whose build-side name table must use the exact
/// same spellings as this typed enum.
pub(super) fn variant_name(const_name: &str) -> Result<String> {
    let rest = const_name
        .strip_prefix("tt")
        .ok_or_else(|| anyhow!("expected a `tt`-prefixed name, got `{const_name}`"))?;
    let mut out = String::new();
    for part in rest.split('_') {
        let word = match part {
            "NFTOKEN" => "NFToken".to_owned(),
            "UNL" | "ID" | "AMM" | "URI" | "DID" => part.to_owned(),
            "XCHAIN" => "XChain".to_owned(),
            "URITOKEN" => "URIToken".to_owned(),
            "MPTOKEN" => "MPToken".to_owned(),
            _ => {
                let mut chars = part.chars();
                let mut word = String::new();
                if let Some(first) = chars.next() {
                    word.extend(first.to_uppercase());
                    for c in chars {
                        word.extend(c.to_lowercase());
                    }
                }
                word
            }
        };
        out.push_str(&word);
    }
    Ok(translate_tt(out))
}

/// Applies Xahau transaction-type names whose canonical spelling cannot be
/// derived from the `tt*` constant alone.
fn translate_tt(inp: String) -> String {
    match inp.as_str() {
        "Amendment" => "EnableAmendment",
        "Fee" => "SetFee",
        "PaychanClaim" => "PaymentChannelClaim",
        "PaychanCreate" => "PaymentChannelCreate",
        "PaychanFund" => "PaymentChannelFund",
        "RegularKeySet" => "SetRegularKey",
        "HookSet" => "SetHook",
        "RemarksSet" => "SetRemarks",
        _ => return inp,
    }
    .to_owned()
}

/// Renders `tx_type.rs`'s full contents from `tts.h`'s parsed
/// [`ConstSpec`]s.
pub fn generate(tts: &[ConstSpec]) -> Result<String> {
    let mut variants = String::new();
    let mut from_arms = String::new();
    let mut code_arms = String::new();
    let mut known_codes = Vec::with_capacity(tts.len());

    for d in tts {
        let value = expect_decimal(&d.name, &d.c_expr)?;
        let variant = variant_name(&d.name)?;
        writeln!(variants, "    /// `{}` ({value}).", d.name).context("writing variant doc")?;
        writeln!(variants, "    {variant},").context("writing variant")?;
        writeln!(
            from_arms,
            "            rshooks_core::{} => TxType::{variant},",
            d.name
        )
        .context("writing From arm")?;
        writeln!(
            code_arms,
            "            TxType::{variant} => rshooks_core::{},",
            d.name
        )
        .context("writing code() arm")?;
        known_codes.push(value);
    }
    let known_codes = known_codes.join(", ");

    let mut body = String::from("\n");
    body.push_str(
        "/// The transaction type of the originating transaction, decoded from the\n\
         /// raw `u16` `tt*` code returned by [`crate::api::otxn::otxn_type`].\n\
         ///\n\
         /// # Examples\n\
         ///\n\
         /// ```\n\
         /// use rshooks::tx_type::TxType;\n\
         ///\n\
         /// let ty = TxType::from(5);\n\
         /// assert_eq!(ty, TxType::SetRegularKey);\n\
         /// assert_eq!(ty.code(), 5);\n\
         /// ```\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
         pub enum TxType {\n",
    );
    body.push_str(&variants);
    body.push('\n');
    body.push_str(
        "    /// A code this version of rshooks does not recognize yet. Carries\n\
         /// the raw code for forward-compatibility.\n\
         Unknown(u16),\n\
         }\n\
         \n\
         impl From<u16> for TxType {\n\
         fn from(code: u16) -> Self {\n\
         match code {\n",
    );
    body.push_str(&from_arms);
    body.push_str(
        "            other => TxType::Unknown(other),\n\
         }\n\
         }\n\
         }\n\
         \n\
         impl TxType {\n\
         /// The raw `u16` code this variant corresponds to. Exact inverse of\n\
         /// [`TxType::from`]: `TxType::from(c).code() == c` for every code,\n\
         /// known or unknown.\n\
         #[inline(always)]\n\
         #[must_use]\n\
         pub const fn code(&self) -> u16 {\n\
         match *self {\n",
    );
    body.push_str(&code_arms);
    body.push_str(&format!(
        "            TxType::Unknown(code) => code,\n\
         }}\n\
         }}\n\
         }}\n\
         \n\
         #[cfg(test)]\n\
         mod tests {{\n\
         use super::*;\n\
         \n\
         #[test]\n\
         fn round_trips_known_codes() {{\n\
         const _: u16 = TxType::Unknown(0).code();\n\
         let known: &[u16] = &[{known_codes}];\n\
         for &code in known {{\n\
         assert_eq!(\n\
         TxType::from(code).code(),\n\
         code,\n\
         \"round-trip failed for {{code}}\"\n\
         );\n\
         }}\n\
         }}\n\
         \n\
         #[test]\n\
         fn unknown_code_round_trips() {{\n\
         let ty = TxType::from(9999);\n\
         assert_eq!(ty, TxType::Unknown(9999));\n\
         assert_eq!(ty.code(), 9999);\n\
         }}\n\
         }}\n"
    ));

    Ok(with_generated_marker("tts.h", MODULE_DOC) + &body)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::variant_name;

    #[test]
    fn uses_xahaud_canonical_transaction_type_names() -> Result<()> {
        let cases = [
            ("ttHOOK_SET", "SetHook"),
            ("ttNFTOKEN_BURN", "NFTokenBurn"),
            ("ttAMM_CREATE", "AMMCreate"),
            ("ttURITOKEN_MINT", "URITokenMint"),
            ("ttXCHAIN_CREATE_CLAIM_ID", "XChainCreateClaimID"),
            ("ttDID_SET", "DIDSet"),
            ("ttMPTOKEN_AUTHORIZE", "MPTokenAuthorize"),
            ("ttUNL_MODIFY", "UNLModify"),
            ("ttREGULAR_KEY_SET", "SetRegularKey"),
            ("ttPAYCHAN_CLAIM", "PaymentChannelClaim"),
            ("ttREMARKS_SET", "SetRemarks"),
            ("ttAMENDMENT", "EnableAmendment"),
            ("ttFEE", "SetFee"),
        ];

        for (constant, expected) in cases {
            assert_eq!(variant_name(constant)?, expected);
        }
        Ok(())
    }
}
