//! Parsing for the vendored xahaud protocol format definitions
//! (`crates/rshooks-core/vendor/xahaud-protocol/`): the three `.macro` files
//! declaring every serialized field, transaction format and ledger entry
//! format, plus the three `.cpp` files carrying the common-field lists and
//! the inner-object formats.
//!
//! Like [`crate::parse`] (which handles the `hook/*.h` headers) this is a
//! from-scratch parser written for `xtask`, deliberately independent of the
//! minimal parser the parity test in
//! `crates/rshooks-core/tests/protocol_formats_parity.rs` uses: that test is
//! this parser's correctness oracle, and shared code would hide a shared bug.
//!
//! # Why a tokenizer and not pattern matching
//!
//! The corpus mixes forms freely — compact `{{a},{b}}` and multiline
//! initializer lists, trailing commas, an empty field list `({})`, comments
//! *inside* field lists, a commented-out `//UNTYPED_SFIELD(...)` line, and a
//! `#ifndef` block in `ledger_entries.macro` that *defines*
//! `LEDGER_ENTRY_DUPLICATE`/`EXPAND` in terms of `LEDGER_ENTRY` before any
//! invocation appears. Everything here therefore runs on a comment- and
//! directive-stripped token stream with balanced-delimiter scanning.
//!
//! # Why unrecognized input is a hard error
//!
//! Nothing in this module skips what it does not understand. A `.macro` file
//! must consist *entirely* of recognized invocations, a field list entry must
//! match `{sfX, soeY[, extra...]}`, and an anchored `.cpp` region must parse
//! completely. An upstream format change can therefore only fail the build —
//! never silently drop a transaction type, a ledger entry, or a field from
//! the generated artifact.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};

use crate::parse::parse_c_int;

// ---------------------------------------------------------------------
// Parsed shapes
// ---------------------------------------------------------------------

/// One `TYPED_SFIELD`/`UNTYPED_SFIELD` declaration from `sfields.macro`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SFieldDecl {
    /// The field's name (`sfAccount`), verbatim.
    pub name: String,
    /// The serialized type token (`UINT32`, `ACCOUNT`, `VL`, …), verbatim.
    pub sti: String,
    /// The field code within that serialized type.
    pub field_code: u16,
    /// `true` for `TYPED_SFIELD`, `false` for `UNTYPED_SFIELD`.
    pub typed: bool,
    /// Any further macro arguments (`SField::sMD_Never`,
    /// `SField::notSigning`, …), verbatim and unparsed — upstream metadata
    /// this toolchain does not interpret but does not throw away either.
    pub extras: Vec<String>,
}

/// How a format declares a field may appear: the `soe*` token of a
/// `{sfX, soeY}` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// `soeREQUIRED`.
    Required,
    /// `soeOPTIONAL`.
    Optional,
    /// `soeDEFAULT` — may be omitted from the wire form. Upstream encodes
    /// only that, never a default *value*.
    Default,
}

impl Presence {
    /// Parses a `soe*` token, hard-erroring on any token upstream has not
    /// used before rather than guessing at its meaning.
    pub fn parse(token: &str) -> Result<Self> {
        Ok(match token {
            "soeREQUIRED" => Self::Required,
            "soeOPTIONAL" => Self::Optional,
            "soeDEFAULT" => Self::Default,
            other => bail!(
                "unknown presence token `{other}` (expected soeREQUIRED/soeOPTIONAL/soeDEFAULT)"
            ),
        })
    }
}

/// One `{sfX, soeY[, extra...]}` entry of a format's field list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldEntry {
    /// The referenced field's name (`sfAmount`).
    pub sfield: String,
    /// Its declared presence.
    pub presence: Presence,
    /// Any further tokens in the entry (`soeMPTSupported`), verbatim.
    pub extras: Vec<String>,
}

/// One `TRANSACTION(tag, value, name, fields)` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxDecl {
    /// The `tt*` tag (`ttPAYMENT`).
    pub tag: String,
    /// The numeric transaction type value.
    pub value: u16,
    /// The upstream type name (`Payment`).
    pub name: String,
    /// The type-specific field list, in declared order.
    pub fields: Vec<FieldEntry>,
}

/// One `LEDGER_ENTRY`/`LEDGER_ENTRY_DUPLICATE(tag, value, name, rpcName,
/// fields)` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntryDecl {
    /// The `lt*` tag (`ltRIPPLE_STATE`).
    pub tag: String,
    /// The numeric ledger entry type value. Upstream writes these as
    /// decimal, hex, *and* character literals (`'D'`, `'E'`, `'H'`).
    pub value: u16,
    /// The upstream type name (`RippleState`).
    pub name: String,
    /// The RPC name (`state`).
    pub rpc_name: String,
    /// `true` when declared via `LEDGER_ENTRY_DUPLICATE` — upstream's marker
    /// for a name that also exists as a transaction type.
    pub duplicate: bool,
    /// The type-specific field list, in declared order.
    pub fields: Vec<FieldEntry>,
}

