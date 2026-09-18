//! Generates `crates/rshooks-build/src/whitelist.rs` from `extern.h`'s
//! parsed [`FunctionSpec`]s (`crates/xtask/src/ir.rs`) — the Hook API import
//! whitelist `rshooks-build`'s wasm validator checks every import against,
//! generated from the same parse as `rshooks-core`'s `src/api.rs` so the two
//! can never drift apart.

use std::fmt::Write as _;

use anyhow::{Context, Result, bail};

use super::with_generated_marker;
use crate::ir::FunctionSpec;
use crate::parse::c_type_to_rust;

const MODULE_DOC: &str = "\
//! The Hook API import whitelist.
//!
//! The single source of truth for which functions a Hook is allowed to
//! import from module `env`, and with which signature — generated from the
//! same `extern.h` parse as `rshooks-core`'s `src/api.rs`, so the two can
//! never drift apart.
";

/// `extern.h`'s C parameter/return types are only ever `uint32_t`,
/// `int32_t`, or `int64_t` ([`c_type_to_rust`]); this maps those straight to
/// the wasm value type every Hook API import uses.
fn val_type_token(c_ty: &str) -> Result<&'static str> {
    match c_type_to_rust(c_ty)? {
        "u32" | "i32" => Ok("I32"),
        "i64" => Ok("I64"),
        other => bail!("unmapped Hook API type `{other}` for the wasm whitelist"),
    }
}

/// Renders `whitelist.rs`'s full contents from `extern.h`'s parsed
/// [`FunctionSpec`]s.
pub fn generate(functions: &[FunctionSpec]) -> Result<String> {
    let mut entries = String::new();
    for f in functions {
        let mut params = Vec::with_capacity(f.params.len());
        for p in &f.params {
            params.push(val_type_token(&p.c_type)?);
        }
        let result = val_type_token(&f.ret_c_type)?;
        writeln!(
            entries,
            "ApiFn {{ name: \"{}\", params: &[{}], result: {result} }},",
            f.name,
            params.join(", ")
        )
        .context("writing whitelist entry")?;
    }

    let mut body = String::from(
        "\nuse wasm_encoder::ValType;\n\n\
        /// One entry in the Hook API whitelist: an import name, its parameter types,\n\
        /// and its single result type.\n\
        #[derive(Debug, Clone, Copy)]\n\
        pub struct ApiFn {\n\
        /// The import name (as it appears after `wasm_import_module = \"env\"`).\n\
        pub name: &'static str,\n\
        /// Parameter types, in order.\n\
        pub params: &'static [ValType],\n\
        /// The single result type (every Hook API function, including `_g`,\n\
        /// returns exactly one value).\n\
        pub result: ValType,\n\
        }\n\n\
        const I32: ValType = ValType::I32;\n\
        const I64: ValType = ValType::I64;\n\n\
        /// Every function a Hook may import from module `env`: `_g` plus every\n\
        /// other Hook API function from `extern.h`, in header order.\n\
        pub static WHITELIST: &[ApiFn] = &[\n",
    );
    body.push_str(&entries);
    body.push_str(
        "];\n\n\
        /// Looks up a whitelist entry by name.\n\
        #[must_use]\n\
        pub fn lookup(name: &str) -> Option<&'static ApiFn> {\n\
        WHITELIST.iter().find(|f| f.name == name)\n\
        }\n",
    );

    Ok(with_generated_marker("extern.h", MODULE_DOC) + &body)
}
