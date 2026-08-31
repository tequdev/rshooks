//! Generates the typed read views in `crates/rshooks/src/views/tx.rs`,
//! `ledger.rs` and `inner.rs` from the formats in `protocol_formats.json`.
//!
//! One struct per transaction type, ledger entry type and inner-object
//! format; one accessor per field the format declares. Upstream declares 74
//! transactions, 34 ledger entries and 28 inner objects on the currently
//! vendored snapshot, so this is the largest generator in the pipeline by an
//! order of magnitude — and the one whose output would be least maintainable
//! by hand, which is the point.
//!
//! # The rules the output obeys
//!
//! - **Code, not data.** No `static`, no export, no function pointer, no
//!   registration table — `rshooks-build`'s cleaner drops unreachable
//!   *functions* but keeps every active data segment regardless of
//!   reachability (`docs/DESIGN.md` §6.2), so a lookup table here would land
//!   in every hook's wasm whether or not it used a view. Everything is
//!   monomorphized `#[inline(always)]` accessors instead.
//! - **No logic.** Every decision a view makes — how absence is detected,
//!   when a slot is cleared, how a type check is spelled — lives in the
//!   hand-written `crate::views::source` module. The files rendered here
//!   are declarations that call into it, so the reviewable surface is one
//!   module rather than sixteen thousand lines.
//! - **Docs on every public item**, because `rshooks` denies `missing_docs`
//!   and this output is production `no_std` code under the same lint wall as
//!   everything else in the crate.
//! - **Deterministic**: a pure function of the artifact, in artifact order.
//! - **Availability-aware.** A format's tier in
//!   [`crate::availability`] decides whether it is rendered at all: a
//!   `dormant` format gets no struct, a `pending` one gets its whole item
//!   set behind `#[cfg(feature = "…")]`. The cfg goes on every item of the
//!   view — the struct, each `impl` block — so a pending view and the
//!   pending `sfield` constants it reads compile together or not at all.
//!
//! # Value types are keyed on the serialized type ID
//!
//! [`access`] maps a field's numeric `sti_code` to how the view reads it,
//! deliberately mirroring [`super::sfield::value_type`] — the two must agree,
//! because a view accessor passes the very `SField<T>` constant that
//! generator emitted. Where they differ is the *output*: `SField<Amount>`
//! reads back as `AmountBytes` and `SField<Issue>` as `IssueData`, since
//! `Amount`/`Issue` are wire markers rather than values.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, anyhow, bail};

use super::sfield::type_id_name;
use super::with_generated_marker_in;
use crate::availability::{FormatAvailability, Tier};
use crate::protocol_ir::{
    FieldSpec, InnerObjectFormat, LedgerEntryFormat, Presence, ProtocolFormats, SFieldDef, TxFormat,
};

// ---------------------------------------------------------------------
// Method naming
// ---------------------------------------------------------------------

/// Capitalized fragments that the generic word splitter cannot infer.
/// Keep longer prefixes before shorter ones because the first match wins.
const ACRONYMS: &[&str] = &[
    "NFToken", "MPToken", "XChain", "AMM", "DID", "UNL", "URI", "ID",
];

/// Splits a `PascalCase` field name into words, honoring [`ACRONYMS`].
fn split_words(name: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;

    while i < name.len() {
        let rest = name
            .get(i..)
            .ok_or_else(|| anyhow!("`{name}` is not ASCII, which field names are assumed to be"))?;

        if let Some(acronym) = ACRONYMS.iter().find(|a| rest.starts_with(**a)) {
            let end = consume_tail(name, i.saturating_add(acronym.len()));
            out.push(word(name, i, end)?);
            i = end;
            continue;
        }

        let first = rest
            .chars()
            .next()
            .ok_or_else(|| anyhow!("empty remainder while splitting `{name}`"))?;
        let mut end = i.saturating_add(1);
        if first.is_ascii_uppercase() {
            while char_at(name, end).is_some_and(|c| c.is_ascii_uppercase()) {
                end = end.saturating_add(1);
            }
            if end > i.saturating_add(1) {
                // A run of capitals: the last one belongs to the next word
                // when a lowercase letter follows it (`VLKey`, `MPToken`).
                if char_at(name, end).is_some_and(|c| c.is_ascii_lowercase()) {
                    end = end.saturating_sub(1);
                }
            } else {
                end = consume_tail(name, end);
            }
        } else {
            end = consume_tail(name, i);
        }
        out.push(word(name, i, end)?);
        i = end;
    }

    if out.is_empty() {
        bail!("`{name}` has no words to split");
    }
    Ok(out)
}

fn char_at(s: &str, at: usize) -> Option<char> {
    s.as_bytes().get(at).map(|b| *b as char)
}

fn consume_tail(s: &str, mut at: usize) -> usize {
    while char_at(s, at).is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        at = at.saturating_add(1);
    }
    at
}

fn word(name: &str, start: usize, end: usize) -> Result<String> {
    name.get(start..end)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("failed to slice `{name}` at {start}..{end}"))
}

/// Turns an `sfXxx` field name into its snake-case accessor name.
///
/// A name that collides with a Rust keyword becomes a raw identifier. The
/// four keywords that have no raw form (`self`, `Self`, `super`, `crate`)
/// are a hard error rather than a mangled name — none occurs today, and if
/// upstream ever adds one, a build failure naming the field is the right
/// outcome.
fn method_name(sfield: &str) -> Result<String> {
    let bare = sfield
        .strip_prefix("sf")
        .ok_or_else(|| anyhow!("expected an `sf`-prefixed field name, got `{sfield}`"))?;
    let joined = split_words(bare)?
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_");
    match joined.as_str() {
        "self" | "Self" | "super" | "crate" => bail!(
            "`{sfield}` maps to `{joined}`, which has no raw-identifier form; \
             it needs a special case in codegen::views"
        ),
        _ if RUST_KEYWORDS.contains(&joined.as_str()) => Ok(format!("r#{joined}")),
        _ => Ok(joined),
    }
}

/// Rust's strict and reserved keywords, minus the four with no `r#` form
/// (which [`method_name`] rejects outright).
const RUST_KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
    "virtual", "where", "while", "yield",
];

