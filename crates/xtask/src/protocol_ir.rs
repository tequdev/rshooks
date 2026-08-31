//! `protocol_formats.json` intermediate representation.
//!
//! [`build`] parses the six vendored xahaud protocol format definitions (via
//! [`crate::protocol_parse`]) exactly once into a single serializable
//! [`ProtocolFormats`] tree, cross-validates it against the vendored
//! `hook/sfcodes.h`, and `gen_core` round-trips that tree through
//! `crates/rshooks-core/protocol_formats.json`, exactly as it does for
//! [`crate::ir::HookApiSpec`] and `hook_api.json`. The JSON is the real
//! intermediate artifact of the pipeline: every later consumer (a view
//! renderer, a transaction-builder renderer) reads it, never a re-parse of
//! the vendored files.
//!
//! # What the artifact carries, and what it deliberately does not
//!
//! It carries every format fact upstream declares: field lists in declared
//! order, each field's presence (`soeREQUIRED`/`soeOPTIONAL`/`soeDEFAULT`),
//! extras such as `soeMPTSupported` verbatim, the common-field split,
//! transaction and ledger-entry type values, and — via [`SFieldDef`] —
//! every field's numeric `(type << 16) | field` code.
//!
//! That last table is what makes **canonical wire order derivable**: the
//! declared macro order is *not* canonical (Payment declares `sfDestination`,
//! type 8, before `sfAmount`, type 6). A consumer that needs canonical order
//! sorts by [`SFieldDef::code`].
//!
//! Deliberately absent, because neither is format data: concrete
//! `soeDEFAULT` default *values* (upstream encodes only "may be omitted"),
//! and emit-plumbing ownership (`sfEmitDetails` and its plumbing fields are
//! written by `rshooks`'s `prepare_for_emit`/`StoWriter`, which remain their
//! owner; upstream marks only that the field is optional).
//!
//! # Versioning and the extension contract
//!
//! [`ProtocolFormats::version`] is [`PROTOCOL_FORMATS_VERSION`]. The
//! contract is **additive**: new fields may be added to any struct here
//! without a version bump (a consumer that doesn't know them ignores them),
//! and must deserialize cleanly for a consumer written against an earlier
//! shape. The version bumps only when an existing field changes meaning,
//! changes type, or disappears. The artifact is checked in and
//! `cargo xtask gen-core --check` fails on drift, so either kind of change
//! shows up as a reviewable diff.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::ir::ConstSpec;
use crate::protocol_parse::{
    self, FieldEntry, InnerObjectDecl, LedgerEntryDecl, PSEUDO_STI_MIN, Presence as ParsedPresence,
    SFieldDecl, TxDecl,
};
use crate::render::render_shift_add;

/// The schema version written to [`ProtocolFormats::version`]; see this
/// module's "Versioning and the extension contract".
pub const PROTOCOL_FORMATS_VERSION: u32 = 1;

/// One serialized field, from `sfields.macro`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SFieldDef {
    /// The field's name (`sfAccount`), verbatim.
    pub name: String,
    /// The serialized type token (`UINT32`, `ACCOUNT`, `VL`, …), verbatim.
    pub sti: String,
    /// That serialized type's numeric ID (`STI_UINT32` = 2, …).
    pub sti_code: u32,
    /// The field code within that serialized type.
    pub field_code: u16,
    /// The full field code, `(sti_code << 16) | field_code` — identical to
    /// `sfcodes.h`'s `sfXxx` constant, which [`build`] verifies. Sorting by
    /// this value yields canonical wire order.
    pub code: u32,
    /// `true` for `TYPED_SFIELD`, `false` for `UNTYPED_SFIELD`.
    pub typed: bool,
    /// Further macro arguments (`SField::sMD_Never`, …), verbatim and
    /// uninterpreted.
    pub extras: Vec<String>,
}

/// How a format declares a field may appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// `soeREQUIRED`.
    Required,
    /// `soeOPTIONAL`.
    Optional,
    /// `soeDEFAULT`: may be omitted from the wire form. This is *not* a
    /// default value — upstream encodes no such value, and neither does this
    /// artifact.
    Default,
}

