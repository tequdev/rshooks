//! Shared parsing/evaluation helpers for rshooks-core's vendored-header parity
//! tests (`docs/DESIGN.md` §4). Pulled in via `mod common;`; holds no
//! `#[test]`s of its own.
//!
//! These headers use a small, fixed vocabulary of numeric-literal and
//! declaration shapes, so hand-rolled string scanning is simpler and more
//! auditable here than a C parser or a `regex` dependency.
//!
//! Test code is exempt from the workspace's panic-freedom lints (per
//! `docs/DESIGN.md` §8); this module also allows
//! `clippy::arithmetic_side_effects` — byte-offset arithmetic is the normal
//! shape of a hand-rolled parser, not a panic risk on these tiny inputs.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;

// ---------------------------------------------------------------------
// Expression evaluator
// ---------------------------------------------------------------------

/// A tiny expression evaluator covering the numeric-literal shapes used in
/// the vendored Hook API headers and their Rust translation: decimal and hex
/// integer literals (`U`/`UL`/`ULL` suffixes on the C side, `_`-separated
/// digit groups on the Rust side), parenthesized groups, unary `-`, and
/// binary `<<` / `+`, plus bare identifier (or `mod::path` on the Rust side)
/// references resolved against `env` — used by `tx_flags.h`'s
/// `tfMPTCanLock = lsfMPTCanLock` style aliases and their
/// `ls_flags::lsfMPTCanLock` Rust equivalents.
pub fn eval_expr(expr: &str, env: &BTreeMap<String, i64>) -> i64 {
    let mut p = ExprParser {
        s: expr.as_bytes(),
        pos: 0,
        env,
    };
    let v = p.parse_add();
    p.skip_ws();
    assert!(
        p.pos == p.s.len(),
        "trailing input in expression {expr:?} at byte {}",
        p.pos
    );
    v
}

struct ExprParser<'a> {
    s: &'a [u8],
    pos: usize,
    env: &'a BTreeMap<String, i64>,
}

impl ExprParser<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.s.len() && (self.s[self.pos] as char).is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.s.get(self.pos).copied()
    }

    /// additive: shift (('+' | '-') shift)*
    fn parse_add(&mut self) -> i64 {
        let mut v = self.parse_shift();
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    v += self.parse_shift();
                }
                Some(b'-') => {
                    self.pos += 1;
                    v -= self.parse_shift();
                }
                _ => break,
            }
        }
        v
    }

    /// shift: unary ('<<' unary)*
    fn parse_shift(&mut self) -> i64 {
        let mut v = self.parse_unary();
        loop {
            self.skip_ws();
            if self.pos + 1 < self.s.len() && &self.s[self.pos..self.pos + 2] == b"<<" {
                self.pos += 2;
                v <<= self.parse_unary();
            } else {
                break;
            }
        }
        v
    }

    fn parse_unary(&mut self) -> i64 {
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                -self.parse_unary()
            }
            Some(b'+') => {
                self.pos += 1;
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> i64 {
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let v = self.parse_add();
                self.skip_ws();
                assert_eq!(self.peek(), Some(b')'), "expected ')' in expression");
                self.pos += 1;
                v
            }
            Some(c) if c.is_ascii_digit() => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => self.parse_ident(),
            other => panic!("unexpected byte {other:?} in expression"),
        }
    }

    fn parse_number(&mut self) -> i64 {
        let is_hex = self.s[self.pos..].starts_with(b"0x") || self.s[self.pos..].starts_with(b"0X");
        if is_hex {
            self.pos += 2;
        }
        let start = self.pos;
        let radix = if is_hex { 16 } else { 10 };
        let mut digits = String::new();
        while self.pos < self.s.len() {
            let c = self.s[self.pos] as char;
            if c == '_' {
                self.pos += 1;
                continue;
            }
            if c.is_digit(radix) {
                digits.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        assert!(
            self.pos > start || !digits.is_empty(),
            "empty numeric literal"
        );
        let v = i64::from_str_radix(&digits, radix)
            .unwrap_or_else(|e| panic!("bad numeric literal {digits:?}: {e}"));
        // C-style U/UL/ULL suffixes (Rust literals never have these).
        while self.pos < self.s.len() && matches!(self.s[self.pos], b'U' | b'u' | b'L' | b'l') {
            self.pos += 1;
        }
        v
    }

    fn parse_ident(&mut self) -> i64 {
        let start = self.pos;
        loop {
            while self.pos < self.s.len()
                && ((self.s[self.pos] as char).is_alphanumeric() || self.s[self.pos] == b'_')
            {
                self.pos += 1;
            }
            // Rust path separator: `ls_flags::lsfMPTCanLock`.
            if self.pos + 1 < self.s.len() && &self.s[self.pos..self.pos + 2] == b"::" {
                self.pos += 2;
                continue;
            }
            break;
        }
        let full = std::str::from_utf8(&self.s[start..self.pos]).unwrap();
        let key = full.rsplit("::").next().unwrap_or(full);
        *self
            .env
            .get(key)
            .unwrap_or_else(|| panic!("unresolved identifier {full:?} (looked up as {key:?})"))
    }
}

// ---------------------------------------------------------------------
// C-side extraction
// ---------------------------------------------------------------------

/// Extracts simple, single-value `#define NAME EXPR` object-like macros from
/// C source, in file order. Function-like macros (`#define NAME(args) ...`)
/// and bare macros with no value (e.g. include guards, `#define FOO_INCLUDED`
/// with nothing after the name) are skipped.
pub fn extract_c_defines(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("#define") else {
            continue;
        };
        let rest = rest.trim_start();
        let name_end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        if name.is_empty() {
            continue;
        }
        // Function-like macro: '(' immediately follows the name, no space.
        if rest[name_end..].starts_with('(') {
            continue;
        }
        let expr = rest[name_end..].trim().trim_end_matches('\\').trim();
        if expr.is_empty() {
            continue;
        }
        out.push((name.to_string(), expr.to_string()));
    }
    out
}