// ---------------------------------------------------------------------
// Value mapping
// ---------------------------------------------------------------------

/// How a view reads a field, decided by its serialized type ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Value(&'static str),
    Raw,
    /// A container: raw bytes on every source, plus a `*_slot` child-slot
    /// accessor on the slot-backed ones (only the slot API can navigate
    /// into a container — `otxn_field` has no way to).
    Container(&'static str),
}

/// Maps a serialized type ID to how a view reads it.
///
/// Kept in lockstep with [`super::sfield::value_type`], which decides the
/// `SField<T>` these accessors are handed; where the two differ it is
/// because `T` is a wire marker rather than a value (`Amount` reads back as
/// `AmountBytes`, `Issue` as `IssueData`).
///
/// Everything else — `Blob`, `PathSet`, `Vector256`, `Number`, `Hash128`,
/// `Hash160`, `UInt96`/`192`/`384`/`512`, `XChainBridge` — is [`Access::Raw`],
/// matching the `Opaque` those fields' constants carry. `Number` in
/// particular is *not* an XFL and is deliberately not typed as one.
fn access(sti_code: u32) -> Access {
    match sti_code {
        16 => Access::Value("u8"),
        1 => Access::Value("u16"),
        2 => Access::Value("u32"),
        3 => Access::Value("u64"),
        5 => Access::Value("crate::types::Hash"),
        8 => Access::Value("crate::types::AccountId"),
        26 => Access::Value("crate::types::CurrencyCode"),
        6 => Access::Value("crate::slot_obj::AmountBytes"),
        24 => Access::Value("crate::slot_obj::IssueData"),
        14 => Access::Container("crate::types::STObject"),
        15 => Access::Container("crate::types::STArray"),
        _ => Access::Raw,
    }
}

fn presence_token(p: Presence) -> &'static str {
    match p {
        Presence::Required => "soeREQUIRED",
        Presence::Optional => "soeOPTIONAL",
        Presence::Default => "soeDEFAULT",
    }
}

/// Whether a field reads back as `Option<_>`.
///
/// `soeDEFAULT` reads as `Option` alongside `soeOPTIONAL`: upstream's
/// `soeDEFAULT` says only that the field may be omitted from the wire form,
/// and encodes no default *value* to substitute (`crate::protocol_ir`'s
/// module docs).
fn is_optional(p: Presence) -> bool {
    !matches!(p, Presence::Required)
}

// ---------------------------------------------------------------------
// Field-list assembly
// ---------------------------------------------------------------------

/// The type-specific fields plus the format's common fields, deduplicated by
/// field name with the type-specific entry winning.
fn merged_fields<'a>(specific: &'a [FieldSpec], common: &'a [FieldSpec]) -> Vec<&'a FieldSpec> {
    let mut out: Vec<&FieldSpec> = specific.iter().collect();
    for c in common {
        if !out.iter().any(|f| f.sfield == c.sfield) {
            out.push(c);
        }
    }
    out
}

fn sfield_index(formats: &ProtocolFormats) -> BTreeMap<&str, &SFieldDef> {
    formats
        .sfields
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect()
}

struct Accessor<'a> {
    spec: &'a FieldSpec,
    def: &'a SFieldDef,
    name: String,
    access: Access,
    /// This *field's* tier, which is not always the view's.
    ///
    /// A derived field tier is the best among the formats using it, so it is
    /// normally at least as available as any view carrying it — and then a
    /// `field_overrides` entry can make it scarcer (`sfCredentialIDs` on an
    /// active `Payment`). When that happens the accessor, not the view, is
    /// what has to disappear or gain a `#[cfg]`.
    tier: Tier,
}

