//! `format_availability.json`: which declared formats a hook author can
//! actually use on Xahau.
//!
//! `protocol_formats.json` is upstream's word on what the *protocol* declares.
//! It is not the same question as what a hook can *use*: xahaud's format
//! tables are inherited wholesale from rippled and include amendments Xahau
//! marks `Supported::no` — the node would be amendment-blocked if one ever
//! activated — alongside Xahau-native features that are supported but not yet
//! voted in. Generating a typed view for an `AMMBid` or an `XChainCommit`
//! offers a hook author an API that can never match a real transaction.
//!
//! This file is the curated answer, and it is **not** vendor data: nothing
//! upstream states it, and no parser derives it. It lives beside
//! `protocol_formats.json` because `gen-core` consumes the two together, and
//! outside `vendor/` because a human maintains it.
//!
//! # The three tiers
//!
//! - [`Tier::Active`] — activated on Xahau mainnet. Generated normally.
//! - [`Tier::Pending`] — Xahau-bound and supported by the node, but not
//!   activated as of the vendored snapshot. Generated behind the
//!   [`PENDING_FEATURE`] cargo feature, so a hook opting in gets the shape
//!   and everyone else does not.
//! - [`Tier::Dormant`] — inherited from rippled with no activation prospect
//!   (in practice: gated by an amendment `features.macro` marks
//!   `Supported::no`, or depending on one). Not generated at all.
//!
//! The `Supported::no` half is objective and checkable against the vendored
//! `features.macro`. The active/pending split is a judgment about ledger
//! state, which no file in this repository can answer — hence a curated,
//! hand-reviewed list rather than a derivation.
//!
//! # The one automatic mutation
//!
//! `cargo xtask gen-core` appends any format present in the artifact but
//! missing here as [`Tier::Dormant`], and that is the only edit it makes:
//! a newly vendored format is unusable until a human says otherwise, which
//! is the safe direction. Moving an entry *between* tiers is always a human
//! decision. `gen-core --check` fails on an unclassified format (telling the
//! reader to run `gen-core`) and on a classification naming a format the
//! artifact does not declare, so the two files cannot drift apart silently.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::protocol_ir::ProtocolFormats;

/// The cargo feature [`Tier::Pending`] items are generated behind.
///
/// Named in exactly one place so renaming it is a one-line change here plus
/// the matching rename in `crates/rshooks/Cargo.toml` — the renderer, the
/// generated `#[cfg]` attributes and the generated rustdoc all read it from
/// this constant.
pub const PENDING_FEATURE: &str = "pending-amendments";

/// This artifact's schema version. Same additive-extension contract as
/// [`crate::protocol_ir::PROTOCOL_FORMATS_VERSION`].
pub const FORMAT_AVAILABILITY_VERSION: u32 = 1;

/// How available a format is to a hook author on Xahau.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Activated on Xahau mainnet. Generated normally.
    ///
    /// First in declaration order because [`Tier::best`] takes the minimum,
    /// and "most available" is what a field's tier resolves to.
    Active,
    /// Supported by the node but not activated as of the vendored snapshot.
    /// Generated behind [`PENDING_FEATURE`].
    Pending,
    /// No activation prospect. Not generated.
    Dormant,
}

impl Tier {
    /// The more available of two tiers.
    ///
    /// A field belongs to every format that lists it, so its availability is
    /// the *best* of theirs: one active format referencing a field makes that
    /// field reachable, whatever else also references it.
    #[must_use]
    pub fn best(self, other: Self) -> Self {
        self.min(other)
    }

    /// The `#[cfg(...)]` line a generated item at this tier needs, if any.
    #[must_use]
    pub fn cfg_attr(self) -> Option<String> {
        match self {
            Self::Active => None,
            Self::Pending => Some(format!("#[cfg(feature = \"{PENDING_FEATURE}\")]")),
            // A dormant item is never rendered at all, so it never reaches
            // an attribute.
            Self::Dormant => None,
        }
    }