/// One `add(sfX.jsonName, sfX.getCode(), {…})` call in
/// `InnerObjectFormats.cpp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InnerObjectDecl {
    /// The field the inner object is the value of (`sfEmitDetails`).
    pub sfield: String,
    /// The inner object's field list, in declared order.
    pub fields: Vec<FieldEntry>,
}

// ---------------------------------------------------------------------
// Serialized type IDs
// ---------------------------------------------------------------------

/// The numeric serialized type ID of a `.macro` `STI` token.
///
/// The four "pseudo" types (10001..10004) name whole serialized containers
/// rather than a field's value type; upstream's own `hook/sfcodes.h` omits
/// their four fields for that reason, which
/// [`crate::protocol_ir`]'s cross-validation gate accounts for.
///
/// An unknown token is a hard error: a new serialized type upstream must be
/// mapped deliberately, never defaulted.
pub fn sti_code(token: &str) -> Result<u32> {
    Ok(match token {
        "UINT16" => 1,
        "UINT32" => 2,
        "UINT64" => 3,
        "UINT128" => 4,
        "UINT256" => 5,
        "AMOUNT" => 6,
        "VL" => 7,
        "ACCOUNT" => 8,
        "NUMBER" => 9,
        "OBJECT" => 14,
        "ARRAY" => 15,
        "UINT8" => 16,
        "UINT160" => 17,
        "PATHSET" => 18,
        "VECTOR256" => 19,
        "UINT96" => 20,
        "UINT192" => 21,
        "UINT384" => 22,
        "UINT512" => 23,
        "ISSUE" => 24,
        "XCHAIN_BRIDGE" => 25,
        "CURRENCY" => 26,
        "TRANSACTION" => 10001,
        "LEDGERENTRY" => 10002,
        "VALIDATION" => 10003,
        "METADATA" => 10004,
        other => bail!(
            "unknown serialized type token `{other}` in sfields.macro; map it \
             explicitly in protocol_parse::sti_code"
        ),
    })
}

/// The lowest serialized type ID that names a whole serialized container
/// (`TRANSACTION`, `LEDGERENTRY`, `VALIDATION`, `METADATA`) rather than a
/// field value type.
pub const PSEUDO_STI_MIN: u32 = 10_000;

// ---------------------------------------------------------------------
// Lexical preprocessing
// ---------------------------------------------------------------------

/// Replaces every `//…` and `/*…*/` comment with equivalent whitespace,
/// preserving newlines so byte offsets keep mapping to the original line
/// numbers. Character and string literals are copied through untouched, so a
/// `'/'` or `"//"` inside one cannot start a comment.
///
/// An **unterminated** `/*` is a hard error, not a comment running to the
/// end of the file. Blanking the rest of the input would delete every
/// declaration after it and leave the parsers with nothing to complain
/// about — precisely the silent drop this module's "why unrecognized input
/// is a hard error" rule exists to prevent. An unterminated *literal* is
/// different and is still deferred: [`end_of_literal`] already rejects it
/// from inside [`match_delimiter`]/[`split_top_level`], where the parser can
/// say which declaration it was in.
pub fn strip_comments(src: &str) -> Result<String> {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(' ');
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let opened = i;
                out.push_str("  ");
                i += 2;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        out.push_str("  ");
                        i += 2;
                        closed = true;
                        break;
                    }
                    // One space per *byte*, so byte offsets — and with them
                    // the line numbers in error messages — survive the strip
                    // even across the non-ASCII bytes in a license header.
                    out.push(if bytes[i] == b'\n' { '\n' } else { ' ' });
                    i += 1;
                }
                if !closed {
                    bail!(
                        "unterminated block comment opened on line {} — everything after it \
                         would be silently dropped",
                        line_of(src, opened)
                    );
                }
            }
            b'\'' | b'"' => {
                let end = match end_of_literal(bytes, i) {
                    Ok(end) => end,
                    // An unterminated literal is left to the parsers, which
                    // report it with far more context than this pass could.
                    Err(_) => bytes.len(),
                };
                out.push_str(src.get(i..end).unwrap_or_default());
                i = end;
            }
            _ => {
                let start = i;
                i += 1;
                while i < bytes.len() && !matches!(bytes[i], b'/' | b'\'' | b'"') {
                    i += 1;
                }
                out.push_str(src.get(start..i).unwrap_or_default());
            }
        }
    }
    Ok(out)
}