impl From<ParsedPresence> for Presence {
    fn from(p: ParsedPresence) -> Self {
        match p {
            ParsedPresence::Required => Self::Required,
            ParsedPresence::Optional => Self::Optional,
            ParsedPresence::Default => Self::Default,
        }
    }
}

/// One entry of a format's field list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSpec {
    /// The referenced field's name (`sfAmount`). Always resolvable in
    /// [`ProtocolFormats::sfields`] — [`build`] fails otherwise.
    pub sfield: String,
    /// Its declared presence.
    pub presence: Presence,
    /// Further tokens in the entry (`soeMPTSupported`), verbatim and
    /// uninterpreted: a renderer that does not model them ignores them, and
    /// one that does gets them unmangled.
    pub extras: Vec<String>,
}

impl From<&FieldEntry> for FieldSpec {
    fn from(e: &FieldEntry) -> Self {
        Self {
            sfield: e.sfield.clone(),
            presence: e.presence.into(),
            extras: e.extras.clone(),
        }
    }
}

/// One transaction format, from `transactions.macro`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxFormat {
    /// The `tt*` tag (`ttPAYMENT`).
    pub tag: String,
    /// The transaction type value.
    pub value: u16,
    /// The upstream type name (`Payment`).
    pub name: String,
    /// The type-specific fields, in declared order. The fields every
    /// transaction also carries are [`ProtocolFormats::tx_common`], kept
    /// separate exactly as upstream keeps them.
    pub fields: Vec<FieldSpec>,
}

/// One ledger entry format, from `ledger_entries.macro`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntryFormat {
    /// The `lt*` tag (`ltRIPPLE_STATE`).
    pub tag: String,
    /// The ledger entry type value. Upstream writes these as decimal, hex
    /// and character literals; all three are normalized to the number here.
    pub value: u16,
    /// The upstream type name (`RippleState`).
    pub name: String,
    /// The RPC name (`state`).
    pub rpc_name: String,
    /// `true` when declared via `LEDGER_ENTRY_DUPLICATE` — upstream's marker
    /// for a ledger entry whose name is also a transaction type name
    /// (`DepositPreauth`).
    pub duplicate: bool,
    /// The type-specific fields, in declared order; see
    /// [`ProtocolFormats::le_common`] for the shared ones.
    pub fields: Vec<FieldSpec>,
}

/// One inner-object format, from `InnerObjectFormats.cpp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InnerObjectFormat {
    /// The field whose value this describes (`sfEmitDetails`).
    pub sfield: String,
    /// Its fields, in declared order. Inner objects have no common-field
    /// list.
    pub fields: Vec<FieldSpec>,
}

/// Every protocol format the vendored upstream sources declare. Serialized
/// verbatim as `crates/rshooks-core/protocol_formats.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolFormats {
    /// This artifact's schema version; see the module docs.
    pub version: u32,
    /// Every field `sfields.macro` declares, in file order.
    pub sfields: Vec<SFieldDef>,
    /// `TxFormats.cpp`'s `commonFields`: the fields every transaction format
    /// carries in addition to its own.
    pub tx_common: Vec<FieldSpec>,
    /// `LedgerFormats.cpp`'s `commonFields`: the fields every ledger entry
    /// format carries in addition to its own.
    pub le_common: Vec<FieldSpec>,
    /// Every transaction format, in file order.
    pub transactions: Vec<TxFormat>,
    /// Every ledger entry format, in file order.
    pub ledger_entries: Vec<LedgerEntryFormat>,
    /// Every inner-object format, in file order.
    pub inner_objects: Vec<InnerObjectFormat>,
}