/// Extracts `enum NAME { A = expr, B = expr, ... };` (optionally
/// `enum NAME : underlying_type { ... };`) member lists from C source, in
/// file order, as `(member_name, expr)` pairs. Every member in the two
/// headers this is used for (`ls_flags.h`, `tx_flags.h`) carries an explicit
/// `= expr`, so implicit enumerator increment is intentionally not
/// implemented — a member without `=` panics rather than silently guessing.
pub fn extract_c_enum_members(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_enum = false;
    let mut buf = String::new();
    for line in src.lines() {
        let trimmed = line.trim();
        if !in_enum {
            if trimmed.starts_with("enum ") {
                in_enum = true;
                buf.clear();
                if let Some(idx) = line.find('{') {
                    buf.push_str(&line[idx + 1..]);
                    buf.push('\n');
                }
            }
            continue;
        }
        if let Some(idx) = line.find('}') {
            buf.push_str(&line[..idx]);
            in_enum = false;
            for member in buf.split(',') {
                let member = member.trim();
                if member.is_empty() {
                    continue;
                }
                let (name, expr) = member
                    .split_once('=')
                    .unwrap_or_else(|| panic!("enum member without explicit value: {member:?}"));
                out.push((name.trim().to_string(), expr.trim().to_string()));
            }
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    out
}

/// A single C function prototype from `extern.h`: name, mapped return type,
/// and ordered `(mapped type, name)` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CProto {
    pub name: String,
    pub ret_ty: String,
    pub params: Vec<(String, String)>,
}

/// Extracts every `extern <ret> <name>(<params>);` prototype from `extern.h`,
/// in file order. Tolerant of the header's line-wrapping (return type and
/// name/params on separate lines, multi-line param lists) and the one
/// `__attribute__((noduplicate))` annotation on `_g`.
pub fn extract_c_prototypes(src: &str) -> Vec<CProto> {
    let mut cleaned = String::new();
    for line in src.lines() {
        let l = line.trim();
        if l.is_empty()
            || l.starts_with("//")
            || l.starts_with('#')
            || l == "extern \"C\" {"
            || l == "}"
        {
            continue;
        }
        cleaned.push_str(l);
        cleaned.push(' ');
    }
    let cleaned = cleaned.replace("__attribute__((noduplicate))", " ");

    let mut out = Vec::new();
    for stmt in cleaned.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        let rest = stmt
            .strip_prefix("extern")
            .unwrap_or_else(|| panic!("expected 'extern' prefix in {stmt:?}"))
            .trim_start();
        let paren_open = rest
            .find('(')
            .unwrap_or_else(|| panic!("no '(' in {stmt:?}"));
        let paren_close = rest
            .rfind(')')
            .unwrap_or_else(|| panic!("no ')' in {stmt:?}"));
        let before = &rest[..paren_open];
        let params_str = rest[paren_open + 1..paren_close].trim();

        let toks: Vec<&str> = before.split_whitespace().collect();
        let (ret_toks, name) = toks.split_at(toks.len() - 1);
        let name = name[0].to_string();
        let ret_ty = map_c_type(&ret_toks.join(" "));

        let mut params = Vec::new();
        if !params_str.is_empty() {
            for p in params_str.split(',') {
                let p = p.trim();
                let ptoks: Vec<&str> = p.split_whitespace().collect();
                let (pty_toks, pname) = ptoks.split_at(ptoks.len() - 1);
                params.push((map_c_type(&pty_toks.join(" ")), pname[0].to_string()));
            }
        }
        out.push(CProto {
            name,
            ret_ty,
            params,
        });
    }
    out
}