/// Blanks out every preprocessor directive (a line whose first non-whitespace
/// character is `#`, plus every line it continues onto with a trailing `\`),
/// preserving newlines.
///
/// Run *after* [`strip_comments`]. Two things depend on this:
///
/// - `ledger_entries.macro`'s `#ifndef LEDGER_ENTRY_DUPLICATE` block
///   `#define`s `LEDGER_ENTRY_DUPLICATE(...)` and `EXPAND(x)` in terms of
///   `LEDGER_ENTRY(__VA_ARGS__)` before any real invocation appears; without
///   this those definitions read as invocations.
/// - `TxFormats.cpp`/`LedgerFormats.cpp` define their `TRANSACTION` /
///   `LEDGER_ENTRY` macro over two lines, and the continuation line mentions
///   `commonFields` — the very anchor [`parse_common_fields`] keys on.
pub fn strip_directives(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut continuing = false;
    for (idx, line) in src.lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let is_directive = continuing || line.trim_start().starts_with('#');
        continuing = is_directive && line.trim_end().ends_with('\\');
        if is_directive {
            out.push_str(&" ".repeat(line.len()));
        } else {
            out.push_str(line);
        }
    }
    if src.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// [`strip_comments`] then [`strip_directives`], the input every parser in
/// this module works on. Fallible only because the first pass is.
pub fn preprocess(src: &str) -> Result<String> {
    Ok(strip_directives(&strip_comments(src)?))
}

// ---------------------------------------------------------------------
// Balanced-delimiter scanning
// ---------------------------------------------------------------------

/// Returns the byte offset just past the delimiter matching the opening
/// delimiter at `open` (`(`, `{` or `[`), skipping over nested pairs and
/// character/string literals.
fn match_delimiter(src: &str, open: usize) -> Result<usize> {
    let bytes = src.as_bytes();
    let opener = *bytes
        .get(open)
        .ok_or_else(|| anyhow!("offset {open} is past the end of the input"))?;
    let closer = match opener {
        b'(' => b')',
        b'{' => b'}',
        b'[' => b']',
        other => bail!("`{}` is not an opening delimiter", char::from(other)),
    };

    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\'' || b == b'"' {
            i = end_of_literal(bytes, i)?;
            continue;
        }
        if b == opener {
            depth += 1;
        } else if b == closer {
            depth -= 1;
            if depth == 0 {
                return Ok(i + 1);
            }
        }
        i += 1;
    }
    bail!(
        "unbalanced `{}` at byte {open} (line {})",
        char::from(opener),
        line_of(src, open)
    )
}

/// Returns the byte offset just past the literal starting at `start`.
fn end_of_literal(bytes: &[u8], start: usize) -> Result<usize> {
    let quote = *bytes
        .get(start)
        .ok_or_else(|| anyhow!("offset {start} is past the end of the input"))?;
    let mut i = start + 1;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        i += 1;
        if c == quote {
            return Ok(i);
        }
    }
    bail!("unterminated {} literal at byte {start}", char::from(quote))
}

/// The 1-based line number of byte offset `at`, for error messages.
fn line_of(src: &str, at: usize) -> usize {
    src.get(..at)
        .map_or(1, |head| head.bytes().filter(|b| *b == b'\n').count() + 1)
}

/// Splits `src` on top-level commas, ignoring commas nested inside `()`,
/// `{}`, `[]` or a literal. Each part is returned trimmed; a trailing comma
/// yields no trailing empty part.
fn split_top_level(src: &str) -> Result<Vec<&str>> {
    let bytes = src.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' => {
                i = end_of_literal(bytes, i)?;
                continue;
            }
            b'(' | b'{' | b'[' => {
                i = match_delimiter(src, i)?;
                continue;
            }
            b',' => {
                parts.push(src.get(start..i).unwrap_or_default().trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = src.get(start..).unwrap_or_default().trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    Ok(parts)
}

/// One `NAME(args)` invocation found in a preprocessed macro file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    name: String,
    /// The argument text between the outermost parentheses, verbatim.
    args: String,
    line: usize,
}

