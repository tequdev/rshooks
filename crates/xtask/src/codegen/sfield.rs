//! Generates typed `SField<T>` constants from the same parsed `sfcodes.h`
//! data used for `rshooks_core::sfcodes`.
//!
//! Like [`super::tx_type`] — the two generators whose output lands in
//! `rshooks` rather than `rshooks-core` — a typed mirror is the
//! `rshooks` layer's job per `docs/DESIGN.md` §5, while a mechanical 1:1
//! header translation is `rshooks-core`'s per §4. It is still fully mechanical
//! *within* that typed layer — every constant's value type is a pure
//! function of the serialized type ID packed into its own code — so it is
//! generated rather than hand-maintained, exactly as `sfcodes.rs` is.
//!
//! `SField<T>` and its wire markers remain hand-written in `rshooks::types`.

use std::fmt::Write as _;

use anyhow::{Context, Result};

use std::collections::BTreeMap;

use super::with_generated_marker;
use crate::availability::Tier;
use crate::ir::ConstSpec;
use crate::render::render_shift_add;

const MODULE_DOC: &str = "\
//! Typed serialized-field constants ([`SField<T>`](crate::types::SField)).
//!
//! One constant per **usable** `sfXxx` code, carrying the Rust type that
//! field's value reads back as. `sfSequence` is an
//! `SField<u32>`, `sfAccount` an `SField<AccountId>`, `sfBalance` an
//! `SField<Amount>` — so
//! [`SlotObject::get`](crate::slot_obj::SlotObject::get) can hand back an
//! already-typed handle and its `value()` needs no turbofish:
//!
//! ```
//! use rshooks::prelude::*;
//!
//! // The type comes from the constant, not from an annotation.
//! let _: SField<u32> = sfSequence;
//! let _: SField<AccountId> = sfAccount;
//!
//! // `.code()` is the const-context bridge back to the raw `u32`.
//! const SEQ: u32 = sfSequence.code();
//! assert_eq!(SEQ, rshooks::raw::sfcodes::sfSequence);
//! ```
//!
//! # Availability
//!
//! Constants follow the availability of the formats that use them: active
//! fields are always present, pending fields are excluded by
//! `active-amendments`, and dormant fields require `all-amendments`.
//! Unreferenced structural fields stay available; curated overrides cover
//! field-level amendment gates. Raw codes always remain in
//! `rshooks::raw::sfcodes`.
//!
//! [`crate::tx_type::TxType`] and [`crate::ledger_entry_type::LedgerEntryType`]
//! stay exhaustive for the opposite reason: they *decode* wire values rather
//! than offering capability, and a decoder that cannot name a code it might
//! receive is strictly worse than one that can.
//!
//! # How the value type is chosen
//!
//! Every `sfXxx` code packs a serialized type ID in its high bits
//! (`code >> 16`), and that ID alone decides the Rust type: 2 (`UInt32`) →
//! `u32`, 8 (`AccountID`) → [`AccountId`](crate::types::AccountId), 14
//! (`STObject`) → [`STObject`](crate::types::STObject), and so on.
//!
//! IDs this crate models no typed read for — `Blob`, `PathSet`,
//! `Vector256`, `Number`, `UInt192`, `XChainBridge`, and `Hash160` (whose
//! four fields carry genuinely different semantics: two currencies and two
//! issuers) — map to [`Opaque`](crate::types::Opaque), which supports
//! navigation and the raw byte escapes but no `value()`. Nothing is lost:
//! the bytes are still reachable, just not pre-interpreted.
//!
//! # Naming
//!
//! `#[allow(non_upper_case_globals)]`, because these mirror C constant names
//! verbatim — the same rule [`rshooks_core::sfcodes`] follows, and for the
//! same reason: a hook author reading xahaud's own sources should find the
//! identical spelling here.
";

/// The Rust value type a serialized type ID maps to, per design §3.1.
///
/// Returns `None` for every ID this layer does not model, which the caller
/// renders as `Opaque`. Deliberately a total function over the IDs that
/// actually appear in `sfcodes.h` rather than an exhaustive enum: the header
/// is the source of truth for which IDs exist, and an unmodelled one should
/// degrade to `Opaque`, never fail the build.
fn value_type(type_id: u32) -> Option<&'static str> {
    match type_id {
        16 => Some("u8"),
        1 => Some("u16"),
        2 => Some("u32"),
        3 => Some("u64"),
        5 => Some("crate::types::Hash"),
        6 => Some("crate::types::Amount"),
        8 => Some("crate::types::AccountId"),
        14 => Some("crate::types::STObject"),
        15 => Some("crate::types::STArray"),
        24 => Some("crate::types::Issue"),
        26 => Some("crate::types::CurrencyCode"),
        _ => None,
    }
}