/// The numeric value of one `sfcodes.h` constant, read through the same
/// [`render_shift_add`] the raw `sfcodes.rs` table is generated with — so the
/// cross-validation below genuinely compares against the constant `rshooks`
/// ships, not against a second interpretation of the header.
fn sfcode_value(spec: &ConstSpec) -> Result<u32> {
    let rendered = render_shift_add(&spec.c_expr)
        .with_context(|| format!("rendering the sfcodes.h value of `{}`", spec.name))?;
    let inner = rendered
        .strip_prefix('(')
        .ok_or_else(|| anyhow!("expected a leading `(` in {rendered:?}"))?;
    let (shift_group, tail) = inner
        .split_once(')')
        .ok_or_else(|| anyhow!("expected a closing `)` in {rendered:?}"))?;
    let (sti, shift) = shift_group
        .split_once("<<")
        .ok_or_else(|| anyhow!("expected `<<` in {rendered:?}"))?;
    if shift.trim() != "16" {
        bail!("expected a 16-bit shift in {rendered:?}");
    }
    let field = tail
        .trim()
        .strip_prefix('+')
        .ok_or_else(|| anyhow!("expected `+` in {rendered:?}"))?;
    let sti: u32 = sti.trim().parse().context("parsing a serialized type ID")?;
    let field: u32 = field.trim().parse().context("parsing a field code")?;
    sti.checked_shl(16)
        .map(|hi| hi | field)
        .ok_or_else(|| anyhow!("serialized type ID {sti} in {rendered:?} does not fit"))
}

/// Cross-validates `sfields.macro` against the vendored `hook/sfcodes.h`,
/// returning each field's full code.
///
/// Every field must exist in `sfcodes.h` with the identical
/// `(type << 16) | field` code. A mismatch or a missing name means the two
/// vendor groups have been synced out of step, and is a hard error naming
/// the field rather than a silently wrong generated view.
///
/// The four fields whose serialized type names a whole container
/// (`sfLedgerEntry`, `sfTransaction`, `sfValidation`, `sfMetadata`, IDs
/// 10001..10004) are exempt: `sfcodes.h` is generated from the Hook API's
/// point of view and omits them, since no hook ever reads a field of one of
/// those types. They are still carried in the artifact.
fn cross_validate(decls: &[SFieldDecl], sfcodes: &[ConstSpec]) -> Result<Vec<u32>> {
    let mut header: BTreeMap<&str, u32> = BTreeMap::new();
    for spec in sfcodes {
        if header
            .insert(spec.name.as_str(), sfcode_value(spec)?)
            .is_some()
        {
            bail!("sfcodes.h defines `{}` more than once", spec.name);
        }
    }

    let mut codes = Vec::with_capacity(decls.len());
    for decl in decls {
        let sti = protocol_parse::sti_code(&decl.sti)?;
        let code = sti
            .checked_shl(16)
            .map(|hi| hi | u32::from(decl.field_code))
            .ok_or_else(|| anyhow!("`{}`: serialized type ID {sti} does not fit", decl.name))?;
        codes.push(code);

        if sti >= PSEUDO_STI_MIN {
            continue;
        }
        let Some(&want) = header.get(decl.name.as_str()) else {
            bail!(
                "`{}` is declared in sfields.macro ({} {}) but missing from \
                 vendor/xahaud-hook/sfcodes.h — the two vendored upstream sources are out \
                 of sync; re-run scripts/sync-vendor.sh",
                decl.name,
                decl.sti,
                decl.field_code
            );
        };
        if want != code {
            bail!(
                "`{}`: sfields.macro says {} {} (code {code}) but sfcodes.h says {want} — the \
                 two vendored upstream sources are out of sync; re-run scripts/sync-vendor.sh",
                decl.name,
                decl.sti,
                decl.field_code
            );
        }
    }
    Ok(codes)
}

/// Converts a parsed field list, checking that every field it names resolves
/// in `known` — a format referencing a field `sfields.macro` does not
/// declare would otherwise generate an accessor for a field with no wire
/// code.
fn field_specs(
    entries: &[FieldEntry],
    known: &BTreeMap<&str, &SFieldDecl>,
    context: &str,
) -> Result<Vec<FieldSpec>> {
    for entry in entries {
        if !known.contains_key(entry.sfield.as_str()) {
            bail!(
                "{context} references `{}`, which sfields.macro does not declare",
                entry.sfield
            );
        }
    }
    Ok(entries.iter().map(FieldSpec::from).collect())
}