fn map_c_type(c: &str) -> String {
    match c {
        "uint32_t" => "u32".to_string(),
        "int32_t" => "i32".to_string(),
        "int64_t" => "i64".to_string(),
        other => panic!("unmapped C type {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Rust-side extraction
// ---------------------------------------------------------------------

/// Extracts `pub const NAME: TYPE = EXPR;` declarations from Rust source, in
/// file order, as `(name, type, expr)` triples. One declaration per line, as
/// used throughout `rshooks-core`'s translation.
pub fn extract_rust_consts(src: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for line in src.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("pub const ") else {
            continue;
        };
        let colon = rest.find(':').unwrap_or_else(|| panic!("no ':' in {l:?}"));
        let name = rest[..colon].trim().to_string();
        let rest2 = &rest[colon + 1..];
        let eq = rest2.find('=').unwrap_or_else(|| panic!("no '=' in {l:?}"));
        let ty = rest2[..eq].trim().to_string();
        let expr = rest2[eq + 1..]
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string();
        out.push((name, ty, expr));
    }
    out
}

/// A single Rust `pub fn` signature: name, ordered `(type, name)` params,
/// and return type (`"()"` if none given).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustFn {
    pub name: String,
    pub ret_ty: String,
    pub params: Vec<(String, String)>,
}

/// Isolates the body of the first `unsafe extern "C" { ... }` block in
/// `src` (the wasm Hook API import block in `api.rs`), excluding the
/// `host_stubs` module that follows it.
pub fn extract_wasm_extern_block(src: &str) -> &str {
    const START: &str = "unsafe extern \"C\" {";
    let start = src
        .find(START)
        .unwrap_or_else(|| panic!("no `{START}` block found"))
        + START.len();
    let rest = &src[start..];
    let end = rest
        .find("\n}\n")
        .unwrap_or_else(|| panic!("no closing brace for wasm extern block"));
    &rest[..end]
}

/// Extracts every `pub fn NAME(params) [-> RET];` signature from a block of
/// Rust source (typically the output of [`extract_wasm_extern_block`]), in
/// order. Doc comments (`///...`) and attributes (`#[...]`) are ignored.
pub fn extract_rust_fn_signatures(block: &str) -> Vec<RustFn> {
    let mut cleaned = String::new();
    for line in block.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("///") || l.starts_with("#[") {
            continue;
        }
        cleaned.push_str(l);
        cleaned.push(' ');
    }

    let mut out = Vec::new();
    for stmt in cleaned.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        let rest = stmt
            .strip_prefix("pub fn ")
            .unwrap_or_else(|| panic!("expected 'pub fn ' in {stmt:?}"));
        let paren_open = rest
            .find('(')
            .unwrap_or_else(|| panic!("no '(' in {stmt:?}"));
        let paren_close = rest
            .rfind(')')
            .unwrap_or_else(|| panic!("no ')' in {stmt:?}"));
        let name = rest[..paren_open].trim().to_string();
        let params_str = rest[paren_open + 1..paren_close].trim();
        let after = rest[paren_close + 1..].trim();
        let ret_ty = after
            .strip_prefix("->")
            .map(|r| r.trim().to_string())
            .unwrap_or_else(|| "()".to_string());

        let mut params = Vec::new();
        if !params_str.is_empty() {
            for p in params_str.split(',') {
                let p = p.trim();
                if p.is_empty() {
                    continue;
                }
                let (pname, pty) = p
                    .split_once(':')
                    .unwrap_or_else(|| panic!("bad param {p:?}"));
                params.push((pty.trim().to_string(), pname.trim().to_string()));
            }
        }
        out.push(RustFn {
            name,
            ret_ty,
            params,
        });
    }
    out
}

// ---------------------------------------------------------------------
// Comparison helpers
// ---------------------------------------------------------------------

/// Builds a name -> value environment by evaluating a sequence of
/// `(name, expr)` pairs in order (so later exprs can reference earlier
/// names).
pub fn build_env(defs: &[(String, String)]) -> BTreeMap<String, i64> {
    let mut env = BTreeMap::new();
    for (name, expr) in defs {
        let v = eval_expr(expr, &env);
        env.insert(name.clone(), v);
    }
    env
}

/// Asserts that two name -> value maps are exactly equal: same key set
/// (missing-on-either-side reported by name) and same values.
pub fn assert_maps_match(
    header_label: &str,
    header: &BTreeMap<String, i64>,
    rust_label: &str,
    rust: &BTreeMap<String, i64>,
) {
    let mut missing_in_rust: Vec<_> = header.keys().filter(|k| !rust.contains_key(*k)).collect();
    missing_in_rust.sort();
    let mut missing_in_header: Vec<_> = rust.keys().filter(|k| !header.contains_key(*k)).collect();
    missing_in_header.sort();
    assert!(
        missing_in_rust.is_empty() && missing_in_header.is_empty(),
        "{header_label} vs {rust_label} name-set mismatch — missing in {rust_label}: {missing_in_rust:?}; missing in {header_label}: {missing_in_header:?}"
    );

    let mut mismatches = Vec::new();
    for (name, hv) in header {
        let rv = rust[name];
        if *hv != rv {
            mismatches.push(format!("{name}: {header_label}={hv} != {rust_label}={rv}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{header_label} vs {rust_label} value mismatches: {mismatches:#?}"
    );
}
