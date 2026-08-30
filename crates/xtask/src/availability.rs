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
//! - [`Tier::Active`] — activated on Xahau mainnet. Always available.
//! - [`Tier::Pending`] — Xahau-bound and supported by the node, but not
//!   activated as of the vendored snapshot. **Available by default**;
//!   excluded by the [`ACTIVE_ONLY_FEATURE`] cargo feature for a hook that
//!   wants its surface restricted to what is live today.
//! - [`Tier::Dormant`] — inherited from rippled with no activation prospect
//!   on Xahau **mainnet** (in practice: gated by an amendment
//!   `features.macro` marks `Supported::no`, or depending on one). Available
//!   only under [`ALL_FEATURE`], for a custom network whose operator knows
//!   otherwise.
//!
//! Nothing is *omitted* any more: every tier is rendered, and the `#[cfg]`
//! it carries decides whether it compiles. [`Tier::cfg_attr`] has the
//! truth table and the widest-wins precedence rule for both features on.
//!
//! The `Supported::no` half is objective and checkable against the vendored
//! `features.macro`. The active/pending split is a fact about ledger state,
//! which no file in this repository can answer — hence a curated list rather
//! than a derivation. It is not guesswork either: [`DOC`] (reproduced into
//! the artifact itself) records the mainnet snapshot the current tiers were
//! verified against and the `sha512half(feature_name) ∈ Amendments`
//! membership recipe to re-verify them, along with the retired-amendment
//! caveat that makes absence from that list *not* evidence of dormancy.
//!
//! # Formats are the unit, with a curated escape hatch for fields
//!
//! Tiers are per *format*, and a field's tier is derived from the formats
//! referencing it. That is right almost always and wrong in one class of
//! case: an amendment can gate a **field** of an otherwise available
//! format. `sfCredentialIDs` sits on `Payment` — active — but needs
//! `featureCredentials`, which xahaud marks `Supported::no`, so no
//! validated Xahau `Payment` can carry it. [`FormatAvailability::field_overrides`]
//! is the curated fix, applied after derivation;
//! [`FormatAvailability::field_tiers`] documents the full rule.
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

/// The cargo feature that *narrows* the surface to [`Tier::Active`] only.
///
/// Opt-in: without it, `pending` shapes are available too.
pub const ACTIVE_ONLY_FEATURE: &str = "active-amendments";

/// The cargo feature that *widens* the surface to everything, [`Tier::Dormant`]
/// included.
///
/// Dominates [`ACTIVE_ONLY_FEATURE`] when both are on — see [`Tier::cfg_attr`]
/// for why that direction is the safe one.
pub const ALL_FEATURE: &str = "all-amendments";

/// This artifact's schema version. Same additive-extension contract as
/// [`crate::protocol_ir::PROTOCOL_FORMATS_VERSION`].
pub const FORMAT_AVAILABILITY_VERSION: u32 = 1;