    /// Whether an item at this tier is rendered.
    #[must_use]
    pub fn is_rendered(self) -> bool {
        !matches!(self, Self::Dormant)
    }
}

/// The curated availability of every declared format, by namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatAvailability {
    /// This artifact's schema version.
    pub version: u32,
    /// A short explanation carried in the file itself, so the list is
    /// readable without this module's source. Rewritten verbatim on every
    /// `gen-core` run — edit [`DOC`], not the JSON.
    pub doc: Vec<String>,
    /// Transaction formats, keyed by upstream type name (`Payment`).
    pub transactions: BTreeMap<String, Tier>,
    /// Ledger entry formats, keyed by upstream type name (`RippleState`).
    pub ledger_entries: BTreeMap<String, Tier>,
    /// Inner-object formats, keyed by the field name **without** its `sf`
    /// prefix (`EmitDetails`), matching the generated struct name.
    pub inner_objects: BTreeMap<String, Tier>,
}

/// The `doc` block written into the artifact.
const DOC: &[&str] = &[
    "Curated: which declared protocol formats a hook can actually use on Xahau.",
    "NOT vendor data and NOT derived — a human maintains the tiers.",
    "",
    "  active   activated on Xahau mainnet; generated normally.",
    "  pending  supported by the node, not yet activated; generated behind",
    "           the `pending-amendments` cargo feature on the rshooks crate.",
    "  dormant  no activation prospect (in practice: gated by an amendment",
    "           features.macro marks Supported::no, or depending on one);",
    "           no code generated at all.",
    "",
    "`cargo xtask gen-core` appends unknown formats as `dormant` and does",
    "nothing else to this file; moving an entry between tiers is a human",
    "decision. `gen-core --check` fails on an unclassified format and on a",
    "classification naming a format the artifact does not declare.",
    "",
    "Field constants in `rshooks::sfield` follow their formats: a field takes",
    "the best tier among the formats referencing it, and a field no format",
    "references stays active (those are structural/metadata fields, not",
    "amendment-borne). The raw layers -- rshooks-core's sfcodes/tts/lets --",
    "and the TxType/LedgerEntryType decoders stay complete regardless: they",
    "mirror the wire protocol, and a decoder that cannot name a code it might",
    "receive is worse than one that can.",
];