/// Builds the complete [`ProtocolFormats`] from the six vendored protocol
/// sources plus the `sfcodes.h` constants [`crate::ir::build`] already
/// parsed, which the cross-validation gate checks against.
pub fn build(
    sfields_macro: &str,
    transactions_macro: &str,
    ledger_entries_macro: &str,
    tx_formats_cpp: &str,
    ledger_formats_cpp: &str,
    inner_object_formats_cpp: &str,
    sfcodes: &[ConstSpec],
) -> Result<ProtocolFormats> {
    let sfield_decls =
        protocol_parse::parse_sfields(sfields_macro).context("parsing sfields.macro")?;
    let tx_decls = protocol_parse::parse_transactions(transactions_macro)
        .context("parsing transactions.macro")?;
    let le_decls = protocol_parse::parse_ledger_entries(ledger_entries_macro)
        .context("parsing ledger_entries.macro")?;
    let tx_common_entries = protocol_parse::parse_common_fields(tx_formats_cpp, "TxFormats.cpp")?;
    let le_common_entries =
        protocol_parse::parse_common_fields(ledger_formats_cpp, "LedgerFormats.cpp")?;
    let inner_decls = protocol_parse::parse_inner_objects(inner_object_formats_cpp)
        .context("parsing InnerObjectFormats.cpp")?;

    let codes = cross_validate(&sfield_decls, sfcodes)?;
    let known = protocol_parse::index_sfields(&sfield_decls)?;

    let sfields = sfield_decls
        .iter()
        .zip(&codes)
        .map(|(decl, code)| {
            Ok(SFieldDef {
                name: decl.name.clone(),
                sti: decl.sti.clone(),
                sti_code: protocol_parse::sti_code(&decl.sti)?,
                field_code: decl.field_code,
                code: *code,
                typed: decl.typed,
                extras: decl.extras.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let tx_common = field_specs(&tx_common_entries, &known, "TxFormats.cpp's commonFields")?;
    let le_common = field_specs(
        &le_common_entries,
        &known,
        "LedgerFormats.cpp's commonFields",
    )?;

    let transactions = tx_decls
        .iter()
        .map(|d: &TxDecl| {
            Ok(TxFormat {
                tag: d.tag.clone(),
                value: d.value,
                name: d.name.clone(),
                fields: field_specs(&d.fields, &known, &format!("TRANSACTION({})", d.tag))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let ledger_entries = le_decls
        .iter()
        .map(|d: &LedgerEntryDecl| {
            Ok(LedgerEntryFormat {
                tag: d.tag.clone(),
                value: d.value,
                name: d.name.clone(),
                rpc_name: d.rpc_name.clone(),
                duplicate: d.duplicate,
                fields: field_specs(&d.fields, &known, &format!("LEDGER_ENTRY({})", d.tag))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let inner_objects = inner_decls
        .iter()
        .map(|d: &InnerObjectDecl| {
            if !known.contains_key(d.sfield.as_str()) {
                bail!(
                    "InnerObjectFormats.cpp declares a format for `{}`, which sfields.macro \
                     does not declare",
                    d.sfield
                );
            }
            Ok(InnerObjectFormat {
                sfield: d.sfield.clone(),
                fields: field_specs(&d.fields, &known, &format!("add({})", d.sfield))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ProtocolFormats {
        version: PROTOCOL_FORMATS_VERSION,
        sfields,
        tx_common,
        le_common,
        transactions,
        ledger_entries,
        inner_objects,
    })
}

#[cfg(test)]
mod tests {
    //! Test code is exempt from the workspace's panic-freedom lints
    //! (`docs/DESIGN.md` §8).
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use std::collections::BTreeSet;

    use super::*;

    const SFIELDS: &str = include_str!("../../rshooks-core/vendor/xahaud-protocol/sfields.macro");
    const TRANSACTIONS: &str =
        include_str!("../../rshooks-core/vendor/xahaud-protocol/transactions.macro");
    const LEDGER_ENTRIES: &str =
        include_str!("../../rshooks-core/vendor/xahaud-protocol/ledger_entries.macro");
    const TX_FORMATS: &str =
        include_str!("../../rshooks-core/vendor/xahaud-protocol/TxFormats.cpp");
    const LEDGER_FORMATS: &str =
        include_str!("../../rshooks-core/vendor/xahaud-protocol/LedgerFormats.cpp");
    const INNER_OBJECT_FORMATS: &str =
        include_str!("../../rshooks-core/vendor/xahaud-protocol/InnerObjectFormats.cpp");
    const SFCODES_H: &str = include_str!("../../rshooks-core/vendor/xahaud-hook/sfcodes.h");

    fn sfcodes() -> Vec<ConstSpec> {
        crate::parse::scan_defines(SFCODES_H)
            .iter()
            .map(|d| ConstSpec {
                name: d.name.clone(),
                c_expr: d.value.clone(),
            })
            .collect()
    }

    fn corpus() -> ProtocolFormats {
        build(
            SFIELDS,
            TRANSACTIONS,
            LEDGER_ENTRIES,
            TX_FORMATS,
            LEDGER_FORMATS,
            INNER_OBJECT_FORMATS,
            &sfcodes(),
        )
        .unwrap_or_else(|e| panic!("{e:#}"))
    }

    /// Lower bounds, not exact counts: pins that the parser hasn't silently
    /// dropped whole swathes of the corpus, without needing a test edit when
    /// an upstream sync adds a type. (Currently vendored `release` snapshot:
    /// 74 / 34 / 329 / 28.)
    #[test]
    fn the_real_corpus_parses_completely() {
        let f = corpus();
        assert_eq!(f.version, PROTOCOL_FORMATS_VERSION);
        assert!(
            f.transactions.len() >= 70,
            "only {} transaction formats",
            f.transactions.len()
        );
        assert!(
            f.ledger_entries.len() >= 30,
            "only {} ledger entry formats",
            f.ledger_entries.len()
        );
        assert!(f.sfields.len() >= 325, "only {} sfields", f.sfields.len());
        assert!(
            f.inner_objects.len() >= 20,
            "only {} inner objects",
            f.inner_objects.len()
        );
        assert!(!f.tx_common.is_empty() && !f.le_common.is_empty());
    }

    #[test]
    fn every_referenced_field_resolves() {
        let f = corpus();
        let known: BTreeSet<&str> = f.sfields.iter().map(|s| s.name.as_str()).collect();
        let referenced = f
            .tx_common
            .iter()
            .chain(&f.le_common)
            .chain(f.transactions.iter().flat_map(|t| &t.fields))
            .chain(f.ledger_entries.iter().flat_map(|l| &l.fields))
            .chain(f.inner_objects.iter().flat_map(|i| &i.fields));
        for spec in referenced {
            assert!(known.contains(spec.sfield.as_str()), "{}", spec.sfield);
        }
        for inner in &f.inner_objects {
            assert!(known.contains(inner.sfield.as_str()), "{}", inner.sfield);
        }
    }

    #[test]
    fn type_values_tags_and_names_are_unique() {
        let f = corpus();
        for (what, values) in [
            (
                "transaction value",
                f.transactions
                    .iter()
                    .map(|t| u32::from(t.value))
                    .collect::<Vec<_>>(),
            ),
            (
                "ledger entry value",
                f.ledger_entries
                    .iter()
                    .map(|l| u32::from(l.value))
                    .collect::<Vec<_>>(),
            ),
        ] {
            let unique: BTreeSet<u32> = values.iter().copied().collect();
            assert_eq!(unique.len(), values.len(), "duplicate {what}");
        }
        for (what, names) in [
            (
                "transaction tag",
                f.transactions
                    .iter()
                    .map(|t| t.tag.clone())
                    .collect::<Vec<_>>(),
            ),
            (
                "transaction name",
                f.transactions
                    .iter()
                    .map(|t| t.name.clone())
                    .collect::<Vec<_>>(),
            ),
            (
                "ledger entry tag",
                f.ledger_entries
                    .iter()
                    .map(|l| l.tag.clone())
                    .collect::<Vec<_>>(),
            ),
            (
                "ledger entry name",
                f.ledger_entries
                    .iter()
                    .map(|l| l.name.clone())
                    .collect::<Vec<_>>(),
            ),
            (
                "inner object",
                f.inner_objects
                    .iter()
                    .map(|i| i.sfield.clone())
                    .collect::<Vec<_>>(),
            ),
        ] {
            let unique: BTreeSet<String> = names.iter().cloned().collect();
            assert_eq!(unique.len(), names.len(), "duplicate {what}");
        }
    }

    #[test]
    fn the_corpus_carries_the_facts_a_view_or_builder_renderer_needs() {
        let f = corpus();

        // Presence, extras and declared order survive verbatim.
        let payment = f
            .transactions
            .iter()
            .find(|t| t.name == "Payment")
            .unwrap_or_else(|| panic!("no Payment format"));
        assert_eq!(payment.tag, "ttPAYMENT");
        assert_eq!(payment.value, 0);
        assert_eq!(payment.fields[0].sfield, "sfDestination");
        let amount = &payment.fields[1];
        assert_eq!(amount.sfield, "sfAmount");
        assert_eq!(amount.presence, Presence::Required);
        assert_eq!(amount.extras, vec!["soeMPTSupported".to_string()]);
        assert!(
            payment
                .fields
                .iter()
                .any(|f| f.sfield == "sfPaths" && f.presence == Presence::Default)
        );

        // Declared order is not canonical order, and the artifact carries
        // what a builder needs to derive the latter (§ module docs).
        let code = |name: &str| {
            f.sfields
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.code)
                .unwrap_or_else(|| panic!("no {name}"))
        };
        assert!(
            code("sfDestination") > code("sfAmount"),
            "Payment declares sfDestination before sfAmount, but sfAmount sorts first"
        );

        // The empty field list, and the character-literal type value.
        let did_delete = f
            .transactions
            .iter()
            .find(|t| t.name == "DIDDelete")
            .unwrap_or_else(|| panic!("no DIDDelete format"));
        assert!(did_delete.fields.is_empty());
        let hook = f
            .ledger_entries
            .iter()
            .find(|l| l.tag == "ltHOOK")
            .unwrap_or_else(|| panic!("no ltHOOK format"));
        assert_eq!(hook.value, 0x48, "'H'");
        assert_eq!(hook.rpc_name, "hook");

        // LEDGER_ENTRY_DUPLICATE is recorded as such.
        let preauth = f
            .ledger_entries
            .iter()
            .find(|l| l.tag == "ltDEPOSIT_PREAUTH")
            .unwrap_or_else(|| panic!("no ltDEPOSIT_PREAUTH format"));
        assert!(preauth.duplicate);

        // Common fields are kept separate from type-specific ones.
        assert!(f.tx_common.iter().any(|f| f.sfield == "sfTransactionType"));
        assert!(f.le_common.iter().any(|f| f.sfield == "sfLedgerEntryType"));
        assert!(!payment.fields.iter().any(|f| f.sfield == "sfAccount"));

        // Inner objects, including one whose call uses the compact form.
        let emit = f
            .inner_objects
            .iter()
            .find(|i| i.sfield == "sfEmitDetails")
            .unwrap_or_else(|| panic!("no sfEmitDetails inner object"));
        assert_eq!(emit.fields[0].sfield, "sfEmitGeneration");

        // The four container-typed pseudo-fields survive the sfcodes.h gate.
        assert!(
            f.sfields
                .iter()
                .any(|s| s.name == "sfLedgerEntry" && s.sti_code >= PSEUDO_STI_MIN)
        );
    }

    #[test]
    fn serialization_is_byte_stable_across_a_round_trip() {
        let f = corpus();
        let once = serde_json::to_string_pretty(&f).unwrap_or_else(|e| panic!("{e}"));
        let twice = serde_json::to_string_pretty(&f).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(once, twice, "serialization is not deterministic");

        let back: ProtocolFormats = serde_json::from_str(&once).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, f, "the artifact does not round-trip");
        assert_eq!(
            serde_json::to_string_pretty(&back).unwrap_or_else(|e| panic!("{e}")),
            once,
            "re-serializing a deserialized artifact is not byte-stable"
        );
    }

    /// Declared macro order is not canonical wire order, but sorting on
    /// [`SFieldDef::code`] derives it — and that sort is total, since no
    /// format names the same field twice, not even across the common-field
    /// split.
    #[test]
    fn canonical_sfcode_order_is_derivable_by_sorting() {
        let f = corpus();
        let code: BTreeMap<&str, u32> = f
            .sfields
            .iter()
            .map(|s| (s.name.as_str(), s.code))
            .collect();

        let formats = f
            .transactions
            .iter()
            .map(|t| (t.name.as_str(), &t.fields, &f.tx_common))
            .chain(
                f.ledger_entries
                    .iter()
                    .map(|l| (l.name.as_str(), &l.fields, &f.le_common)),
            );

        let mut checked = 0usize;
        let mut saw_non_canonical_declaration = false;
        for (label, specific, common) in formats {
            // The field set a renderer sees: type-specific fields plus the
            // common ones, deduplicated by name with the type-specific entry
            // winning.
            let mut names: Vec<&str> = specific.iter().map(|s| s.sfield.as_str()).collect();
            for c in common {
                if !names.contains(&c.sfield.as_str()) {
                    names.push(c.sfield.as_str());
                }
            }
            let declared: Vec<u32> = names
                .iter()
                .map(|n| {
                    *code
                        .get(n)
                        .unwrap_or_else(|| panic!("{label} references unknown field {n}"))
                })
                .collect();
            let mut sorted = declared.clone();
            sorted.sort_unstable();
            assert!(
                sorted.windows(2).all(|w| w[0] < w[1]),
                "{label}: sorting field codes does not yield a strict order — a field \
                 appears twice, so canonical order is not derivable"
            );
            if declared != sorted {
                saw_non_canonical_declaration = true;
            }
            checked += 1;
        }
        assert_eq!(checked, f.transactions.len() + f.ledger_entries.len());
        assert!(
            saw_non_canonical_declaration,
            "no format declares its fields out of canonical order — the test is no longer \
             exercising the distinction it exists to pin"
        );
    }

    /// A consumer compiled against today's shape must keep deserializing an
    /// artifact that has *grown* fields, ignoring rather than absorbing them.
    #[test]
    fn the_version_and_additive_extension_contract_hold() {
        let f = corpus();
        assert_eq!(f.version, PROTOCOL_FORMATS_VERSION);

        let text = serde_json::to_string(&f).unwrap_or_else(|e| panic!("{e}"));
        let mut json: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}"));
        json["a_future_top_level_field"] = serde_json::json!(true);
        json["sfields"][0]["a_future_sfield_field"] = serde_json::json!("x");
        json["tx_common"][0]["a_future_field_spec_field"] = serde_json::json!(1);

        let extended: ProtocolFormats = serde_json::from_value(json)
            .unwrap_or_else(|e| panic!("an additive extension broke deserialization: {e}"));
        assert_eq!(
            extended, f,
            "unknown fields must be ignored, not absorbed into known ones"
        );
    }

    /// Generating from the in-memory IR and from the same IR round-tripped
    /// through JSON produces byte-identical output.
    #[test]
    fn generated_sources_are_deterministic() {
        let f = corpus();
        let text = serde_json::to_string(&f).unwrap_or_else(|e| panic!("{e}"));
        let round_tripped: ProtocolFormats =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}"));
        // The view renderers also read the curated availability
        // classification; the JSON hop must not perturb them either.
        let av: crate::availability::FormatAvailability =
            serde_json::from_str(include_str!("../../rshooks-core/format_availability.json"))
                .unwrap_or_else(|e| panic!("{e}"));

        for (label, direct, hopped) in [
            (
                "lets.rs",
                crate::codegen::lets::generate(&f.ledger_entries),
                crate::codegen::lets::generate(&round_tripped.ledger_entries),
            ),
            (
                "ledger_entry_type.rs",
                crate::codegen::ledger_entry_type::generate(&f.ledger_entries),
                crate::codegen::ledger_entry_type::generate(&round_tripped.ledger_entries),
            ),
            (
                "views/tx.rs",
                crate::codegen::views::generate_tx(&f, &av),
                crate::codegen::views::generate_tx(&round_tripped, &av),
            ),
            (
                "views/ledger.rs",
                crate::codegen::views::generate_ledger(&f, &av),
                crate::codegen::views::generate_ledger(&round_tripped, &av),
            ),
            (
                "views/inner.rs",
                crate::codegen::views::generate_inner(&f, &av),
                crate::codegen::views::generate_inner(&round_tripped, &av),
            ),
        ] {
            let direct = direct.unwrap_or_else(|e| panic!("{label}: {e:#}"));
            let hopped = hopped.unwrap_or_else(|e| panic!("{label}: {e:#}"));
            assert_eq!(direct, hopped, "{label} is not deterministic");
            assert!(!direct.is_empty(), "{label} rendered empty");
        }
    }

    #[test]
    fn checked_in_artifact_matches_the_vendored_sources() {
        let on_disk = include_str!("../../rshooks-core/protocol_formats.json");
        let mut expected =
            serde_json::to_string_pretty(&corpus()).unwrap_or_else(|e| panic!("{e}"));
        expected.push('\n');
        assert_eq!(
            on_disk, expected,
            "crates/rshooks-core/protocol_formats.json is stale; run `cargo xtask gen-core`"
        );
    }

    // --- cross-validation gate -------------------------------------------

    fn build_with(sfields: &str, sfcodes: &[ConstSpec]) -> Result<ProtocolFormats> {
        build(
            sfields,
            "TRANSACTION(ttPAYMENT, 0, Payment, ({{sfAmount, soeREQUIRED}}))\n",
            "",
            "static const std::initializer_list<SOElement> commonFields{{sfFlags, soeOPTIONAL}};\n",
            "static const std::initializer_list<SOElement> commonFields{{sfFlags, soeREQUIRED}};\n",
            "add(sfSigner.jsonName, sfSigner.getCode(), {{sfAmount, soeREQUIRED}});\n",
            sfcodes,
        )
    }

    fn spec(name: &str, expr: &str) -> ConstSpec {
        ConstSpec {
            name: name.into(),
            c_expr: expr.into(),
        }
    }

    fn minimal_sfcodes() -> Vec<ConstSpec> {
        vec![
            spec("sfAmount", "((6U << 16U) + 1U)"),
            spec("sfFlags", "((2U << 16U) + 2U)"),
            spec("sfSigner", "((14U << 16U) + 16U)"),
        ]
    }

    const MINIMAL_SFIELDS: &str = "\
TYPED_SFIELD(sfAmount, AMOUNT, 1)
TYPED_SFIELD(sfFlags,  UINT32, 2)
UNTYPED_SFIELD(sfSigner, OBJECT, 16)
";

    #[test]
    fn a_consistent_minimal_corpus_builds() {
        let f = build_with(MINIMAL_SFIELDS, &minimal_sfcodes()).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(f.transactions.len(), 1);
        assert_eq!(f.ledger_entries.len(), 0);
        assert_eq!(f.sfields[0].code, (6 << 16) | 1);
    }

    #[test]
    fn a_field_code_mismatch_against_sfcodes_h_is_an_error() {
        let mut codes = minimal_sfcodes();
        codes[0] = spec("sfAmount", "((6U << 16U) + 9U)");
        let msg = match build_with(MINIMAL_SFIELDS, &codes) {
            Ok(_) => panic!("expected a cross-validation failure"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            msg.contains("sfAmount") && msg.contains("out of sync"),
            "{msg}"
        );
    }

    #[test]
    fn a_field_missing_from_sfcodes_h_is_an_error() {
        let codes: Vec<ConstSpec> = minimal_sfcodes()
            .into_iter()
            .filter(|c| c.name != "sfFlags")
            .collect();
        let msg = match build_with(MINIMAL_SFIELDS, &codes) {
            Ok(_) => panic!("expected a cross-validation failure"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            msg.contains("sfFlags") && msg.contains("missing from"),
            "{msg}"
        );
    }

    #[test]
    fn a_format_referencing_an_undeclared_field_is_an_error() {
        let msg = match build(
            MINIMAL_SFIELDS,
            "TRANSACTION(ttPAYMENT, 0, Payment, ({{sfNotDeclared, soeREQUIRED}}))\n",
            "",
            "static const std::initializer_list<SOElement> commonFields{{sfFlags, soeOPTIONAL}};\n",
            "static const std::initializer_list<SOElement> commonFields{{sfFlags, soeREQUIRED}};\n",
            "add(sfSigner.jsonName, sfSigner.getCode(), {{sfAmount, soeREQUIRED}});\n",
            &minimal_sfcodes(),
        ) {
            Ok(_) => panic!("expected a resolution failure"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            msg.contains("sfNotDeclared") && msg.contains("does not declare"),
            "{msg}"
        );
    }
}