/// How available a format is to a hook author on Xahau.
///
/// Every tier is *rendered*; what differs is the `#[cfg]` it carries, and so
/// which cargo features have to be on for it to compile. See
/// [`Tier::cfg_attr`] for the three states and the precedence rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Activated on Xahau mainnet. Generated normally.
    ///
    /// First in declaration order because [`Tier::best`] takes the minimum,
    /// and "most available" is what a field's tier resolves to.
    Active,
    /// Supported by the node but not activated as of the vendored snapshot.
    /// Available by default; excluded under [`ACTIVE_ONLY_FEATURE`].
    Pending,
    /// No activation prospect on Xahau. Available only under
    /// [`ALL_FEATURE`].
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
    ///
    /// Three states, from two features:
    ///
    /// | features on | active | pending | dormant |
    /// |---|---|---|---|
    /// | *(none)* | yes | yes | no |
    /// | `active-amendments` | yes | no | no |
    /// | `all-amendments` | yes | yes | yes |
    /// | both | yes | yes | yes |
    ///
    /// The default is deliberately the middle one: a `pending` shape is
    /// something Xahau is expected to get, so writing against it early is a
    /// reasonable thing to do without ceremony, while `dormant` shapes
    /// cannot appear on Xahau mainnet and stay out of the way.
    ///
    /// **Both features on is widest-wins**, which is not a detail — cargo
    /// unifies features across the whole dependency graph, so a crate that
    /// enables `all-amendments` turns it on for every other user of
    /// `rshooks` in that build. Making the wider feature dominate keeps
    /// enabling a feature *additive in effect*: pulling in a dependency can
    /// give you more API than you asked for, but it can never take API away
    /// from you, which would be a build break you did not cause and cannot
    /// see. That is why `pending` reads
    /// `any(not(active-amendments), all-amendments)` rather than the more
    /// obvious `not(active-amendments)`.
    #[must_use]
    pub fn cfg_attr(self) -> Option<String> {
        match self {
            Self::Active => None,
            Self::Pending => Some(format!(
                "#[cfg(any(not(feature = \"{ACTIVE_ONLY_FEATURE}\"), feature = \"{ALL_FEATURE}\"))]"
            )),
            Self::Dormant => Some(format!("#[cfg(feature = \"{ALL_FEATURE}\")]")),
        }
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
    /// Per-field tiers that **override** what the format-derivation rule
    /// produces, applied afterwards and keyed by full `sfXxx` name.
    ///
    /// The escape hatch for the derivation rule's blind spot: it reasons
    /// about formats, and an amendment can gate a *field* of an otherwise
    /// available format. `sfCredentialIDs` is the worked example — `Payment`
    /// is active, but the field is gated by `featureCredentials`, which
    /// xahaud marks `Supported::no`, so a validated Xahau `Payment` can
    /// never carry it (its transactor returns `temDISABLED` when it is
    /// present). Derivation says active; the truth is dormant.
    ///
    /// Deliberately **not** auto-populated: nothing here derives what an
    /// amendment gates at field level, so every entry is a human's
    /// evidence-backed claim. [`FormatAvailability::auto_add`] never touches
    /// this map, and [`FormatAvailability::validate`] rejects an entry
    /// naming a field the artifact does not declare.
    #[serde(default)]
    pub field_overrides: BTreeMap<String, Tier>,
}

