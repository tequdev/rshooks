//! Parsing for the vendored xahaud `hook/*.h` C headers.
//!
//! Deliberately independent of `crates/rshooks-core/tests/common/mod.rs`,
//! which parses the same headers for parity assertions at test time: that
//! module is this generator's correctness oracle, and shared code would hide
//! a shared bug from the parity tests. See
//! `crates/rshooks-core/vendor/xahaud-hook/VENDOR.md` and `docs/DESIGN.md`
//! §4 for the header set this parses.
//!
//! The header dialect handled here is narrow on purpose — object-like
//! `#define`s, brace-delimited C enums with explicit `= value` on every
//! member, and `extern <ret> <name>(<params>);` prototypes (optionally
//! wrapped across lines) — because that is the entire vocabulary the eight
//! vendored headers actually use.

use anyhow::{Context, Result, anyhow, bail};

/// One object-like `#define NAME VALUE` macro, in file order.
#[derive(Debug, Clone)]
pub struct Define {
    /// The macro name.
    pub name: String,
    /// The macro's literal replacement text (untouched C expression text).
    pub value: String,
}

/// Scans `src` line by line for object-like `#define` macros, skipping
/// function-like macros (`#define NAME(args) ...`) and value-less macros
/// (bare include guards such as `#define HOOK_ERROR_CODES`). Every macro
/// this generator consumes is a single physical line, so backslash-continued
/// macro bodies (used elsewhere in `macro.h` for multi-statement helper
/// macros) are read only up to their first line — enough to see the name
/// and, for function-like macros, the `(` that identifies and skips them.
pub fn scan_defines(src: &str) -> Vec<Define> {
    let mut out = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        let Some(after_directive) = trimmed.strip_prefix("#define") else {
            continue;
        };
        let after_directive = after_directive.trim_start();
        let name_len = after_directive
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after_directive.len());
        if name_len == 0 {
            continue;
        }
        let name = &after_directive[..name_len];
        let tail = &after_directive[name_len..];
        // Function-like: '(' immediately follows the macro name.
        if tail.starts_with('(') {
            continue;
        }
        let value = tail.trim().trim_end_matches('\\').trim();
        if value.is_empty() {
            continue;
        }
        out.push(Define {
            name: name.to_string(),
            value: value.to_string(),
        });
    }
    out
}

/// One member of a C enum: the member's name and its literal `= value`
/// expression text. Which enum it belongs to is [`EnumGroup::name`] on the
/// group it's stored in.
#[derive(Debug, Clone)]
pub struct EnumMember {
    /// The member's name.
    pub name: String,
    /// The member's literal `= value` expression text.
    pub value: String,
}

/// A contiguous group of enum members sharing one `enum NAME { ... };`.
#[derive(Debug, Clone)]
pub struct EnumGroup {
    /// The enum's name (its `NAME` in `enum NAME { ... }`).
    pub name: String,
    /// The enum's members, in declaration order.
    pub members: Vec<EnumMember>,
}

/// Extracts every `enum NAME { A = expr, B = expr, ... };` block from `src`,
/// in file order, as one [`EnumGroup`] per block. `ls_flags.h`/`tx_flags.h`
/// give every member an explicit value, so a member with none is a parse
/// error rather than a silently-guessed implicit increment.
pub fn scan_enum_groups(src: &str) -> Result<Vec<EnumGroup>> {
    let mut groups = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = src[cursor..].find("enum ") {
        let kw_start = cursor + rel;
        let at_line_start = kw_start == 0 || src.as_bytes()[kw_start - 1] == b'\n';
        if !at_line_start {
            cursor = kw_start + "enum ".len();
            continue;
        }

        let after_kw = &src[kw_start + "enum ".len()..];
        let brace = after_kw
            .find('{')
            .ok_or_else(|| anyhow!("`enum` with no opening brace"))?;
        // Header is `NAME` or `NAME : underlying_type`.
        let enum_name = after_kw[..brace]
            .split(':')
            .next()
            .ok_or_else(|| anyhow!("empty enum header"))?
            .trim()
            .to_string();

        let body_start = brace + 1;
        let close = after_kw[body_start..]
            .find('}')
            .ok_or_else(|| anyhow!("enum `{enum_name}` has no closing brace"))?;
        let body = &after_kw[body_start..body_start + close];

        let mut members = Vec::new();
        for member in body.split(',') {
            let member = member.trim();
            if member.is_empty() {
                continue;
            }
            let (name, value) = member
                .split_once('=')
                .ok_or_else(|| anyhow!("enum `{enum_name}` member without a value: {member:?}"))?;
            members.push(EnumMember {
                name: name.trim().to_string(),
                value: value.trim().to_string(),
            });
        }
        groups.push(EnumGroup {
            name: enum_name,
            members,
        });

        cursor = kw_start + "enum ".len() + body_start + close;
    }
    Ok(groups)
}