/// Reads a whole preprocessed `.macro` file as a sequence of `NAME(args)`
/// invocations, each optionally followed by a `;`.
///
/// Anything else left in the file after preprocessing — a stray token, an
/// identifier with no argument list — is a hard error naming its line. This
/// is what makes "an unrecognized construct fails the build" true of the
/// whole file rather than only of the constructs this module recognizes.
fn scan_invocations(src: &str) -> Result<Vec<Invocation>> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() || b == b';' {
            i += 1;
            continue;
        }
        if !(b.is_ascii_alphabetic() || b == b'_') {
            bail!(
                "unexpected `{}` at line {} — the file should contain only \
                 macro invocations",
                char::from(b),
                line_of(src, i)
            );
        }
        let name_end = src
            .get(i..)
            .unwrap_or_default()
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(bytes.len(), |rel| i + rel);
        let name = src.get(i..name_end).unwrap_or_default().to_string();

        let mut j = name_end;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if bytes.get(j) != Some(&b'(') {
            bail!(
                "identifier `{name}` at line {} is not a macro invocation \
                 (expected `(` after it)",
                line_of(src, i)
            );
        }
        let close = match_delimiter(src, j)?;
        out.push(Invocation {
            name,
            args: src.get(j + 1..close - 1).unwrap_or_default().to_string(),
            line: line_of(src, i),
        });
        i = close;
    }
    Ok(out)
}

/// Parses one `{sfX, soeY[, extra...]}` entry.
fn parse_field_entry(text: &str) -> Result<FieldEntry> {
    let inner = text
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| anyhow!("field list entry {text:?} is not a `{{...}}` group"))?;
    let parts = split_top_level(inner)?;
    let (sfield, presence, extras) = match parts.split_first() {
        Some((sfield, rest)) => {
            let (presence, extras) = rest
                .split_first()
                .ok_or_else(|| anyhow!("field list entry {text:?} has no presence token"))?;
            (*sfield, *presence, extras)
        }
        None => bail!("empty field list entry {text:?}"),
    };
    if !sfield.starts_with("sf") {
        bail!("field list entry {text:?} does not name an `sf*` field");
    }
    for extra in extras {
        if !extra.starts_with("soe") {
            bail!("unexpected token {extra:?} in field list entry {text:?}");
        }
    }
    Ok(FieldEntry {
        sfield: sfield.to_string(),
        presence: Presence::parse(presence)?,
        extras: extras.iter().map(|e| (*e).to_string()).collect(),
    })
}

/// Parses a brace-delimited field list — `{}` or `{ {sfX, soeY}, ... }`,
/// with or without a trailing comma — into its entries in declared order.
fn parse_field_list(text: &str) -> Result<Vec<FieldEntry>> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| anyhow!("field list {trimmed:?} is not a `{{...}}` group"))?;
    split_top_level(inner)?
        .into_iter()
        .map(parse_field_entry)
        .collect()
}

/// Parses a `.macro` field-list argument: the `({...})` wrapper upstream
/// writes around every `TRANSACTION`/`LEDGER_ENTRY` field list, including
/// the empty `({})` form (`ttDID_DELETE`).
fn parse_wrapped_field_list(text: &str) -> Result<Vec<FieldEntry>> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| anyhow!("field list argument {trimmed:?} is not wrapped in `(...)`"))?;
    parse_field_list(inner)
}

/// Parses a type value written as a decimal literal, a `0x` hex literal, or
/// a single-quoted character literal — all three of which
/// `ledger_entries.macro` uses (`'D'`, `'E'`, `'H'` on the `release`
/// branch). A character literal maps to its ASCII code.
pub fn parse_type_value(text: &str) -> Result<u16> {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix('\'') {
        let body = rest
            .strip_suffix('\'')
            .ok_or_else(|| anyhow!("unterminated character literal {trimmed:?}"))?;
        let mut chars = body.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii() => {
                return u8::try_from(u32::from(c))
                    .map(u16::from)
                    .map_err(|_| anyhow!("character literal {trimmed:?} is not ASCII"));
            }
            _ => bail!("unsupported character literal {trimmed:?} (expected one ASCII character)"),
        }
    }
    let (value, _) = parse_c_int(trimmed)?;
    u16::try_from(value).map_err(|_| anyhow!("type value {trimmed:?} does not fit in a u16"))
}

// ---------------------------------------------------------------------
// Per-file parsers
// ---------------------------------------------------------------------

/// Parses `sfields.macro` into every declared field, in file order.
pub fn parse_sfields(src: &str) -> Result<Vec<SFieldDecl>> {
    let src = preprocess(src)?;
    let mut out = Vec::new();
    for inv in scan_invocations(&src)? {
        let typed = match inv.name.as_str() {
            "TYPED_SFIELD" => true,
            "UNTYPED_SFIELD" => false,
            other => bail!(
                "unknown macro `{other}` at sfields.macro line {} (expected \
                 TYPED_SFIELD or UNTYPED_SFIELD)",
                inv.line
            ),
        };
        let args = split_top_level(&inv.args)?;
        let [name, sti, code, extras @ ..] = args.as_slice() else {
            bail!(
                "{} at sfields.macro line {} takes at least 3 arguments, found {}",
                inv.name,
                inv.line,
                args.len()
            );
        };
        let (field_code, _) = parse_c_int(code)?;
        let field_code = u16::try_from(field_code)
            .map_err(|_| anyhow!("field code {code:?} of `{name}` does not fit in a u16"))?;
        // Validate the token here so a new upstream serialized type fails at
        // its declaration, naming the field.
        sti_code(sti).map_err(|e| anyhow!("{name} (sfields.macro line {}): {e}", inv.line))?;
        out.push(SFieldDecl {
            name: (*name).to_string(),
            sti: (*sti).to_string(),
            field_code,
            typed,
            extras: extras.iter().map(|e| (*e).to_string()).collect(),
        });
    }
    Ok(out)
}

