//! Generates `crates/rshooks-core/src/host.rs`: the `HookHost` trait and its
//! `Guest` implementor, from `extern.h`'s parsed [`FunctionSpec`]s
//! (`crates/xtask/src/ir.rs`, `hook_api.json`).

use anyhow::Result;

use super::with_generated_marker;
use crate::ir::FunctionSpec;
use crate::parse::c_type_to_rust;

const MODULE_DOC: &str = "\
//! `HookHost`: the raw Hook API as a trait, plus `Guest`, the trait's one
//! real implementor.
//!
//! Generated from `crates/rshooks-core/hook_api.json` (`crates/xtask/src/ir.rs`),
//! itself generated from `extern.h` (`docs/DESIGN.md` §4). Every method here
//! is a 1:1 forward to the same-named function in [`crate::api`] — same
//! name, same signature (raw `u32`/`i64` Hook API types), same safety
//! contract — so this trait adds an indirection point without adding any
//! logic: swap `Guest` for another `HookHost` implementor (e.g. a native
//! test host) and every Hook API call routes through it instead.
//!
//! `rshooks` and the examples do not use this trait today — the flat
//! free-function API in [`crate::api`] remains the public surface they call
//! (`docs/DESIGN.md` §4/§5).
//!
//! **Deprecated.** `HookHost`'s methods take raw `u32` pointers, so an
//! implementor cannot soundly read a caller's buffer on a 64-bit host; it
//! cannot serve as a native test seam. That role belongs to
//! `rshooks_core::backend::HostBackend` (present only on native builds with
//! the `testenv` feature), which uses semantic Rust types instead of FFI
//! signatures. `HookHost`/`Guest` are kept for this release as published
//! API with no known consumers, and will be removed in the next breaking
//! release.
";

const DEPRECATION_NOTE: &str = "implement rshooks_core::backend::HostBackend instead; HookHost will be removed in the next breaking release";

fn rust_signature(f: &FunctionSpec) -> Result<(String, String)> {
    let ret_ty = c_type_to_rust(&f.ret_c_type)?;
    let mut params = Vec::with_capacity(f.params.len());
    for p in &f.params {
        params.push(format!("{}: {}", p.name, c_type_to_rust(&p.c_type)?));
    }
    Ok((params.join(", "), ret_ty.to_string()))
}

fn arg_names(f: &FunctionSpec) -> String {
    f.params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// clippy's `too_many_arguments` lint fires once a function has more
/// parameters than its threshold; mirrors `codegen::api`'s own check so the
/// trait and its impl carry the same allow the free functions do.
fn needs_too_many_arguments_allow(f: &FunctionSpec) -> bool {
    f.params.len() >= 7
}

/// Renders `host.rs`'s full contents from `extern.h`'s parsed
/// [`FunctionSpec`]s.
pub fn generate(fns: &[FunctionSpec]) -> Result<String> {
    let mut trait_body = String::new();
    for f in fns {
        let (params, ret_ty) = rust_signature(f)?;
        trait_body.push_str(&format!("    /// C: `{}` (extern.h)\n", f.doc));
        if needs_too_many_arguments_allow(f) {
            trait_body.push_str("    #[allow(clippy::too_many_arguments)]\n");
        }
        trait_body.push_str(&format!(
            "    unsafe fn {}(&self, {params}) -> {ret_ty};\n\n",
            f.name
        ));
    }
    trait_body.truncate(trait_body.trim_end_matches('\n').len());
    trait_body.push('\n');

    let mut impl_body = String::new();
    for f in fns {
        let (params, ret_ty) = rust_signature(f)?;
        let args = arg_names(f);
        impl_body.push_str(&format!("    /// C: `{}` (extern.h)\n", f.doc));
        if needs_too_many_arguments_allow(f) {
            impl_body.push_str("    #[allow(clippy::too_many_arguments)]\n");
        }
        impl_body.push_str(&format!(
            "    unsafe fn {}(&self, {params}) -> {ret_ty} {{\n        unsafe {{ crate::api::{}({args}) }}\n    }}\n\n",
            f.name, f.name
        ));
    }
    impl_body.truncate(impl_body.trim_end_matches('\n').len());
    impl_body.push('\n');

    let mut out = with_generated_marker("extern.h (via hook_api.json)", MODULE_DOC);
    out.push('\n');
    out.push_str("/// Every Hook API host function (`_g` plus the 74 `extern.h` functions), as\n");
    out.push_str(
        "/// trait methods with the exact same names, parameter names/types, and return\n",
    );
    out.push_str("/// types as `crate::api`'s free functions and `extern.h` itself.\n");
    out.push_str(&format!("#[deprecated(note = \"{DEPRECATION_NOTE}\")]\n"));
    out.push_str("#[allow(clippy::missing_safety_doc)]\n");
    out.push_str("pub trait HookHost {\n");
    out.push_str(&trait_body);
    out.push_str("}\n");
    out.push('\n');
    out.push_str(
        "/// The real `HookHost` implementor: on `target_arch = \"wasm32\"` this forwards\n",
    );
    out.push_str("/// to the genuine `extern \"C\"` Hook API imports; on every other target it\n");
    out.push_str("/// forwards to the same deterministic, non-panicking `NOT_IMPLEMENTED` host\n");
    out.push_str(
        "/// stubs `crate::api` provides there. Either way, `Guest` is a zero-sized unit\n",
    );
    out.push_str("/// struct with no state of its own — it exists purely as `HookHost`'s\n");
    out.push_str("/// concrete implementor.\n");
    out.push_str(&format!("#[deprecated(note = \"{DEPRECATION_NOTE}\")]\n"));
    out.push_str("pub struct Guest;\n");
    out.push('\n');
    out.push_str("#[allow(clippy::missing_safety_doc)]\n");
    out.push_str("#[allow(deprecated)]\n");
    out.push_str("impl HookHost for Guest {\n");
    out.push_str(&impl_body);
    out.push_str("}\n");

    Ok(out)
}