impl FormatAvailability {
    /// An empty classification, for the first `gen-core` run.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            version: FORMAT_AVAILABILITY_VERSION,
            doc: DOC.iter().map(|s| (*s).to_owned()).collect(),
            transactions: BTreeMap::new(),
            ledger_entries: BTreeMap::new(),
            inner_objects: BTreeMap::new(),
        }
    }

    /// The tier of one transaction format. Unclassified formats are
    /// [`Tier::Dormant`]; [`validate`](Self::validate) is what turns that
    /// into a build failure rather than a silent omission.
    #[must_use]
    pub fn tx(&self, name: &str) -> Tier {
        self.transactions
            .get(name)
            .copied()
            .unwrap_or(Tier::Dormant)
    }

    /// The tier of one ledger entry format.
    #[must_use]
    pub fn ledger_entry(&self, name: &str) -> Tier {
        self.ledger_entries
            .get(name)
            .copied()
            .unwrap_or(Tier::Dormant)
    }

    /// The tier of one inner-object format, keyed without the `sf` prefix.
    #[must_use]
    pub fn inner_object(&self, name: &str) -> Tier {
        self.inner_objects
            .get(name)
            .copied()
            .unwrap_or(Tier::Dormant)
    }

    /// Appends every format the artifact declares and this file does not, as
    /// [`Tier::Dormant`]. Returns the names added, for the `gen-core` log.
    ///
    /// The only automatic mutation this file ever gets.
    pub fn auto_add(&mut self, formats: &ProtocolFormats) -> Vec<String> {
        // The doc block is owned by `DOC`, so a wording change lands on the
        // next run rather than needing a hand edit.
        self.doc = DOC.iter().map(|s| (*s).to_owned()).collect();
        self.version = FORMAT_AVAILABILITY_VERSION;

        let mut added = Vec::new();
        for t in &formats.transactions {
            if !self.transactions.contains_key(&t.name) {
                self.transactions.insert(t.name.clone(), Tier::Dormant);
                added.push(format!("transactions/{}", t.name));
            }
        }
        for l in &formats.ledger_entries {
            if !self.ledger_entries.contains_key(&l.name) {
                self.ledger_entries.insert(l.name.clone(), Tier::Dormant);
                added.push(format!("ledger_entries/{}", l.name));
            }
        }
        for i in &formats.inner_objects {
            let name = inner_key(&i.sfield);
            if !self.inner_objects.contains_key(&name) {
                self.inner_objects.insert(name.clone(), Tier::Dormant);
                added.push(format!("inner_objects/{name}"));
            }
        }
        added
    }

    /// Fails when the two files have drifted apart in either direction.
    ///
    /// An **unclassified** format would silently render as dormant — a view
    /// vanishing because nobody classified it is exactly the failure this
    /// check exists to prevent, so it names the offenders and points at
    /// `gen-core`. A **stale** classification (naming a format upstream no
    /// longer declares) is the other direction, and equally worth failing:
    /// it is dead policy that will outlive anyone's memory of why it is
    /// there.
    pub fn validate(&self, formats: &ProtocolFormats) -> Result<()> {
        let declared_tx: BTreeSet<&str> = formats
            .transactions
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        let declared_le: BTreeSet<&str> = formats
            .ledger_entries
            .iter()
            .map(|l| l.name.as_str())
            .collect();
        let declared_in: BTreeSet<String> = formats
            .inner_objects
            .iter()
            .map(|i| inner_key(&i.sfield))
            .collect();

        let mut unclassified = Vec::new();
        for n in &declared_tx {
            if !self.transactions.contains_key(*n) {
                unclassified.push(format!("transactions/{n}"));
            }
        }
        for n in &declared_le {
            if !self.ledger_entries.contains_key(*n) {
                unclassified.push(format!("ledger_entries/{n}"));
            }
        }
        for n in &declared_in {
            if !self.inner_objects.contains_key(n) {
                unclassified.push(format!("inner_objects/{n}"));
            }
        }
        if !unclassified.is_empty() {
            bail!(
                "crates/rshooks-core/format_availability.json does not classify {} format(s): \
                 {}\nrun `cargo xtask gen-core` to add them as `dormant`, then move any that \
                 should be usable to `pending` or `active`",
                unclassified.len(),
                unclassified.join(", ")
            );
        }

        let mut stale = Vec::new();
        for n in self.transactions.keys() {
            if !declared_tx.contains(n.as_str()) {
                stale.push(format!("transactions/{n}"));
            }
        }
        for n in self.ledger_entries.keys() {
            if !declared_le.contains(n.as_str()) {
                stale.push(format!("ledger_entries/{n}"));
            }
        }
        for n in self.inner_objects.keys() {
            if !declared_in.contains(n) {
                stale.push(format!("inner_objects/{n}"));
            }
        }
        if !stale.is_empty() {
            bail!(
                "crates/rshooks-core/format_availability.json classifies {} format(s) \
                 protocol_formats.json does not declare: {}\nremove them, or re-vendor if \
                 upstream should still have them",
                stale.len(),
                stale.join(", ")
            );
        }
        Ok(())
    }

    /// Every serialized field's tier: the best tier among the formats
    /// referencing it.
    ///
    /// Fields no format references are **not** in the map, and callers treat
    /// them as [`Tier::Active`]. That is deliberate: an unreferenced field is
    /// structural rather than amendment-borne — metadata fields
    /// (`sfAffectedNodes` and friends), hash and index plumbing, the four
    /// container-typed pseudo-fields — and none of those arrive with an
    /// amendment. See this module's docs; the imprecision it accepts is that
    /// a handful of genuinely amendment-borne fields are reachable only from
    /// inside an opaque wire type (`sfLockingChainIssue` inside
    /// `sfXChainBridge`) and so look structural here. They stay available as
    /// typed constants that no Xahau object will ever contain, which costs
    /// nothing but tidiness.
    #[must_use]
    pub fn field_tiers(&self, formats: &ProtocolFormats) -> BTreeMap<String, Tier> {
        let mut out: BTreeMap<String, Tier> = BTreeMap::new();
        let mut note = |field: &str, tier: Tier| {
            out.entry(field.to_owned())
                .and_modify(|t| *t = t.best(tier))
                .or_insert(tier);
        };

        // Common fields ride every format in their namespace, so they are as
        // available as the most available format there — active, since both
        // namespaces have active members.
        for f in formats.tx_common.iter().chain(&formats.le_common) {
            note(&f.sfield, Tier::Active);
        }
        for t in &formats.transactions {
            let tier = self.tx(&t.name);
            for f in &t.fields {
                note(&f.sfield, tier);
            }
        }
        for l in &formats.ledger_entries {
            let tier = self.ledger_entry(&l.name);
            for f in &l.fields {
                note(&f.sfield, tier);
            }
        }
        for i in &formats.inner_objects {
            let tier = self.inner_object(&inner_key(&i.sfield));
            // The field that *names* the inner object is as available as the
            // object itself — `sfEmitDetails` is only meaningful because
            // `EmitDetails` is.
            note(&i.sfield, tier);
            for f in &i.fields {
                note(&f.sfield, tier);
            }
        }
        out
    }
}