/// Parses `transactions.macro` into every `TRANSACTION` declaration, in file
/// order.
pub fn parse_transactions(src: &str) -> Result<Vec<TxDecl>> {
    let src = preprocess(src)?;
    let mut out = Vec::new();
    for inv in scan_invocations(&src)? {
        if inv.name != "TRANSACTION" {
            bail!(
                "unknown macro `{}` at transactions.macro line {} (expected TRANSACTION)",
                inv.name,
                inv.line
            );
        }
        let args = split_top_level(&inv.args)?;
        let [tag, value, name, fields] = args.as_slice() else {
            bail!(
                "TRANSACTION at transactions.macro line {} takes 4 arguments, found {}",
                inv.line,
                args.len()
            );
        };
        out.push(TxDecl {
            tag: (*tag).to_string(),
            value: parse_type_value(value)?,
            name: (*name).to_string(),
            fields: parse_wrapped_field_list(fields)
                .map_err(|e| anyhow!("in TRANSACTION({tag}) at line {}: {e}", inv.line))?,
        });
    }
    Ok(out)
}

/// Parses `ledger_entries.macro` into every `LEDGER_ENTRY` /
/// `LEDGER_ENTRY_DUPLICATE` declaration, in file order.
pub fn parse_ledger_entries(src: &str) -> Result<Vec<LedgerEntryDecl>> {
    let src = preprocess(src)?;
    let mut out = Vec::new();
    for inv in scan_invocations(&src)? {
        let duplicate = match inv.name.as_str() {
            "LEDGER_ENTRY" => false,
            "LEDGER_ENTRY_DUPLICATE" => true,
            other => bail!(
                "unknown macro `{other}` at ledger_entries.macro line {} (expected \
                 LEDGER_ENTRY or LEDGER_ENTRY_DUPLICATE)",
                inv.line
            ),
        };
        let args = split_top_level(&inv.args)?;
        let [tag, value, name, rpc_name, fields] = args.as_slice() else {
            bail!(
                "{} at ledger_entries.macro line {} takes 5 arguments, found {}",
                inv.name,
                inv.line,
                args.len()
            );
        };
        out.push(LedgerEntryDecl {
            tag: (*tag).to_string(),
            value: parse_type_value(value)?,
            name: (*name).to_string(),
            rpc_name: (*rpc_name).to_string(),
            duplicate,
            fields: parse_wrapped_field_list(fields)
                .map_err(|e| anyhow!("in {}({tag}) at line {}: {e}", inv.name, inv.line))?,
        });
    }
    Ok(out)
}

/// Extracts the `commonFields` initializer list from `TxFormats.cpp` or
/// `LedgerFormats.cpp`.
///
/// Narrow and anchored: only the balanced `{...}` following the single
/// `commonFields` anchor is read, everything outside it is ignored, and a
/// missing (or duplicated) anchor is a hard error — an upstream rename must
/// not silently yield an empty common-field list.
pub fn parse_common_fields(src: &str, what: &str) -> Result<Vec<FieldEntry>> {
    const ANCHOR: &str = "commonFields";
    let src = preprocess(src)?;
    let mut found: Option<usize> = None;
    let mut from = 0usize;
    while let Some(rel) = src.get(from..).unwrap_or_default().find(ANCHOR) {
        let at = from + rel;
        if found.is_some() {
            bail!("{what}: more than one `{ANCHOR}` anchor");
        }
        found = Some(at);
        from = at + ANCHOR.len();
    }
    let at = found.ok_or_else(|| anyhow!("{what}: no `{ANCHOR}` anchor found"))?;

    let rest = src
        .get(at + ANCHOR.len()..)
        .ok_or_else(|| anyhow!("{what}: truncated after `{ANCHOR}`"))?;
    let brace_rel = rest
        .find('{')
        .ok_or_else(|| anyhow!("{what}: no `{{` after the `{ANCHOR}` anchor"))?;
    let brace = at + ANCHOR.len() + brace_rel;
    if !rest
        .get(..brace_rel)
        .is_some_and(|gap| gap.trim().is_empty())
    {
        bail!("{what}: unexpected text between the `{ANCHOR}` anchor and its `{{`");
    }
    let close = match_delimiter(&src, brace)?;
    let list = src
        .get(brace..close)
        .ok_or_else(|| anyhow!("{what}: truncated `{ANCHOR}` initializer list"))?;
    parse_field_list(list).map_err(|e| anyhow!("{what}: {e}"))
}