fn resolve<'a>(
    fields: &[&'a FieldSpec],
    sfields: &BTreeMap<&str, &'a SFieldDef>,
    view: &str,
    field_tiers: &BTreeMap<String, Tier>,
) -> Result<Vec<Accessor<'a>>> {
    let mut out = Vec::with_capacity(fields.len());
    let mut taken: BTreeSet<String> = BTreeSet::new();

    for spec in fields {
        let def = *sfields.get(spec.sfield.as_str()).ok_or_else(|| {
            anyhow!(
                "view `{view}` references undeclared field `{}`",
                spec.sfield
            )
        })?;
        let tier = field_tiers
            .get(&spec.sfield)
            .copied()
            .unwrap_or(Tier::Active);
        let name = method_name(&spec.sfield)
            .with_context(|| format!("naming `{}`'s accessor on view `{view}`", spec.sfield))?;
        let access = access(def.sti_code);

        // Every method name this field claims, checked against every name
        // already claimed on this view. A silent shadow here would be a
        // field the view simply cannot read.
        let claimed: Vec<String> = match access {
            Access::Value(_) => vec![name.clone()],
            Access::Raw => vec![format!("{name}_into")],
            Access::Container(_) => vec![format!("{name}_into"), format!("{name}_slot")],
        };
        for c in claimed {
            if !taken.insert(c.clone()) {
                bail!(
                    "view `{view}`: `{}` wants the accessor name `{c}`, which another of its \
                     fields already claims — codegen::views needs a disambiguation rule",
                    spec.sfield
                );
            }
        }

        out.push(Accessor {
            spec,
            def,
            name,
            access,
            tier,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// Accessor rendering
// ---------------------------------------------------------------------

/// Describes a field and any availability gate stricter than its view's.
fn field_doc(a: &Accessor<'_>, view_tier: Tier) -> String {
    let mut out = format!(
        "/// `{name}` — {ty}, `{presence}`.\n",
        name = a.def.name,
        ty = type_id_name(a.def.sti_code),
        presence = presence_token(a.spec.presence),
    );
    if a.tier > view_tier {
        out.push_str(&tier_field_doc(a.tier));
    }
    out
}

/// Gates a field only when it is less available than its enclosing view.
fn field_cfg(a: &Accessor<'_>, view_tier: Tier) -> String {
    if a.tier > view_tier {
        a.tier
            .cfg_attr()
            .map(|c| format!("{c}\n"))
            .unwrap_or_default()
    } else {
        String::new()
    }
}

fn tier_field_doc(tier: Tier) -> String {
    match tier {
        Tier::Active => String::new(),
        Tier::Pending => format!(
            "///\n/// Pending field; excluded by the `{narrow}` feature.\n",
            narrow = crate::availability::ACTIVE_ONLY_FEATURE,
        ),
        Tier::Dormant => format!(
            "///\n/// Dormant field; requires the `{all}` feature.\n",
            all = crate::availability::ALL_FEATURE,
        ),
    }
}

fn optional_doc(p: Presence) -> &'static str {
    match p {
        Presence::Default => {
            "///\n\
             /// `Ok(None)` when omitted; `soeDEFAULT` defines no value to substitute.\n"
        }
        _ => "///\n/// `Ok(None)` when the field is absent.\n",
    }
}

/// Renders the value/raw accessors — the ones every source can serve.
fn push_shared_accessor(
    buf: &mut String,
    a: &Accessor<'_>,
    view_tier: Tier,
    visibility: &str,
    source: &str,
) -> Result<()> {
    let opt = is_optional(a.spec.presence);
    match a.access {
        Access::Value(ty) => {
            buf.push_str(&field_doc(a, view_tier));
            if opt {
                buf.push_str(optional_doc(a.spec.presence));
            }
            let ret = if opt {
                format!("crate::error::Result<Option<{ty}>>")
            } else {
                format!("crate::error::Result<{ty}>")
            };
            let call = if opt { "read_opt" } else { "read" };
            buf.push_str(&field_cfg(a, view_tier));
            writeln!(
                buf,
                "#[inline(always)]\n\
                 {visibility}fn {name}(&self) -> {ret} {{\n\
                 {source}.{call}(crate::sfield::{sf})\n\
                 }}\n",
                name = a.name,
                sf = a.def.name,
            )
            .context("writing a value accessor")?;
        }
        Access::Raw | Access::Container(_) => {
            let container = matches!(a.access, Access::Container(_));
            buf.push_str(&field_doc(a, view_tier));
            buf.push_str("///\n/// Writes the raw wire bytes to `out`.\n");
            if container {
                // A plain code span, not an intra-doc link: on a
                // transaction view this doc sits in the `impl<S:
                // FieldSource>` block, where `Self::{name}_slot` does not
                // resolve — that method exists only on the `SlotSource`
                // instantiation.
                writeln!(
                    buf,
                    "/// Use `{name}_slot` on a slot-backed view to navigate the container.",
                    name = a.name,
                )
                .context("writing a container accessor doc")?;
            }
            if opt {
                buf.push_str(optional_doc(a.spec.presence));
            }
            let ret = if opt {
                "crate::error::Result<Option<usize>>"
            } else {
                "crate::error::Result<usize>"
            };
            let call = if opt { "read_raw_opt" } else { "read_raw" };
            buf.push_str(&field_cfg(a, view_tier));
            writeln!(
                buf,
                "#[inline(always)]\n\
                 {visibility}fn {name}_into<B: AsMut<[u8]> + ?Sized>(&self, out: &mut B) -> {ret} {{\n\
                 {source}.{call}(crate::sfield::{sf}.code(), out)\n\
                 }}\n",
                name = a.name,
                sf = a.def.name,
            )
            .context("writing a raw accessor")?;
        }
    }
    Ok(())
}

/// Renders the `*_slot` child-slot accessor a container field gets on a
/// slot-backed view. Writes nothing for a non-container field.
fn push_slot_accessor(
    buf: &mut String,
    a: &Accessor<'_>,
    view_tier: Tier,
    visibility: &str,
    source: &str,
) -> Result<()> {
    let Access::Container(slot_ty) = a.access else {
        return Ok(());
    };
    let opt = is_optional(a.spec.presence);
    buf.push_str(&field_doc(a, view_tier));
    buf.push_str(
        "///\n\
         /// Returns an owned child slot. Clear or consume it to avoid exhausting slots.\n",
    );
    if opt {
        buf.push_str(optional_doc(a.spec.presence));
    }
    let inner = format!("crate::slot_obj::SlotObject<{slot_ty}>");
    let (ret, call) = if opt {
        (
            format!("crate::error::Result<Option<{inner}>>"),
            "subobject_opt",
        )
    } else {
        (format!("crate::error::Result<{inner}>"), "subobject")
    };
    buf.push_str(&field_cfg(a, view_tier));
    writeln!(
        buf,
        "#[inline(always)]\n\
         {visibility}fn {name}_slot(&self) -> {ret} {{\n\
         {source}.{call}(crate::sfield::{sf})\n\
         }}\n",
        name = a.name,
        sf = a.def.name,
    )
    .context("writing a slot accessor")?;
    Ok(())
}

/// Returns the gate and documentation for a view. The gate is repeated on
/// its impl blocks because a gated-out struct cannot have an ungated impl.
fn tier_prelude(tier: Tier) -> (String, String) {
    let Some(attr) = tier.cfg_attr() else {
        return (String::new(), String::new());
    };
    let doc = match tier {
        Tier::Active => String::new(),
        Tier::Pending => format!(
            "///\n/// Pending on Xahau mainnet; excluded by the `{narrow}` feature.\n",
            narrow = crate::availability::ACTIVE_ONLY_FEATURE,
        ),
        Tier::Dormant => format!(
            "///\n/// Dormant on Xahau mainnet; requires the `{all}` feature.\n",
            all = crate::availability::ALL_FEATURE,
        ),
    };
    (format!("{attr}\n"), doc)
}

/// Brings `FieldSource` methods into scope for concrete `SlotSource` views.
const SOURCE_TRAIT_IMPORT: &str = "\nuse crate::views::source::FieldSource as _;\n\n";

/// Guards against two formats claiming the same Rust type name in one
/// generated module.
fn check_unique(names: impl Iterator<Item = String>, what: &str) -> Result<()> {
    let mut seen = BTreeSet::new();
    for n in names {
        if !seen.insert(n.clone()) {
            bail!("two {what} formats are both named `{n}`");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// tx.rs
// ---------------------------------------------------------------------

const TX_MODULE_DOC: &str = "\
//! Transaction views: one struct per transaction type xahaud declares.
//!
//! Each is named exactly as upstream names the type ([`Payment`],
//! [`EscrowCreate`], …) and is generic over
//! [`FieldSource`](crate::views::source::FieldSource), so the same struct
//! reads the originating transaction directly
//! ([`OtxnSource`](crate::views::source::OtxnSource), via `Xxx::otxn()`) or
//! an already-loaded transaction slot
//! ([`SlotSource`](crate::views::source::SlotSource), via
//! `Xxx::from_slot()`). Both constructors verify the transaction type
//! first — a view never lies about its shape.
//!
//! A field the format declares `soeREQUIRED` reads as `Result<T>`; one
//! declared `soeOPTIONAL` or `soeDEFAULT` reads as `Result<Option<T>>`,
//! with absence reported as `Ok(None)`. A field whose serialized type this
//! crate models no typed read for is reachable as raw wire bytes through a
//! `…_into` accessor. `STObject`/`STArray` fields have that too, plus a
//! `…_slot` child-slot accessor on the slot-backed views only — navigating
//! into a container is something `otxn_field` cannot do.
//!
//! The fields every transaction carries (`sfAccount`, `sfFee`, `sfMemos`, …)
//! are served once, as [`TransactionCommonFields`] default methods — plus
//! [`TransactionCommonSlotFields`] for their container navigation — rather
//! than repeated on each view. Both traits are implemented for every view
//! here and re-exported through `rshooks::prelude`; import one of those to
//! call the common accessors.
//!
//! See [`crate::views`] for when to reach for a view at all, and
//! [`crate::views::source`] for what one costs.
";

/// Renders `crates/rshooks/src/views/tx.rs`.
pub fn generate_tx(formats: &ProtocolFormats, availability: &FormatAvailability) -> Result<String> {
    let sfields = sfield_index(formats);
    let field_tiers = availability.field_tiers(formats);
    check_unique(
        formats.transactions.iter().map(|t| t.name.clone()),
        "transaction",
    )?;

    let mut body = String::from(SOURCE_TRAIT_IMPORT);
    push_tx_common_traits(&mut body, formats, &sfields, &field_tiers)?;
    for tx in &formats.transactions {
        let tier = availability.tx(&tx.name);
        push_tx_view(&mut body, tx, formats, &sfields, tier, &field_tiers)
            .with_context(|| format!("rendering the `{}` transaction view", tx.name))?;
    }
    Ok(with_generated_marker_in("xahaud-protocol", "transactions.macro", TX_MODULE_DOC) + &body)
}

/// Renders the transaction-common fields once as default trait methods.
/// Every generated transaction view implements the first trait; slot-backed
/// views receive the container navigation methods through the blanket second
/// trait implementation.
fn push_tx_common_traits(
    buf: &mut String,
    formats: &ProtocolFormats,
    sfields: &BTreeMap<&str, &SFieldDef>,
    field_tiers: &BTreeMap<String, Tier>,
) -> Result<()> {
    let fields: Vec<&FieldSpec> = formats.tx_common.iter().collect();
    let accessors = resolve(&fields, sfields, "transaction common fields", field_tiers)?;
    let mut shared = String::new();
    let mut slot_only = String::new();
    for a in &accessors {
        push_shared_accessor(&mut shared, a, Tier::Active, "", "self.field_source()")?;
        push_slot_accessor(&mut slot_only, a, Tier::Active, "", "self.field_source()")?;
    }

    writeln!(
        buf,
        "/// Accessors shared by every generated transaction view.\n\
         ///\n\
         /// This trait is implemented automatically; import it to call common-field\n\
         /// methods on a transaction view.\n\
         pub trait TransactionCommonFields {{\n\
         /// The backing source used by this transaction view.\n\
         #[doc(hidden)]\n\
         type Source: crate::views::source::FieldSource;\n\
         \n\
         /// Returns the backing field source.\n\
         #[doc(hidden)]\n\
         fn field_source(&self) -> &Self::Source;\n\
         \n\
         {shared}\
         }}\n\
         \n\
         /// Transaction-common container accessors available on slot-backed views.\n\
         pub trait TransactionCommonSlotFields:\n\
         TransactionCommonFields<Source = crate::views::source::SlotSource>\n\
         {{\n\
         {slot_only}\
         }}\n\
         \n\
         impl<T> TransactionCommonSlotFields for T where\n\
         T: TransactionCommonFields<Source = crate::views::source::SlotSource>\n\
         {{}}\n"
    )
    .context("writing transaction common-field traits")?;
    Ok(())
}

fn push_tx_view(
    buf: &mut String,
    tx: &TxFormat,
    formats: &ProtocolFormats,
    sfields: &BTreeMap<&str, &SFieldDef>,
    tier: Tier,
    field_tiers: &BTreeMap<String, Tier>,
) -> Result<()> {
    let fields = merged_fields(&tx.fields, &formats.tx_common);
    // Validate collisions across the complete effective format, including a
    // type-specific field which overrides a common declaration.
    let _ = resolve(&fields, sfields, &tx.name, field_tiers)?;
    let specific_fields: Vec<&FieldSpec> = tx.fields.iter().collect();
    let accessors = resolve(&specific_fields, sfields, &tx.name, field_tiers)?;
    let name = &tx.name;
    let (cfg, tier_doc) = tier_prelude(tier);

    writeln!(
        buf,
        "/// View of the `{name}` transaction (`{tag}`, type code {value}).\n\
         {tier_doc}{cfg}pub struct {name}<S: crate::views::source::FieldSource> {{\n\
         src: S,\n\
         }}\n",
        tag = tx.tag,
        value = tx.value,
    )
    .context("writing the view struct")?;

    writeln!(
        buf,
        "{cfg}impl {name}<crate::views::source::OtxnSource> {{\n\
         /// Views the originating transaction as `{name}`, checking its type.\n\
         #[inline(always)]\n\
         pub fn otxn() -> crate::error::Result<Self> {{\n\
         crate::views::source::otxn_of_type(rshooks_core::{tag}).map(|src| Self {{ src }})\n\
         }}\n\
         }}\n",
        tag = tx.tag,
    )
    .context("writing the otxn constructor")?;

    let mut slot_only = String::new();
    writeln!(
        slot_only,
        "/// Takes a transaction slot after verifying `sfTransactionType` is `{tag}`.\n\
         /// A failed check best-effort clears the consumed slot.\n\
         #[inline(always)]\n\
         pub fn from_slot(\n\
         obj: crate::slot_obj::SlotObject<crate::types::STObject>,\n\
         ) -> crate::error::Result<Self> {{\n\
         crate::views::source::slot_of_type(\n\
         obj,\n\
         crate::sfield::sfTransactionType,\n\
         rshooks_core::{tag},\n\
         )\n\
         .map(|src| Self {{ src }})\n\
         }}\n\
         \n\
         /// Consumes the view and returns its slot.\n\
         #[inline(always)]\n\
         pub fn into_slot(self) -> crate::slot_obj::SlotObject<crate::types::STObject> {{\n\
         self.src.into_slot()\n\
         }}\n",
        tag = tx.tag,
    )
    .context("writing the slot constructor")?;

    let mut shared = String::new();
    for a in &accessors {
        push_shared_accessor(&mut shared, a, tier, "pub ", "self.src")?;
        push_slot_accessor(&mut slot_only, a, tier, "pub ", "self.src")?;
    }

    writeln!(
        buf,
        "{cfg}impl {name}<crate::views::source::SlotSource> {{\n{slot_only}}}\n"
    )
    .context("writing the slot impl")?;
    writeln!(
        buf,
        "{cfg}impl<S: crate::views::source::FieldSource> {name}<S> {{\n{shared}}}\n"
    )
    .context("writing the shared impl")?;
    writeln!(
        buf,
        "{cfg}impl<S: crate::views::source::FieldSource> TransactionCommonFields for {name}<S> {{\n\
         type Source = S;\n\
         \n\
         #[inline(always)]\n\
         fn field_source(&self) -> &Self::Source {{\n\
         &self.src\n\
         }}\n\
         }}\n"
    )
    .context("writing the transaction common-field trait impl")?;
    Ok(())
}

// ---------------------------------------------------------------------
// ledger.rs
// ---------------------------------------------------------------------

const LEDGER_MODULE_DOC: &str = "\
//! Ledger-entry views: one struct per ledger entry type xahaud declares.
//!
//! Each is named exactly as upstream names the type ([`AccountRoot`],
//! [`RippleState`], …). Unlike [`crate::views::tx`]'s views these are not
//! generic: a ledger object only ever reaches a hook through a slot, so
//! every one of them wraps a
//! [`SlotSource`](crate::views::source::SlotSource) and every constructor
//! verifies `sfLedgerEntryType` before handing the view back.
//!
//! `Xxx::from_keylet` is the usual entry point; `Xxx::from_slot` takes a
//! slot you already have, and `Xxx::into_slot` gives it back. Keylet
//! construction itself stays in [`crate::api::keylet`] — how a keylet is
//! parameterized is per-type knowledge upstream's format macros do not
//! encode, so it is not generated.
//!
//! Presence and value-type rules are [`crate::views::tx`]'s, unchanged. A
//! name that is both a transaction type and a ledger entry type
//! (`DepositPreauth`) is two different structs in two different modules.
//!
//! The fields every ledger entry carries (`sfLedgerEntryType`, `sfFlags`, …)
//! are served once, as [`LedgerEntryCommonFields`] default methods,
//! implemented for every view here and re-exported through
//! `rshooks::prelude`; import either to call the common accessors.
";

/// Renders `crates/rshooks/src/views/ledger.rs`.
pub fn generate_ledger(
    formats: &ProtocolFormats,
    availability: &FormatAvailability,
) -> Result<String> {
    let sfields = sfield_index(formats);
    let field_tiers = availability.field_tiers(formats);
    check_unique(
        formats.ledger_entries.iter().map(|l| l.name.clone()),
        "ledger entry",
    )?;

    let mut body = String::from(SOURCE_TRAIT_IMPORT);
    push_ledger_common_trait(&mut body, formats, &sfields, &field_tiers)?;
    for le in &formats.ledger_entries {
        let tier = availability.ledger_entry(&le.name);
        push_ledger_view(&mut body, le, formats, &sfields, tier, &field_tiers)
            .with_context(|| format!("rendering the `{}` ledger entry view", le.name))?;
    }
    Ok(
        with_generated_marker_in("xahaud-protocol", "ledger_entries.macro", LEDGER_MODULE_DOC)
            + &body,
    )
}

/// Renders ledger-entry common fields once as default trait methods.
fn push_ledger_common_trait(
    buf: &mut String,
    formats: &ProtocolFormats,
    sfields: &BTreeMap<&str, &SFieldDef>,
    field_tiers: &BTreeMap<String, Tier>,
) -> Result<()> {
    let fields: Vec<&FieldSpec> = formats.le_common.iter().collect();
    let accessors = resolve(&fields, sfields, "ledger-entry common fields", field_tiers)?;
    let mut methods = String::new();
    for a in &accessors {
        push_shared_accessor(&mut methods, a, Tier::Active, "", "self.field_source()")?;
        push_slot_accessor(&mut methods, a, Tier::Active, "", "self.field_source()")?;
    }
    writeln!(
        buf,
        "/// Accessors shared by every generated ledger-entry view.\n\
         ///\n\
         /// This trait is implemented automatically; import it to call common-field\n\
         /// methods on a ledger-entry view.\n\
         pub trait LedgerEntryCommonFields {{\n\
         /// Returns the backing field source.\n\
         #[doc(hidden)]\n\
         fn field_source(&self) -> &crate::views::source::SlotSource;\n\
         \n\
         {methods}\
         }}\n"
    )
    .context("writing ledger-entry common-field trait")?;
    Ok(())
}

fn push_ledger_view(
    buf: &mut String,
    le: &LedgerEntryFormat,
    formats: &ProtocolFormats,
    sfields: &BTreeMap<&str, &SFieldDef>,
    tier: Tier,
    field_tiers: &BTreeMap<String, Tier>,
) -> Result<()> {
    let fields = merged_fields(&le.fields, &formats.le_common);
    // Validate the complete effective format, then render only type-specific
    // fields here. Common accessors live on `LedgerEntryCommonFields`.
    let _ = resolve(&fields, sfields, &le.name, field_tiers)?;
    let specific_fields: Vec<&FieldSpec> = le.fields.iter().collect();
    let accessors = resolve(&specific_fields, sfields, &le.name, field_tiers)?;
    let name = &le.name;
    let (cfg, tier_doc) = tier_prelude(tier);

    writeln!(
        buf,
        "/// View of the `{name}` ledger object (`{tag}`, type code 0x{value:04x}, RPC\n\
         /// name `{rpc}`).\n\
         {tier_doc}{cfg}pub struct {name} {{\n\
         src: crate::views::source::SlotSource,\n\
         }}\n\
         \n\
         {cfg}impl {name} {{\n\
         /// Loads and type-checks the ledger object identified by `keylet`.\n\
         #[inline(always)]\n\
         pub fn from_keylet(keylet: &crate::types::Keylet) -> crate::error::Result<Self> {{\n\
         Self::from_slot(crate::slot_obj::SlotObject::from_keylet(keylet)?)\n\
         }}\n\
         \n\
         /// Takes a ledger-entry slot after verifying `sfLedgerEntryType` is `{tag}`.\n\
         /// A failed check best-effort clears the consumed slot.\n\
         #[inline(always)]\n\
         pub fn from_slot(\n\
         obj: crate::slot_obj::SlotObject<crate::types::STObject>,\n\
         ) -> crate::error::Result<Self> {{\n\
         crate::views::source::slot_of_type(\n\
         obj,\n\
         crate::sfield::sfLedgerEntryType,\n\
         rshooks_core::{tag},\n\
         )\n\
         .map(|src| Self {{ src }})\n\
         }}\n\
         \n\
         /// Consumes the view and returns its slot.\n\
         #[inline(always)]\n\
         pub fn into_slot(self) -> crate::slot_obj::SlotObject<crate::types::STObject> {{\n\
         self.src.into_slot()\n\
         }}\n",
        tag = le.tag,
        value = le.value,
        rpc = le.rpc_name,
    )
    .context("writing the view struct and constructors")?;

    for a in &accessors {
        push_shared_accessor(buf, a, tier, "pub ", "self.src")?;
        push_slot_accessor(buf, a, tier, "pub ", "self.src")?;
    }
    writeln!(
        buf,
        "}}\n\n\
         {cfg}impl LedgerEntryCommonFields for {name} {{\n\
         #[inline(always)]\n\
         fn field_source(&self) -> &crate::views::source::SlotSource {{\n\
         &self.src\n\
         }}\n\
         }}\n"
    )
    .context("writing ledger-entry common-field trait impl")?;
    Ok(())
}

// ---------------------------------------------------------------------
// inner.rs
// ---------------------------------------------------------------------

const INNER_MODULE_DOC: &str = "\
//! Inner-object views: one struct per inner-object format xahaud declares.
//!
//! These are the object-typed fields that carry a format of their own —
//! [`EmitDetails`], [`Signer`], [`HookParameter`], [`HookExecution`], … — reached by
//! navigating into a parent slot, typically the `…_slot` accessor of a
//! [`crate::views::tx`] or [`crate::views::ledger`] view, or an element of
//! an `STArray`.
//!
//! `from_slot` here verifies nothing and cannot fail: an inner object
//! carries no type field to check, so this is a plain typed wrapper. That is
//! the one thing that distinguishes these from the ledger views; presence,
//! value types and slot lifetime are identical.
";

/// Renders `crates/rshooks/src/views/inner.rs`.
pub fn generate_inner(
    formats: &ProtocolFormats,
    availability: &FormatAvailability,
) -> Result<String> {
    let sfields = sfield_index(formats);
    let field_tiers = availability.field_tiers(formats);
    let names = formats
        .inner_objects
        .iter()
        .map(|i| inner_name(&i.sfield))
        .collect::<Result<Vec<_>>>()?;
    check_unique(names.into_iter(), "inner object")?;

    let mut body = String::from(SOURCE_TRAIT_IMPORT);
    for obj in &formats.inner_objects {
        let tier = availability.inner_object(&inner_name(&obj.sfield)?);
        push_inner_view(&mut body, obj, &sfields, tier, &field_tiers)
            .with_context(|| format!("rendering the `{}` inner-object view", obj.sfield))?;
    }
    Ok(with_generated_marker_in(
        "xahaud-protocol",
        "InnerObjectFormats.cpp",
        INNER_MODULE_DOC,
    ) + &body)
}

/// The struct name for an inner-object format: its field name without the
/// `sf` prefix (`sfEmitDetails` → `EmitDetails`).
fn inner_name(sfield: &str) -> Result<String> {
    sfield
        .strip_prefix("sf")
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("expected an `sf`-prefixed field name, got `{sfield}`"))
}

fn push_inner_view(
    buf: &mut String,
    obj: &InnerObjectFormat,
    sfields: &BTreeMap<&str, &SFieldDef>,
    tier: Tier,
    field_tiers: &BTreeMap<String, Tier>,
) -> Result<()> {
    let name = inner_name(&obj.sfield)?;
    let (cfg, tier_doc) = tier_prelude(tier);
    let fields: Vec<&FieldSpec> = obj.fields.iter().collect();
    let accessors = resolve(&fields, sfields, &name, field_tiers)?;
    let ty = type_id_name(
        sfields
            .get(obj.sfield.as_str())
            .ok_or_else(|| anyhow!("`{}` is not declared in sfields.macro", obj.sfield))?
            .sti_code,
    );

    writeln!(
        buf,
        "/// View of the `{sf}` inner object ({ty}).\n\
         {tier_doc}{cfg}pub struct {name} {{\n\
         src: crate::views::source::SlotSource,\n\
         }}\n\
         \n\
         {cfg}impl {name} {{\n\
         /// Takes a child slot without type-checking it; inner objects have no type field.\n\
         #[inline(always)]\n\
         #[must_use]\n\
         pub fn from_slot(obj: crate::slot_obj::SlotObject<crate::types::STObject>) -> Self {{\n\
         Self {{\n\
         src: crate::views::source::SlotSource::new(obj),\n\
         }}\n\
         }}\n\
         \n\
         /// Consumes the view and returns its slot.\n\
         #[inline(always)]\n\
         pub fn into_slot(self) -> crate::slot_obj::SlotObject<crate::types::STObject> {{\n\
         self.src.into_slot()\n\
         }}\n",
        sf = obj.sfield,
    )
    .context("writing the view struct and constructors")?;

    for a in &accessors {
        push_shared_accessor(buf, a, tier, "pub ", "self.src")?;
        push_slot_accessor(buf, a, tier, "pub ", "self.src")?;
    }
    buf.push_str("}\n\n");
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Test code is exempt from the workspace's panic-freedom lints
    //! (`docs/DESIGN.md` §8).
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use super::*;

    const SFIELDS: &str =
        include_str!("../../../rshooks-core/vendor/xahaud-protocol/sfields.macro");

    fn corpus() -> ProtocolFormats {
        let json = include_str!("../../../rshooks-core/protocol_formats.json");
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"))
    }

    /// The checked-in curated classification — what `gen-core` actually
    /// renders with.
    fn availability() -> FormatAvailability {
        let json = include_str!("../../../rshooks-core/format_availability.json");
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Everything `active`, for the tests that assert on rendering shape
    /// rather than on availability: those want every format present.
    fn all_active(f: &ProtocolFormats) -> FormatAvailability {
        let mut a = FormatAvailability::empty();
        for t in &f.transactions {
            a.transactions.insert(t.name.clone(), Tier::Active);
        }
        for l in &f.ledger_entries {
            a.ledger_entries.insert(l.name.clone(), Tier::Active);
        }
        for i in &f.inner_objects {
            a.inner_objects
                .insert(inner_name(&i.sfield).unwrap_or_default(), Tier::Active);
        }
        a
    }

    #[test]
    fn field_names_become_the_accessor_names_upstream_spelling_implies() {
        let cases = [
            ("sfAmount", "amount"),
            ("sfDestinationTag", "destination_tag"),
            ("sfLedgerEntryType", "ledger_entry_type"),
            // Ordinary capital runs, handled without a table entry.
            ("sfImportVLKeys", "import_vl_keys"),
            ("sfMPTAmount", "mpt_amount"),
            ("sfEPrice", "e_price"),
            ("sfLPTokenBalance", "lp_token_balance"),
            ("sfURI", "uri"),
            // Table entries, and the trailing lowercase they absorb.
            ("sfAMMID", "amm_id"),
            ("sfCredentialIDs", "credential_ids"),
            ("sfNFToken", "nftoken"),
            ("sfNFTokens", "nftokens"),
            ("sfNFTokenTaxon", "nftoken_taxon"),
            ("sfMPTokenIssuanceID", "mptoken_issuance_id"),
            ("sfXChainClaimID", "xchain_claim_id"),
            ("sfDIDDocument", "did_document"),
            ("sfUNLModifyValidator", "unl_modify_validator"),
        ];
        for (field, expected) in cases {
            assert_eq!(method_name(field).unwrap(), expected, "{field}");
        }
    }

    #[test]
    fn a_keyword_accessor_name_becomes_a_raw_identifier() {
        assert_eq!(method_name("sfType").unwrap(), "r#type");
        assert_eq!(method_name("sfMatch").unwrap(), "r#match");
        // The four with no raw form are a hard error, not a mangled name.
        let msg = format!("{:#}", method_name("sfSuper").unwrap_err());
        assert!(msg.contains("no raw-identifier form"), "{msg}");
    }

    #[test]
    fn a_name_without_the_sf_prefix_is_an_error() {
        let msg = format!("{:#}", method_name("Amount").unwrap_err());
        assert!(msg.contains("`sf`-prefixed"), "{msg}");
    }

    /// The whole point of failing hard on a collision: two fields mapping
    /// to one accessor name would silently leave one of them unreadable.
    #[test]
    fn two_fields_claiming_one_accessor_name_are_a_hard_error() {
        let sfields = vec![
            SFieldDef {
                name: "sfMemos".into(),
                sti: "ARRAY".into(),
                sti_code: 15,
                field_code: 9,
                code: (15 << 16) | 9,
                typed: false,
                extras: Vec::new(),
            },
            // A `UInt32` field whose accessor is `memos_slot` — exactly the
            // name `sfMemos`'s child-slot accessor takes.
            SFieldDef {
                name: "sfMemosSlot".into(),
                sti: "UINT32".into(),
                sti_code: 2,
                field_code: 99,
                code: (2 << 16) | 99,
                typed: true,
                extras: Vec::new(),
            },
        ];
        let field = |n: &str| FieldSpec {
            sfield: n.into(),
            presence: Presence::Required,
            extras: Vec::new(),
        };
        let formats = ProtocolFormats {
            version: 1,
            sfields,
            tx_common: Vec::new(),
            le_common: Vec::new(),
            transactions: vec![TxFormat {
                tag: "ttPAYMENT".into(),
                value: 0,
                name: "Payment".into(),
                fields: vec![field("sfMemos"), field("sfMemosSlot")],
            }],
            ledger_entries: Vec::new(),
            inner_objects: Vec::new(),
        };
        let av = all_active(&formats);
        let msg = format!("{:#}", generate_tx(&formats, &av).unwrap_err());
        assert!(
            msg.contains("memos_slot") && msg.contains("already claims"),
            "{msg}"
        );
    }

    /// The renderer rule the zero-cost argument rests on: nothing that
    /// survives `rshooks-build`'s cleaner regardless of reachability
    /// (`docs/DESIGN.md` §6.2 — active data segments are retained, unused
    /// functions are not).
    #[test]
    fn the_generated_code_has_no_statics_exports_or_function_pointers() {
        let formats = corpus();
        let av = availability();
        for (label, text) in [
            ("tx.rs", generate_tx(&formats, &av).unwrap()),
            ("ledger.rs", generate_ledger(&formats, &av).unwrap()),
            ("inner.rs", generate_inner(&formats, &av).unwrap()),
        ] {
            for needle in [
                "static ",
                "#[no_mangle]",
                "#[unsafe(no_mangle)]",
                "extern \"C\"",
                "fn(",
                "const ",
            ] {
                assert!(
                    !text.contains(needle),
                    "{label} contains `{needle}`, which the zero-cost rule forbids"
                );
            }
            assert!(text.contains("#[inline(always)]"), "{label}");
        }
    }

    #[test]
    fn rendering_is_deterministic_and_covers_the_whole_corpus() {
        let formats = corpus();
        let av = all_active(&formats);
        let real = availability();
        type Render = fn(&ProtocolFormats, &FormatAvailability) -> Result<String>;
        for (label, render) in [
            ("tx.rs", generate_tx as Render),
            ("ledger.rs", generate_ledger),
            ("inner.rs", generate_inner),
        ] {
            let once = render(&formats, &av).unwrap();
            let twice = render(&formats, &av).unwrap();
            assert_eq!(once, twice, "{label} is not deterministic");
            // Determinism has to hold under the curated classification too:
            // the `#[cfg]` attributes are part of the rendered text.
            assert_eq!(
                render(&formats, &real).unwrap(),
                render(&formats, &real).unwrap(),
                "{label} is not deterministic under the curated classification"
            );
        }

        // With everything active, every declared format is rendered.
        let tx = generate_tx(&formats, &av).unwrap();
        for t in &formats.transactions {
            assert!(
                tx.contains(&format!("pub struct {}<", t.name)),
                "no view for {}",
                t.name
            );
        }
        let ledger = generate_ledger(&formats, &av).unwrap();
        for l in &formats.ledger_entries {
            assert!(
                ledger.contains(&format!("pub struct {} {{", l.name)),
                "no view for {}",
                l.name
            );
        }
        let inner = generate_inner(&formats, &av).unwrap();
        for i in &formats.inner_objects {
            let name = inner_name(&i.sfield).unwrap();
            assert!(inner.contains(&format!("pub struct {name} {{")), "{name}");
        }
    }

    /// The presence and value-type rules, checked on one view whose shape
    /// is stable and recognizable rather than on all 74.
    #[test]
    fn the_payment_view_maps_presence_and_value_types_as_specified() {
        let formats = corpus();
        let tx = generate_tx(&formats, &all_active(&formats)).unwrap();
        let payment = tx
            .split("pub struct ")
            .find(|s| s.starts_with("Payment<"))
            .expect("no Payment view");

        // soeREQUIRED -> Result<T>; AMOUNT -> AmountBytes, ACCOUNT -> AccountId.
        assert!(payment.contains(
            "pub fn amount(&self) -> crate::error::Result<crate::slot_obj::AmountBytes>"
        ));
        assert!(payment.contains(
            "pub fn destination(&self) -> crate::error::Result<crate::types::AccountId>"
        ));
        // soeOPTIONAL -> Result<Option<T>>.
        assert!(
            payment.contains("pub fn destination_tag(&self) -> crate::error::Result<Option<u32>>")
        );
        // Unmodeled serialized type -> a raw `*_into` accessor only.
        assert!(payment.contains("pub fn paths_into<B: AsMut<[u8]> + ?Sized>"));
        assert!(!payment.contains("pub fn paths(&self)"));
        // Common fields render once, as default trait methods, not on the
        // per-type views.
        let common = tx
            .split("pub trait TransactionCommonFields")
            .nth(1)
            .expect("no TransactionCommonFields trait");
        let common = common
            .split("pub trait TransactionCommonSlotFields")
            .next()
            .expect("no TransactionCommonSlotFields trait");
        assert!(common.contains("fn signing_pub_key_into"));
        assert!(common.contains("fn memos_into<B: AsMut<[u8]> + ?Sized>"));
        assert!(!payment.contains("fn signing_pub_key_into"));
        assert!(!payment.contains("fn memos_into"));
        // A common container: raw bytes on the base trait, a child slot on
        // the slot-source-bounded trait only.
        assert!(!common.contains("fn memos_slot(&self)"));
        let slot_common = tx
            .split("pub trait TransactionCommonSlotFields")
            .nth(1)
            .expect("no TransactionCommonSlotFields trait")
            .split("pub struct ")
            .next()
            .expect("unterminated trait section");
        assert!(slot_common.contains("fn memos_slot(&self)"));
        // Every view wires itself into the common trait.
        assert!(payment.contains(
            "impl<S: crate::views::source::FieldSource> TransactionCommonFields for Payment<S>"
        ));
    }

    /// Every field any format references has an accessor name, and no
    /// format has an internal collision — the properties [`resolve`] would
    /// otherwise only discover on the day upstream adds the offending
    /// field.
    #[test]
    fn the_whole_corpus_names_and_resolves_without_collisions() {
        let formats = corpus();
        let av = all_active(&formats);
        assert!(generate_tx(&formats, &av).is_ok());
        assert!(generate_ledger(&formats, &av).is_ok());
        assert!(generate_inner(&formats, &av).is_ok());

        // The vendored sfields file is the authority on which names exist;
        // check the namer copes with every one of them, not only the ones
        // some format happens to reference today.
        for line in SFIELDS.lines() {
            let Some(rest) = line
                .trim()
                .strip_prefix("TYPED_SFIELD(")
                .or_else(|| line.trim().strip_prefix("UNTYPED_SFIELD("))
            else {
                continue;
            };
            let Some((name, _)) = rest.split_once(',') else {
                continue;
            };
            let name = name.trim();
            assert!(method_name(name).is_ok(), "{name} has no accessor name");
        }
    }

    #[test]
    fn the_acronym_table_is_ordered_longest_first() {
        // The matcher takes the first entry that matches, so a shorter
        // prefix listed ahead of a longer one would shadow it (`ID` before
        // `IDs`-style entries, were any added).
        let mut sorted = ACRONYMS.to_vec();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.len()));
        assert_eq!(ACRONYMS, sorted.as_slice());
    }
}
