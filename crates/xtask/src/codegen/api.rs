//! Generates `crates/rshooks-core/src/api.rs` from `extern.h`'s
//! [`FunctionSpec`]s (`crates/xtask/src/ir.rs`).

use anyhow::Result;

use super::with_generated_marker;
use crate::ir::FunctionSpec;
use crate::parse::c_type_to_rust;

const MODULE_DOC: &str = "\
//! Raw Hook API function declarations.
//!
//! Upstream: `Xahau/xahaud`, branch `release`, `hook/extern.h`, vendored at
//! `crates/rshooks-core/vendor/xahaud-hook/extern.h`.
//!
//! This mirrors `extern.h` exactly, in header order: 75 functions total
//! (`_g` plus 74 Hook API functions), all imported from wasm import module
//! `env`. Parameter names and types are kept verbatim (`read_ptr`/`read_len`
//! style `u32`, `i64` returns) so C hook source and this file can be
//! compared line by line.
//!
//! On non-`wasm32` targets (host builds, so rshooks and its tests/docs can
//! compile and run) the same signatures are provided as deterministic stub
//! functions that return [`crate::error::NOT_IMPLEMENTED`] (the `_g` stub
//! returns `0`, i.e. \"guard check passed\"). None of the stubs panic.
";

fn rust_signature(f: &FunctionSpec) -> Result<(String, String)> {
    let ret_ty = c_type_to_rust(&f.ret_c_type)?;
    let mut params = Vec::with_capacity(f.params.len());
    for p in &f.params {
        params.push(format!("{}: {}", p.name, c_type_to_rust(&p.c_type)?));
    }
    Ok((params.join(", "), ret_ty.to_string()))
}

/// `clippy::too_many_arguments` applies to functions with seven or more
/// parameters, so generated declarations at that threshold receive an allow.
fn needs_too_many_arguments_allow(f: &FunctionSpec) -> bool {
    f.params.len() >= 7
}

/// Renders `api.rs`'s full contents from `extern.h`'s parsed [`FunctionSpec`]s.
pub fn generate(fns: &[FunctionSpec]) -> Result<String> {
    let mut wasm_block = String::new();
    for f in fns {
        let (params, ret_ty) = rust_signature(f)?;
        wasm_block.push_str(&format!("    /// C: `{}` (extern.h)\n", f.doc));
        if needs_too_many_arguments_allow(f) {
            wasm_block.push_str("    #[allow(clippy::too_many_arguments)]\n");
        }
        wasm_block.push_str(&format!("    pub fn {}({params}) -> {ret_ty};\n\n", f.name));
    }
    // Drop the trailing blank line before the block's closing brace.
    wasm_block.truncate(wasm_block.trim_end_matches('\n').len());
    wasm_block.push('\n');

    let mut stubs = String::new();
    for f in fns {
        let (params, ret_ty) = rust_signature(f)?;
        let body = if f.name == "_g" {
            "0"
        } else {
            "NOT_IMPLEMENTED"
        };
        stubs.push_str(&format!("    /// C: `{}` (extern.h) — host stub\n", f.doc));
        stubs.push_str(&format!(
            "    pub unsafe fn {}({params}) -> {ret_ty} {{\n        {body}\n    }}\n\n",
            f.name
        ));
    }
    stubs.truncate(stubs.trim_end_matches('\n').len());
    stubs.push('\n');

    let mut out = with_generated_marker("extern.h", MODULE_DOC);
    out.push('\n');
    out.push_str("// The Hook API import block, exactly as declared in `extern.h`.\n");
    out.push_str("#[cfg(target_arch = \"wasm32\")]\n");
    out.push_str("#[link(wasm_import_module = \"env\")]\n");
    out.push_str("unsafe extern \"C\" {\n");
    out.push_str(&wasm_block);
    out.push_str("}\n");
    out.push('\n');
    out.push_str("/// Non-wasm host stubs mirroring the `extern.h` signatures exactly, so\n");
    out.push_str("/// rshooks and its docs/tests compile and run on the host. Every stub is a\n");
    out.push_str("/// deterministic, non-panicking placeholder: it returns\n");
    out.push_str("/// [`crate::error::NOT_IMPLEMENTED`] (the `_g` stub returns `0`, meaning\n");
    out.push_str("/// \"guard check passed\").\n");
    out.push_str("#[cfg(not(target_arch = \"wasm32\"))]\n");
    out.push_str("#[allow(\n");
    out.push_str("    missing_docs,\n");
    out.push_str("    clippy::missing_safety_doc,\n");
    out.push_str("    unused_variables,\n");
    out.push_str("    clippy::too_many_arguments\n");
    out.push_str(")]\n");
    out.push_str("mod host_stubs {\n");
    out.push_str("    use crate::error::NOT_IMPLEMENTED;\n");
    out.push('\n');
    out.push_str(&stubs);
    out.push_str("}\n");
    out.push('\n');
    out.push_str("#[cfg(not(target_arch = \"wasm32\"))]\n");
    out.push_str("pub use host_stubs::*;\n");

    Ok(out)
}