/// One `extern <ret> <name>(<params>);` prototype from `extern.h`: the C
/// return type token, the function name, and its ordered `(C type, name)`
/// parameters.
#[derive(Debug, Clone)]
pub struct ExternFn {
    /// The function's name.
    pub name: String,
    /// The C return type token (e.g. `int64_t`), unmapped.
    pub ret_c_ty: String,
    /// Ordered `(C type token, parameter name)` pairs.
    pub params: Vec<(String, String)>,
}

/// Extracts every prototype from `extern.h`, in file order. The header
/// wraps some declarations (return type on its own line; multi-line
/// parameter lists), so this first collapses the whole input to a
/// statement stream (comments, preprocessor lines, and the `extern "C" {`
/// wrapper dropped) and then splits that stream on `;`.
pub fn scan_extern_fns(src: &str) -> Result<Vec<ExternFn>> {
    let mut stream = String::new();
    for raw_line in src.lines() {
        let line = raw_line.trim();
        if line.is_empty()
            || line.starts_with("//")
            || line.starts_with('#')
            || line == "extern \"C\" {"
            || line == "}"
        {
            continue;
        }
        stream.push_str(line);
        stream.push(' ');
    }
    let stream = stream.replace("__attribute__((noduplicate))", " ");

    let mut fns = Vec::new();
    for stmt in stream.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        fns.push(parse_prototype(stmt)?);
    }
    Ok(fns)
}

fn parse_prototype(stmt: &str) -> Result<ExternFn> {
    let body = stmt
        .strip_prefix("extern")
        .ok_or_else(|| anyhow!("prototype missing `extern` prefix: {stmt:?}"))?
        .trim_start();
    let open = body
        .find('(')
        .ok_or_else(|| anyhow!("prototype missing '(': {stmt:?}"))?;
    let close = body
        .rfind(')')
        .ok_or_else(|| anyhow!("prototype missing ')': {stmt:?}"))?;

    let head_tokens: Vec<&str> = body[..open].split_whitespace().collect();
    let (name_token, ret_tokens) = head_tokens
        .split_last()
        .ok_or_else(|| anyhow!("prototype has no name: {stmt:?}"))?;
    let name = (*name_token).to_string();
    let ret_c_ty = ret_tokens.join(" ");

    let params_src = body[open + 1..close].trim();
    let mut params = Vec::new();
    if !params_src.is_empty() {
        for p in params_src.split(',') {
            let toks: Vec<&str> = p.split_whitespace().collect();
            let (pname, ty_toks) = toks
                .split_last()
                .ok_or_else(|| anyhow!("bad parameter {p:?} in {stmt:?}"))?;
            params.push((ty_toks.join(" "), (*pname).to_string()));
        }
    }
    Ok(ExternFn {
        name,
        ret_c_ty,
        params,
    })
}

/// Maps a C type token used in `extern.h` to its Rust FFI equivalent.
pub fn c_type_to_rust(c_ty: &str) -> Result<&'static str> {
    Ok(match c_ty {
        "uint32_t" => "u32",
        "int32_t" => "i32",
        "int64_t" => "i64",
        other => bail!("unmapped C type `{other}`"),
    })
}

/// Parses a single C integer literal (`123`, `-1`, `0x0040_0000`-style hex
/// without underscores, with an optional `U`/`UL`/`ULL` suffix) into its
/// value plus whether it was written in hex.
pub fn parse_c_int(text: &str) -> Result<(i64, bool)> {
    let text = text.trim();
    let (neg, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, text),
    };
    let (is_hex, digits_part) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (true, rest)
        } else {
            (false, text)
        };
    let end = digits_part
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(digits_part.len());
    let (digits, suffix) = digits_part.split_at(end);
    if !suffix.chars().all(|c| matches!(c, 'U' | 'u' | 'L' | 'l')) {
        bail!("unexpected trailing characters in integer literal {text:?}: {suffix:?}");
    }
    let radix = if is_hex { 16 } else { 10 };
    let magnitude = i64::from_str_radix(digits, radix)
        .with_context(|| format!("parsing integer literal {text:?}"))?;
    Ok((if neg { -magnitude } else { magnitude }, is_hex))
}