/// Parses `InnerObjectFormats.cpp`'s `add(sfX.jsonName, sfX.getCode(), {…})`
/// calls, in file order.
///
/// Anchored on `add(`; the file mixes compact `{{a},{b}}` and multiline
/// initializer lists, both of which reduce to the same brace-delimited field
/// list. Anything inside a matched `add(` call that does not parse is a hard
/// error; text outside those calls (the constructor boilerplate, the two
/// accessor definitions below it) is ignored.
pub fn parse_inner_objects(src: &str) -> Result<Vec<InnerObjectDecl>> {
    let src = preprocess(src)?;
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = src.get(from..).unwrap_or_default().find("add") {
        let at = from + rel;
        from = at + "add".len();
        // Require an identifier boundary before `add`, so `.add`/`padd`
        // style occurrences cannot match.
        if at > 0
            && bytes
                .get(at - 1)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.')
        {
            continue;
        }
        let mut j = from;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if bytes.get(j) != Some(&b'(') {
            continue;
        }
        let close = match_delimiter(&src, j)?;
        let args_text = src.get(j + 1..close - 1).unwrap_or_default();
        let args = split_top_level(args_text)?;
        let [json_name, code, fields] = args.as_slice() else {
            bail!(
                "add(...) at InnerObjectFormats.cpp line {} takes 3 arguments, found {}",
                line_of(&src, at),
                args.len()
            );
        };
        let sfield = json_name
            .strip_suffix(".jsonName")
            .ok_or_else(|| {
                anyhow!(
                    "add(...) at InnerObjectFormats.cpp line {}: expected `sfX.jsonName`, found {json_name:?}",
                    line_of(&src, at)
                )
            })?
            .to_string();
        let code_sfield = code.strip_suffix(".getCode()").ok_or_else(|| {
            anyhow!(
                "add(...) at InnerObjectFormats.cpp line {}: expected `sfX.getCode()`, found {code:?}",
                line_of(&src, at)
            )
        })?;
        if code_sfield != sfield {
            bail!(
                "add(...) at InnerObjectFormats.cpp line {}: `{json_name}` and `{code}` name \
                 different fields",
                line_of(&src, at)
            );
        }
        out.push(InnerObjectDecl {
            sfield,
            fields: parse_field_list(fields).map_err(|e| {
                anyhow!(
                    "in add({json_name}) at InnerObjectFormats.cpp line {}: {e}",
                    line_of(&src, at)
                )
            })?,
        });
        from = close;
    }
    if out.is_empty() {
        bail!("InnerObjectFormats.cpp: no `add(...)` calls found");
    }
    Ok(out)
}