/// The `doc` block written into the artifact.
const DOC: &[&str] = &[
    "Curated: which declared protocol formats a hook can actually use on Xahau.",
    "NOT vendor data and NOT derived — a human maintains the tiers.",
    "",
    "  active   activated on Xahau mainnet.",
    "  pending  supported by the node, not yet activated.",
    "  dormant  gated by an amendment features.macro marks Supported::no",
    "           (or depending on one), so it cannot activate on Xahau mainnet",
    "           -- though a custom network may still run it.",
    "",
    "Every tier is generated. Two cargo features on the rshooks crate decide",
    "which ones compile:",
    "",
    "  (neither)           active + pending   <- the default",
    "  active-amendments   active only",
    "  all-amendments      active + pending + dormant",
    "  both                same as all-amendments (widest wins)",
    "",
    "Widest-wins matters because cargo unifies features across the whole",
    "dependency graph: a crate enabling all-amendments turns it on for every",
    "other user of rshooks in that build. Making the wider feature dominate",
    "keeps enabling a feature additive in effect -- a dependency can give you",
    "more API than you asked for, never take API away.",
    "",
    "`cargo xtask gen-core` appends unknown formats as `dormant` and does",
    "nothing else to this file; moving an entry between tiers is a human",
    "decision. `gen-core --check` fails on an unclassified format and on a",
    "classification naming a format the artifact does not declare.",
    "",
    "Field constants in `rshooks::sfield` follow their formats: a field takes",
    "the best tier among the formats referencing it, and a field no format",
    "references stays active (those are usually structural/metadata fields,",
    "not amendment-borne). The raw layers -- rshooks-core's sfcodes/tts/lets",
    "-- and the TxType/LedgerEntryType decoders stay complete regardless:",
    "they mirror the wire protocol, and a decoder that cannot name a code it",
    "might receive is worse than one that can.",
    "",
    "`field_overrides` corrects that derivation where it is wrong, and is",
    "applied after it. An amendment can gate a FIELD rather than a format:",
    "sfCredentialIDs is on Payment (active) but needs featureCredentials,",
    "which is Supported::no, so no validated Xahau Payment can carry it --",
    "derivation says active, the truth is dormant. The reverse also happens:",
    "sfLockingChainIssue/sfIssuingChainIssue live inside the opaque",
    "sfXChainBridge wire type, so no format lists them and the",
    "structural-fallback keeps them active.",
    "",
    "Nothing derives field_overrides -- gen-core never adds, removes or",
    "retiers an entry there. Seed it only with fields whose gating amendment",
    "is Supported::no (objectively dead); leave judgment cases such as",
    "sfHookName/NamedHooks to deliberate manual curation.",
    "",
    "=== HOW THE active/pending SPLIT WAS VERIFIED ===",
    "",
    "Snapshot: Xahau mainnet, validated ledger 25441901, 2026-08-30,",
    "via https://xahau.network. The Amendments ledger object at index",
    "7DB0788C020F02780A673DC74757F23823FA3014C1866E72CC4CD8B226CD6EF4",
    "listed 74 activated amendment hashes.",
    "",
    "An amendment is identified on-ledger by its hash, not its name:",
    "",
    "  amendment_id = sha512half(feature_name)   # first 32 bytes of SHA-512",
    "                                            # of the ASCII name, e.g. \"Cron\"",
    "",
    "So the membership test for one amendment is:",
    "",
    "  curl -s https://xahau.network -H 'Content-Type: application/json' -d '{",
    "    \"method\":\"ledger_entry\",",
    "    \"params\":[{\"index\":\"7DB0788C020F02780A673DC74757F23823FA3014C1866E72CC4CD8B226CD6EF4\",",
    "                \"ledger_index\":\"validated\"}]}' \\",
    "  | python3 -c 'import sys,json,hashlib;",
    "      n=sys.argv[1].encode();",
    "      h=hashlib.sha512(n).hexdigest()[:64].upper();",
    "      a=json.load(sys.stdin)[\"result\"][\"node\"][\"Amendments\"];",
    "      print(sys.argv[1], h in a)' Cron",
    "",
    "CAVEAT — absence proves nothing on its own. A RETIRED amendment is",
    "unconditionally on and is absent from the Amendments object (Escrow,",
    "PaymentChannels, MultiSign and friends are retired in features.macro).",
    "Never demote a format to `dormant` because its amendment is missing",
    "from that list. The dormant criterion is and stays `Supported::no` in",
    "the vendored features.macro; the ledger check only distinguishes",
    "`active` from `pending` among amendments features.macro still tracks.",
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
            field_overrides: BTreeMap::new(),
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
    /// Appends and **nothing else**: no tier is changed, no entry removed,
    /// and [`Self::field_overrides`] is not touched at all. A run that finds
    /// no new format leaves the file byte-identical, which a test pins.
    /// Normalizing the `doc`/`version` header is a separate, explicit step
    /// ([`Self::refresh_doc`]) applied when the file is rendered.
    pub fn auto_add(&mut self, formats: &ProtocolFormats) -> Vec<String> {
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

    /// Rewrites the `doc`/`version` header from this module's [`DOC`], so a
    /// wording change here lands on the next `gen-core` run instead of
    /// needing a hand edit.
    ///
    /// Separate from [`Self::auto_add`] on purpose: that function's contract
    /// is "appends dormant entries and nothing else", and quietly
    /// normalizing a header alongside it would make the contract false. This
    /// is a rendering concern, applied where the file is serialized — so a
    /// stale header shows up as an ordinary `gen-core --check` staleness
    /// mismatch rather than being silently preserved.
    pub fn refresh_doc(&mut self) {
        self.doc = DOC.iter().map(|s| (*s).to_owned()).collect();
        self.version = FORMAT_AVAILABILITY_VERSION;
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
        if self.version != FORMAT_AVAILABILITY_VERSION {
            bail!(
                "crates/rshooks-core/format_availability.json is schema version {} but this \
                 xtask understands {FORMAT_AVAILABILITY_VERSION}; run `cargo xtask gen-core` if \
                 the file is merely stale, or reconcile the shapes if it is not",
                self.version
            );
        }

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

        // An override naming a field upstream no longer declares is dead
        // policy, the same way a stale format classification is — and more
        // easily missed, since these are hand-written one at a time.
        let declared_fields: BTreeSet<&str> =
            formats.sfields.iter().map(|s| s.name.as_str()).collect();
        let unknown: Vec<&str> = self
            .field_overrides
            .keys()
            .map(String::as_str)
            .filter(|n| !declared_fields.contains(n))
            .collect();
        if !unknown.is_empty() {
            bail!(
                "crates/rshooks-core/format_availability.json overrides {} field(s) \
                 sfields.macro does not declare: {}\nremove them, or re-vendor if upstream \
                 should still have them",
                unknown.len(),
                unknown.join(", ")
            );
        }
        Ok(())
    }

    /// Every serialized field's tier: the best tier among the formats
    /// referencing it.
    ///
    /// Fields no format references are **not** in the map, and callers treat
    /// them as [`Tier::Active`]. That is deliberate: an unreferenced field is
    /// usually structural rather than amendment-borne — metadata fields
    /// (`sfAffectedNodes` and friends), hash and index plumbing, the four
    /// container-typed pseudo-fields — and none of those arrive with an
    /// amendment.
    ///
    /// # Where format-derivation is wrong, and what fixes it
    ///
    /// Deriving a field's tier from its formats has one blind spot, in both
    /// directions, because **an amendment can gate a field rather than a
    /// format**:
    ///
    /// - A field of an *available* format can still be unreachable.
    ///   `sfCredentialIDs` is on `Payment`, `EscrowFinish`,
    ///   `PaymentChannelClaim` and `AccountDelete` — all active — but is
    ///   gated by `featureCredentials`, which xahaud marks `Supported::no`.
    ///   A validated Xahau `Payment` can never carry it; its transactor
    ///   returns `temDISABLED` when it is present. Derivation says active.
    /// - A genuinely amendment-borne field can look *structural* because it
    ///   is reachable only from inside an opaque wire type, so no format's
    ///   field list names it: `sfLockingChainIssue` and
    ///   `sfIssuingChainIssue` live inside `sfXChainBridge` (serialized type
    ///   25, not an object), and the fallback above keeps them active.
    ///
    /// [`FormatAvailability::field_overrides`] is the remedy for both, and
    /// is applied after derivation. It is curated rather than derived
    /// because nothing in this repository knows what an amendment gates at
    /// field level — so it is seeded only with cases whose amendment is
    /// `Supported::no` (objectively dead), and judgment-tier cases
    /// (`sfHookName` and `NamedHooks`, say) are left to manual curation
    /// rather than guessed at.
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

        // Overrides land last and win outright — including over the
        // "unreferenced fields stay active" fallback, which is how
        // `sfLockingChainIssue` (reachable only inside the opaque
        // `sfXChainBridge` wire type) gets classified at all.
        for (field, tier) in &self.field_overrides {
            out.insert(field.clone(), *tier);
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
    use crate::protocol_ir::{
        FieldSpec, InnerObjectFormat, LedgerEntryFormat, Presence, SFieldDef, TxFormat,
    };

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
            sfields: [
                "sfAccount",
                "sfFlags",
                "sfAmount",
                "sfShared",
                "sfBidMax",
                "sfBaseAsset",
                "sfPrice",
                "sfAuctionSlot",
                "sfLedgerIndex",
                "sfCredentialIDs",
            ]
            .iter()
            .map(|n| SFieldDef {
                name: (*n).to_owned(),
                sti: "UINT32".into(),
                sti_code: 2,
                field_code: 1,
                code: (2 << 16) | 1,
                typed: true,
                extras: Vec::new(),
            })
            .collect(),
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

    /// The invariant `auto_add`'s doc comment states: a run that finds no
    /// new format must leave the file byte-identical. Serialized rather than
    /// compared structurally, because byte-identity is the property that
    /// actually matters to `gen-core --check`.
    #[test]
    fn auto_add_on_a_complete_file_is_byte_identical() {
        let mut a = classified();
        a.field_overrides
            .insert("sfCredentialIDs".into(), Tier::Dormant);
        a.refresh_doc();
        let before = serde_json::to_string_pretty(&a).unwrap_or_default();

        let added = a.auto_add(&formats());
        let after = serde_json::to_string_pretty(&a).unwrap_or_default();

        assert!(added.is_empty(), "nothing new to add: {added:?}");
        assert_eq!(before, after, "auto_add mutated an already-complete file");
    }

    /// `auto_add` must not touch `field_overrides` even when it *is* adding
    /// formats — that map is curated, and nothing derives it.
    #[test]
    fn auto_add_never_touches_field_overrides() {
        let mut a = FormatAvailability::empty();
        a.field_overrides
            .insert("sfCredentialIDs".into(), Tier::Dormant);
        let before = a.field_overrides.clone();
        assert!(
            !a.auto_add(&formats()).is_empty(),
            "should have added formats"
        );
        assert_eq!(a.field_overrides, before);
    }

    #[test]
    fn a_field_override_wins_over_the_derived_tier() {
        let mut a = classified();
        // Derivation says active (only `Payment`, an active format, uses it).
        assert_eq!(
            a.field_tiers(&formats()).get("sfAmount"),
            Some(&Tier::Active)
        );
        a.field_overrides.insert("sfAmount".into(), Tier::Dormant);
        assert_eq!(
            a.field_tiers(&formats()).get("sfAmount"),
            Some(&Tier::Dormant),
            "an override must beat the derived tier"
        );
    }

    /// The other direction the override exists for: a field no format
    /// references at all, which the structural fallback would keep active.
    #[test]
    fn a_field_override_reaches_fields_no_format_references() {
        let mut a = classified();
        assert!(!a.field_tiers(&formats()).contains_key("sfLedgerIndex"));
        a.field_overrides
            .insert("sfLedgerIndex".into(), Tier::Dormant);
        assert_eq!(
            a.field_tiers(&formats()).get("sfLedgerIndex"),
            Some(&Tier::Dormant)
        );
    }

    #[test]
    fn an_override_for_an_undeclared_field_fails_the_check() {
        let mut a = classified();
        a.field_overrides
            .insert("sfNotAField".into(), Tier::Dormant);
        let msg = format!("{:#}", a.validate(&formats()).unwrap_err());
        assert!(msg.contains("sfNotAField"), "{msg}");
        assert!(msg.contains("does not declare"), "{msg}");
    }

    #[test]
    fn a_wrong_schema_version_fails_the_check() {
        let mut a = classified();
        a.version = FORMAT_AVAILABILITY_VERSION + 1;
        let msg = format!("{:#}", a.validate(&formats()).unwrap_err());
        assert!(msg.contains("schema version"), "{msg}");
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
        // Every tier renders now; the cfg is what differs.
        assert_eq!(Tier::Active.cfg_attr(), None);
        let pending = Tier::Pending.cfg_attr().unwrap_or_default();
        let dormant = Tier::Dormant.cfg_attr().unwrap_or_default();
        assert!(
            pending.contains(&format!("not(feature = \"{ACTIVE_ONLY_FEATURE}\")"))
                && pending.contains(&format!("feature = \"{ALL_FEATURE}\"")),
            "pending must be widest-wins, not a bare `not(active-amendments)`: {pending}"
        );
        assert_eq!(dormant, format!("#[cfg(feature = \"{ALL_FEATURE}\")]"));
    }
}