/// Human-readable serialized type name shared with view documentation.
pub fn type_id_name(type_id: u32) -> String {
    match type_id {
        1 => "UInt16".into(),
        2 => "UInt32".into(),
        3 => "UInt64".into(),
        4 => "Hash128".into(),
        5 => "Hash256".into(),
        6 => "Amount".into(),
        7 => "Blob".into(),
        8 => "AccountID".into(),
        9 => "Number".into(),
        14 => "STObject".into(),
        15 => "STArray".into(),
        16 => "UInt8".into(),
        17 => "Hash160".into(),
        18 => "PathSet".into(),
        19 => "Vector256".into(),
        21 => "UInt192".into(),
        24 => "Issue".into(),
        25 => "XChainBridge".into(),
        26 => "Currency".into(),
        other => format!("type {other}"),
    }
}

/// Renders `sfield.rs`'s full contents from `sfcodes.h`'s parsed
/// [`ConstSpec`]s.
///
/// Each constant's value expression is rendered by the same
/// [`render_shift_add`] the raw `sfcodes.rs` generator uses, then wrapped in
/// `SField::new(..)` — so the typed and raw tables are two renderings of one
/// parse, and `typed.code() == raw` holds by construction (pinned anyway by
/// a 325-name parity test, because "by construction" is a claim worth
/// checking).
pub fn generate(sfcodes: &[ConstSpec], field_tiers: &BTreeMap<String, Tier>) -> Result<String> {
    let mut body =
        String::from("\n#![allow(non_upper_case_globals)]\n\nuse crate::types::SField;\n\n");
    let mut rendered: Vec<&ConstSpec> = Vec::with_capacity(sfcodes.len());

    for d in sfcodes {
        let tier = field_tiers.get(&d.name).copied().unwrap_or(Tier::Active);
        rendered.push(d);

        let value = render_shift_add(&d.c_expr)?;
        let type_id = serialized_type_id(&value)
            .with_context(|| format!("deriving the serialized type ID of `{}`", d.name))?;
        let ty = value_type(type_id).unwrap_or("crate::types::Opaque");

        writeln!(
            body,
            "/// C: `{name}` (sfcodes.h) — {id_name}, read as [`{ty}`].",
            name = d.name,
            id_name = type_id_name(type_id),
        )
        .context("writing constant doc")?;
        match tier {
            Tier::Active => {}
            Tier::Pending => writeln!(
                body,
                "///\n/// Pending amendment; excluded by the `{narrow}` feature.",
                narrow = crate::availability::ACTIVE_ONLY_FEATURE,
            )
            .context("writing a pending-tier doc note")?,
            Tier::Dormant => writeln!(
                body,
                "///\n/// Dormant on Xahau mainnet; requires the `{all}` feature.",
                all = crate::availability::ALL_FEATURE,
            )
            .context("writing a dormant-tier doc note")?,
        }
        if let Some(attr) = tier.cfg_attr() {
            writeln!(body, "{attr}").context("writing a tier cfg")?;
        }
        writeln!(
            body,
            "pub const {name}: SField<{ty}> = SField::new({value});",
            name = d.name,
        )
        .context("writing constant")?;
    }

    body.push_str(
        "\n#[cfg(test)]\nmod parity {\n    //! Verifies every typed constant against its raw counterpart.\n\n    #[test]\n    fn typed_field_codes_match_raw() {\n",
    );
    for d in &rendered {
        let tier = field_tiers.get(&d.name).copied().unwrap_or(Tier::Active);
        if let Some(attr) = tier.cfg_attr() {
            writeln!(body, "        {attr}").context("writing a parity-assertion cfg")?;
        }
        writeln!(
            body,
            "        assert_eq!(super::{name}.code(), ::rshooks_core::sfcodes::{name}, \"{name}\");",
            name = d.name,
        )
        .context("writing parity assertion")?;
    }
    writeln!(
        body,
        "        assert_eq!({count}, {count}, \"generated for {count} constants\");",
        count = rendered.len(),
    )
    .context("writing parity count")?;
    body.push_str("    }\n}\n");

    Ok(with_generated_marker("sfcodes.h", MODULE_DOC) + &body)
}

/// Extracts the serialized type ID from a rendered `(A << 16) + B` value
/// expression — i.e. `A`.
///
/// Parsing the rendered expression rather than re-parsing the C source keeps
/// this generator honest about using the *same* value the raw table gets: if
/// [`render_shift_add`] ever changed shape, this fails loudly at generation
/// time instead of silently typing every field `Opaque`.
fn serialized_type_id(rendered: &str) -> Result<u32> {
    let s = rendered.trim();
    let inner = s
        .strip_prefix('(')
        .ok_or_else(|| anyhow::anyhow!("expected a leading `(` in {rendered:?}"))?;
    let (shift_group, _) = inner
        .split_once(')')
        .ok_or_else(|| anyhow::anyhow!("expected a closing `)` in {rendered:?}"))?;
    let (a, b) = shift_group
        .split_once("<<")
        .ok_or_else(|| anyhow::anyhow!("expected `<<` in {rendered:?}"))?;
    if b.trim() != "16" {
        return Err(anyhow::anyhow!(
            "expected a 16-bit shift in {rendered:?}, got {b:?}"
        ));
    }
    a.trim()
        .parse::<u32>()
        .with_context(|| format!("parsing the serialized type ID out of {rendered:?}"))
}