/// Indexes `sfields.macro` declarations by field name, hard-erroring on a
/// duplicate name.
pub fn index_sfields(decls: &[SFieldDecl]) -> Result<BTreeMap<&str, &SFieldDecl>> {
    let mut map = BTreeMap::new();
    for decl in decls {
        if map.insert(decl.name.as_str(), decl).is_some() {
            bail!("sfields.macro declares `{}` more than once", decl.name);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    //! Test code is exempt from the workspace's panic-freedom lints
    //! (`docs/DESIGN.md` §8): panicking on a known-good fixture is the
    //! normal, idiomatic way to assert behavior in a test.
    #![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn strips_comments_but_keeps_line_numbers_and_literals() {
        let src = "a // gone\nb /* also\ngone */ c\n'/'\n";
        let stripped = strip_comments(src).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(stripped.lines().count(), src.lines().count());
        assert!(!stripped.contains("gone"));
        assert!(stripped.contains("'/'"));
    }

    /// An unterminated `/*` blanks everything after it, so accepting one
    /// would delete declarations with nothing left to complain about — the
    /// silent drop this module's hard-error rule exists to prevent. The
    /// fixture is a real format file whose second declaration would vanish.
    #[test]
    fn an_unterminated_block_comment_is_an_error() {
        let src = "\
TYPED_SFIELD(sfAmount, AMOUNT, 1)
/* a comment nobody closed
TYPED_SFIELD(sfFlags, UINT32, 2)
";
        let msg = match strip_comments(src) {
            Ok(_) => panic!("expected an unterminated-block-comment failure"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            msg.contains("unterminated block comment") && msg.contains("line 2"),
            "{msg}"
        );

        // And the parsers inherit it rather than silently returning the one
        // declaration that survived the blanking.
        let msg = match parse_sfields(src) {
            Ok(decls) => panic!("expected a failure, parsed {} declarations", decls.len()),
            Err(e) => format!("{e:#}"),
        };
        assert!(msg.contains("unterminated block comment"), "{msg}");
    }

    #[test]
    fn commented_out_invocations_are_not_parsed() {
        let decls = parse_sfields(
            "//UNTYPED_SFIELD(sfSigningAccounts, ARRAY, 2)\nTYPED_SFIELD(sfMethod, UINT8, 2)\n",
        )
        .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].name, "sfMethod");
    }

    #[test]
    fn ledger_entry_duplicate_definition_is_not_an_invocation() {
        // The exact `#ifndef` block `ledger_entries.macro` opens with.
        let src = "\
#ifndef LEDGER_ENTRY_DUPLICATE
#define EXPAND(x) x
#define LEDGER_ENTRY_DUPLICATE(...) EXPAND(LEDGER_ENTRY(__VA_ARGS__))
#endif

LEDGER_ENTRY(ltTICKET, 0x0054, Ticket, ticket, ({
    {sfAccount, soeREQUIRED},
}))

#undef EXPAND
#undef LEDGER_ENTRY_DUPLICATE
";
        let entries = parse_ledger_entries(src).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tag, "ltTICKET");
        assert_eq!(entries[0].value, 0x0054);
    }

    #[test]
    fn parses_a_transaction_with_extras_and_a_trailing_comma() {
        let src = "\
/** This transaction type executes a payment. */
TRANSACTION(ttPAYMENT, 0, Payment, ({
    {sfDestination, soeREQUIRED},
    {sfAmount, soeREQUIRED, soeMPTSupported},
    {sfPaths, soeDEFAULT},
}))
";
        let txs = parse_transactions(src).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(txs.len(), 1);
        let tx = &txs[0];
        assert_eq!(tx.tag, "ttPAYMENT");
        assert_eq!(tx.value, 0);
        assert_eq!(tx.name, "Payment");
        assert_eq!(
            tx.fields,
            vec![
                FieldEntry {
                    sfield: "sfDestination".into(),
                    presence: Presence::Required,
                    extras: vec![],
                },
                FieldEntry {
                    sfield: "sfAmount".into(),
                    presence: Presence::Required,
                    extras: vec!["soeMPTSupported".into()],
                },
                FieldEntry {
                    sfield: "sfPaths".into(),
                    presence: Presence::Default,
                    extras: vec![],
                },
            ]
        );
    }

    #[test]
    fn parses_an_empty_field_list() {
        let txs = parse_transactions("TRANSACTION(ttDID_DELETE, 59, DIDDelete, ({}))\n")
            .unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(txs.len(), 1);
        assert!(txs[0].fields.is_empty());
    }

    #[test]
    fn parses_character_literal_ledger_entry_values() {
        let src = "LEDGER_ENTRY(ltHOOK, 'H', Hook, hook, ({\n    {sfOwner, soeREQUIRED},\n}))\n";
        let entries = parse_ledger_entries(src).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(entries[0].value, 0x48, "'H' is ASCII 0x48");
        assert_eq!(parse_type_value("'D'").unwrap_or(0), 0x44);
        assert_eq!(parse_type_value("0x0072").unwrap_or(0), 0x72);
        assert_eq!(parse_type_value("104").unwrap_or(0), 104);
    }

    #[test]
    fn parses_ledger_entry_duplicate_invocations() {
        let src = "\
LEDGER_ENTRY_DUPLICATE(ltDEPOSIT_PREAUTH, 0x0070, DepositPreauth, deposit_preauth, ({
    {sfAccount,   soeREQUIRED},
    {sfAuthorize, soeOPTIONAL},
}))
";
        let entries = parse_ledger_entries(src).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(entries.len(), 1);
        assert!(entries[0].duplicate);
        assert_eq!(entries[0].rpc_name, "deposit_preauth");
        assert_eq!(entries[0].fields.len(), 2);
    }

    #[test]
    fn parses_typed_sfields_with_three_four_and_five_arguments() {
        let src = "\
UNTYPED_SFIELD(sfLedgerEntry,   LEDGERENTRY, 257)
TYPED_SFIELD(sfLedgerEntryType, UINT16,      1, SField::sMD_Never)
TYPED_SFIELD(sfTxnSignature,    VL,          4, SField::sMD_Default, SField::notSigning)
";
        let decls = parse_sfields(src).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(decls.len(), 3);
        assert!(!decls[0].typed);
        assert_eq!(decls[0].sti, "LEDGERENTRY");
        assert_eq!(decls[0].field_code, 257);
        assert_eq!(decls[1].extras, vec!["SField::sMD_Never".to_string()]);
        assert_eq!(decls[2].extras.len(), 2);
    }

    #[test]
    fn parses_a_common_fields_block() {
        let src = "\
namespace ripple {
TxFormats::TxFormats()
{
    // Fields shared by all txFormats:
    static const std::initializer_list<SOElement> commonFields{
        {sfTransactionType, soeREQUIRED},
        {sfFlags, soeOPTIONAL},
        {sfAccount, soeREQUIRED},  // emulate027
    };
}
}
";
        let fields = parse_common_fields(src, "TxFormats.cpp").unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].sfield, "sfTransactionType");
        assert_eq!(fields[1].presence, Presence::Optional);
    }

    #[test]
    fn parses_compact_and_multiline_inner_object_formats() {
        let src = "\
InnerObjectFormats::InnerObjectFormats()
{
    add(sfEmitDetails.jsonName,
        sfEmitDetails.getCode(),
        {{sfEmitGeneration, soeREQUIRED},
         {sfEmitBurden, soeREQUIRED}});

    add(sfSigner.jsonName,
        sfSigner.getCode(),
        {
            {sfAccount, soeREQUIRED},
            {sfTxnSignature, soeREQUIRED},
        });
}
";
        let inners = parse_inner_objects(src).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(inners.len(), 2);
        assert_eq!(inners[0].sfield, "sfEmitDetails");
        assert_eq!(inners[0].fields.len(), 2);
        assert_eq!(inners[1].sfield, "sfSigner");
        assert_eq!(inners[1].fields[1].sfield, "sfTxnSignature");
    }

    // --- hard-error cases ------------------------------------------------

    fn err(result: Result<impl std::fmt::Debug>) -> String {
        match result {
            Ok(v) => panic!("expected an error, got {v:?}"),
            Err(e) => format!("{e:#}"),
        }
    }

    #[test]
    fn unknown_presence_token_is_an_error() {
        let msg = err(parse_transactions(
            "TRANSACTION(ttPAYMENT, 0, Payment, ({{sfAmount, soeMAYBE}}))\n",
        ));
        assert!(msg.contains("soeMAYBE"), "{msg}");
    }

    #[test]
    fn malformed_field_entry_is_an_error() {
        // A field entry with no presence token.
        let msg = err(parse_transactions(
            "TRANSACTION(ttPAYMENT, 0, Payment, ({{sfAmount}}))\n",
        ));
        assert!(msg.contains("presence"), "{msg}");

        // A bare token where a `{...}` entry belongs.
        let msg = err(parse_transactions(
            "TRANSACTION(ttPAYMENT, 0, Payment, ({sfAmount}))\n",
        ));
        assert!(msg.contains("not a `{...}` group"), "{msg}");

        // An entry naming something that is not an `sf*` field.
        let msg = err(parse_transactions(
            "TRANSACTION(ttPAYMENT, 0, Payment, ({{notAField, soeREQUIRED}}))\n",
        ));
        assert!(msg.contains("sf*"), "{msg}");
    }

    #[test]
    fn unknown_macro_invocation_is_an_error() {
        let msg = err(parse_transactions("PSEUDO_TRANSACTION(ttX, 1, X, ({}))\n"));
        assert!(msg.contains("PSEUDO_TRANSACTION"), "{msg}");
    }

    #[test]
    fn stray_text_in_a_macro_file_is_an_error() {
        let msg = err(parse_transactions(
            "TRANSACTION(ttPAYMENT, 0, Payment, ({}))\nstray_token\n",
        ));
        assert!(msg.contains("stray_token"), "{msg}");
    }

    #[test]
    fn missing_common_fields_anchor_is_an_error() {
        let msg = err(parse_common_fields(
            "TxFormats::TxFormats() { add(jss::name, tag, fields); }\n",
            "TxFormats.cpp",
        ));
        assert!(msg.contains("commonFields"), "{msg}");
    }

    #[test]
    fn unknown_serialized_type_token_is_an_error() {
        let msg = err(parse_sfields("TYPED_SFIELD(sfWhatever, UINT77, 1)\n"));
        assert!(
            msg.contains("UINT77") && msg.contains("sfWhatever"),
            "{msg}"
        );
    }

    #[test]
    fn wrong_argument_count_is_an_error() {
        let msg = err(parse_ledger_entries("LEDGER_ENTRY(ltX, 1, X, ({}))\n"));
        assert!(msg.contains("5 arguments"), "{msg}");
    }

    #[test]
    fn mismatched_inner_object_field_names_are_an_error() {
        let msg = err(parse_inner_objects(
            "add(sfSigner.jsonName, sfMajority.getCode(), {{sfAccount, soeREQUIRED}});\n",
        ));
        assert!(msg.contains("different fields"), "{msg}");
    }
}