/// The inner-object key: the field name without its `sf` prefix, matching
/// the generated struct name. Names that somehow lack the prefix are used
/// verbatim; [`FormatAvailability::validate`] would surface the mismatch.
fn inner_key(sfield: &str) -> String {
    sfield.strip_prefix("sf").unwrap_or(sfield).to_owned()
}

#[cfg(test)]
mod tests {
    //! Test code is exempt from the workspace's panic-freedom lints
    //! (`docs/DESIGN.md` §8).
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use super::*;
    use crate::protocol_ir::{FieldSpec, InnerObjectFormat, LedgerEntryFormat, Presence, TxFormat};

    fn field(name: &str) -> FieldSpec {
        FieldSpec {
            sfield: name.into(),
            presence: Presence::Required,
            extras: Vec::new(),
        }
    }

    fn formats() -> ProtocolFormats {
        ProtocolFormats {
            version: 1,
            sfields: Vec::new(),
            tx_common: vec![field("sfAccount")],
            le_common: vec![field("sfFlags")],
            transactions: vec![
                TxFormat {
                    tag: "ttPAYMENT".into(),
                    value: 0,
                    name: "Payment".into(),
                    fields: vec![field("sfAmount"), field("sfShared")],
                },
                TxFormat {
                    tag: "ttAMM_BID".into(),
                    value: 39,
                    name: "AMMBid".into(),
                    fields: vec![field("sfBidMax"), field("sfShared")],
                },
                TxFormat {
                    tag: "ttORACLE_SET".into(),
                    value: 51,
                    name: "OracleSet".into(),
                    fields: vec![field("sfBaseAsset")],
                },
            ],
            ledger_entries: vec![LedgerEntryFormat {
                tag: "ltAMM".into(),
                value: 0x0079,
                name: "AMM".into(),
                rpc_name: "amm".into(),
                duplicate: false,
                fields: vec![field("sfBidMax")],
            }],
            inner_objects: vec![InnerObjectFormat {
                sfield: "sfAuctionSlot".into(),
                fields: vec![field("sfPrice")],
            }],
        }
    }

    fn classified() -> FormatAvailability {
        let mut a = FormatAvailability::empty();
        a.transactions.insert("Payment".into(), Tier::Active);
        a.transactions.insert("AMMBid".into(), Tier::Dormant);
        a.transactions.insert("OracleSet".into(), Tier::Pending);
        a.ledger_entries.insert("AMM".into(), Tier::Dormant);
        a.inner_objects.insert("AuctionSlot".into(), Tier::Dormant);
        a
    }

    #[test]
    fn a_field_takes_the_best_tier_among_the_formats_using_it() {
        let tiers = classified().field_tiers(&formats());
        // Only Payment (active) uses it.
        assert_eq!(tiers.get("sfAmount"), Some(&Tier::Active));
        // Only AMMBid/AMM (both dormant) use it.
        assert_eq!(tiers.get("sfBidMax"), Some(&Tier::Dormant));
        // Only OracleSet (pending).
        assert_eq!(tiers.get("sfBaseAsset"), Some(&Tier::Pending));
        // Shared between an active and a dormant format: the active one wins,
        // which is the whole point of `best`.
        assert_eq!(tiers.get("sfShared"), Some(&Tier::Active));
        // The naming field of a dormant inner object goes with it.
        assert_eq!(tiers.get("sfAuctionSlot"), Some(&Tier::Dormant));
        assert_eq!(tiers.get("sfPrice"), Some(&Tier::Dormant));
        // Common fields are active in both namespaces.
        assert_eq!(tiers.get("sfAccount"), Some(&Tier::Active));
        assert_eq!(tiers.get("sfFlags"), Some(&Tier::Active));
    }

    #[test]
    fn an_unreferenced_field_is_absent_from_the_map() {
        // Callers read that as `active` — see `field_tiers`' doc comment.
        let tiers = classified().field_tiers(&formats());
        assert!(!tiers.contains_key("sfLedgerIndex"));
    }

    #[test]
    fn auto_add_appends_unknown_formats_as_dormant_and_nothing_else() {
        let mut a = FormatAvailability::empty();
        a.transactions.insert("Payment".into(), Tier::Active);
        let added = a.auto_add(&formats());

        assert_eq!(
            a.tx("Payment"),
            Tier::Active,
            "an existing tier is untouched"
        );
        assert_eq!(a.tx("AMMBid"), Tier::Dormant);
        assert_eq!(a.ledger_entry("AMM"), Tier::Dormant);
        assert_eq!(a.inner_object("AuctionSlot"), Tier::Dormant);
        assert_eq!(
            added,
            vec![
                "transactions/AMMBid",
                "transactions/OracleSet",
                "ledger_entries/AMM",
                "inner_objects/AuctionSlot",
            ]
        );
        // Idempotent: a second run adds nothing.
        assert!(a.auto_add(&formats()).is_empty());
    }

    #[test]
    fn an_unclassified_format_fails_the_check() {
        let mut a = classified();
        a.transactions.remove("AMMBid");
        let msg = format!("{:#}", a.validate(&formats()).unwrap_err());
        assert!(msg.contains("transactions/AMMBid"), "{msg}");
        assert!(msg.contains("cargo xtask gen-core"), "{msg}");
    }

    #[test]
    fn a_classification_for_an_undeclared_format_fails_the_check() {
        let mut a = classified();
        a.ledger_entries.insert("Bridge".into(), Tier::Dormant);
        let msg = format!("{:#}", a.validate(&formats()).unwrap_err());
        assert!(msg.contains("ledger_entries/Bridge"), "{msg}");
        assert!(msg.contains("does not declare"), "{msg}");
    }

    #[test]
    fn a_fully_classified_corpus_validates() {
        classified()
            .validate(&formats())
            .unwrap_or_else(|e| panic!("{e:#}"));
    }

    #[test]
    fn tiers_serialize_as_the_snake_case_names_the_file_uses() {
        let json = serde_json::to_string(&Tier::Pending).unwrap_or_default();
        assert_eq!(json, "\"pending\"");
        assert_eq!(
            Tier::Dormant.cfg_attr(),
            None,
            "a dormant item is never rendered, so it never carries a cfg"
        );
        assert_eq!(
            Tier::Pending.cfg_attr().unwrap_or_default(),
            format!("#[cfg(feature = \"{PENDING_FEATURE}\")]")
        );
        assert!(Tier::Active.is_rendered() && Tier::Pending.is_rendered());
        assert!(!Tier::Dormant.is_rendered());
    }
}
