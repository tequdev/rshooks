//! The SetHook validator: `docs/DESIGN.md` §6.4.
//!
//! Applies the full SetHook-derived hard-error and warning rule set to a
//! module's *final* bytes. Used both as the last stage of `build`/`clean`
//! (after cleaning and the optional guard pass) and as the entirety of
//! `check`, which runs it against arbitrary wasm (including C-built hooks).

use crate::guard::{find_g_index, guard_hint, scan_function_loops};
use crate::ir;
use crate::{ApiVersion, Options};

/// A validation failure, split by whether the native upstream guard checker
/// (`docs/DESIGN.md` §6.5) also evaluates the same rule.
///
/// [`crate::verify`] may let the native checker's acceptance downgrade
/// [`ValidationError::guard`] findings to warnings, because the native
/// checker independently re-derives them (guard shape, R1/R2, worst-case
/// nesting). It may never downgrade [`ValidationError::hard`] findings —
/// MVP validity, the export/import set, structural sections, float
/// opcodes, and similar — because the native checker does not evaluate
/// them at all; an `Ok` native verdict says nothing about them.
#[derive(Debug, Clone, Default)]
pub struct ValidationError {
    /// Findings the native guard checker does not evaluate.
    pub hard: Vec<String>,
    /// Guard/nesting findings the native guard checker also evaluates.
    pub guard: Vec<String>,
}

impl ValidationError {
    /// True if there is at least one finding and every finding is
    /// guard-class (safe for [`crate::verify`] to downgrade wholesale).
    pub fn is_guard_only(&self) -> bool {
        self.hard.is_empty() && !self.guard.is_empty()
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let joined: Vec<&str> = self
            .hard
            .iter()
            .chain(self.guard.iter())
            .map(String::as_str)
            .collect();
        write!(f, "{}", joined.join("\n"))
    }
}

impl std::error::Error for ValidationError {}

/// The maximum size, in bytes, of a SetHook-legal wasm binary.
pub const MAX_SIZE: usize = 65_535;

/// Sizes at or above this many bytes trigger a "getting close to the limit"
/// warning.
pub const SIZE_WARNING_THRESHOLD: usize = 56 * 1024;

/// The maximum `block`/`loop`/`if` nesting depth a SetHook-legal
/// api-version-0 module's function bodies may reach (`Guard.h`
/// `NESTING_LIMIT` under `GuardRuleDepth32`; see `docs/DESIGN.md` §6.2c).
pub const MAX_NESTING_DEPTH: u32 = 32;

/// Nesting depths at or above this level trigger an "approaching the limit"
/// warning (api-version 0 only).
pub const NESTING_DEPTH_WARNING_THRESHOLD: u32 = 28;

/// The result of a successful validation: any non-fatal warnings found.
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    /// Human-readable warning messages.
    pub warnings: Vec<String>,
    /// True if the module exceeded [`MAX_SIZE`] but was allowed through by
    /// `opts.allow_oversize` (only ever set outside of `check`).
    pub oversize_allowed: bool,
    /// The worst-case instruction counts reported by the vendored upstream
    /// guard checker (`docs/DESIGN.md` §6.5), when it ran and accepted the
    /// module. Only ever set for API version 0, by
    /// [`crate::verify`]/[`crate::run_pipeline`] — [`validate`] itself never
    /// populates this field.
    pub guard_verdict: Option<crate::GuardVerdict>,
    /// The maximum `block`/`loop`/`if` nesting depth reached by any defined
    /// function (0 if none). Computed for every api version so
    /// `build`/`check` can always print it; only api-version 0
    /// hard-errors/warns on it (`docs/DESIGN.md` §6.2c/§6.4).
    pub max_nesting_depth: u32,
}

/// Validates `wasm` against the full SetHook rule set. Returns `Ok` (with
/// any warnings) if it is SetHook-legal, or `Err` describing every hard
/// error found otherwise.
pub fn validate(wasm: &[u8], opts: &Options) -> Result<ValidationReport, ValidationError> {
    let mut errors: Vec<String> = Vec::new();
    let mut guard_errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut oversize_allowed = false;

    // --- Size. ---
    if wasm.len() > MAX_SIZE {
        if opts.allow_oversize {
            oversize_allowed = true;
            warnings.push(format!(
                "INVALID: module is {} bytes, exceeding the {MAX_SIZE}-byte SetHook limit \
                 (written anyway because --allow-oversize was given)",
                wasm.len()
            ));
        } else {
            errors.push(format!(
                "module is {} bytes, exceeding the {MAX_SIZE}-byte SetHook limit",
                wasm.len()
            ));
        }
    } else if wasm.len() >= SIZE_WARNING_THRESHOLD {
        warnings.push(format!(
            "module is {} bytes, approaching the {MAX_SIZE}-byte SetHook limit",
            wasm.len()
        ));
    }

    // --- Generic WASM validity, restricted to the MVP feature set: catches
    // float *types* (not just opcodes), any post-MVP encoding (bulk-memory,
    // sign-extension, reference-types, SIMD, multi-value, multi-memory,
    // tail-call, exceptions, GC, component-model, ...), and general
    // structural soundness (e.g. a `start` function with the wrong
    // signature). ---
    if let Err(e) = wasmparser::Validator::new_with_features(mvp_features()).validate_all(wasm) {
        errors.push(format!("wasm is not valid under the MVP feature set: {e}"));
    }

    // The rest of the rules need our own parse; if that itself fails there
    // is nothing more we can usefully check.
    let m = match ir::parse(wasm) {
        Ok(m) => m,
        Err(e) => {
            errors.push(format!("failed to parse module: {e}"));
            return Err(ValidationError {
                hard: errors,
                guard: guard_errors,
            });
        }
    };

    // --- Export set. ---
    let mut hook_idx = None;
    let mut cbak_idx = None;
    for e in &m.exports {
        match (e.name, e.kind) {
            ("hook", wasmparser::ExternalKind::Func) => hook_idx = Some(e.index),
            ("cbak", wasmparser::ExternalKind::Func) => cbak_idx = Some(e.index),
            _ => errors.push(format!(
                "unexpected export `{}` (only `hook` and `cbak` may be exported)",
                e.name
            )),
        }
    }
    match hook_idx {
        None => errors.push("missing required `hook` export".to_string()),
        Some(idx) => check_entry_signature(&m, idx, "hook", &mut errors),
    }
    if let Some(idx) = cbak_idx {
        check_entry_signature(&m, idx, "cbak", &mut errors);
    }

    // --- Imports: module must be `env`, name must be whitelisted, and the
    // signature must match exactly. No imported memories/tables/globals. ---
    for imp in &m.imports {
        if imp.module != "env" {
            errors.push(format!(
                "import `{}::{}` is not from module `env`",
                imp.module, imp.name
            ));
            continue;
        }
        match imp.ty {
            wasmparser::TypeRef::Func(type_idx) => match crate::whitelist::lookup(imp.name) {
                None => errors.push(format!(
                    "import `{}` is not a recognized Hook API function",
                    imp.name
                )),
                Some(entry) => {
                    if let Some(ty) = m.types.get(type_idx as usize) {
                        if !signature_matches(ty, entry) {
                            errors.push(format!(
                                "import `{}` has signature `({:?}) -> {:?}`, expected `({:?}) -> {:?}`",
                                imp.name,
                                ty.params(),
                                ty.results(),
                                entry.params,
                                entry.result
                            ));
                        }
                    } else {
                        errors.push(format!("import `{}` has an invalid type index", imp.name));
                    }
                }
            },
            wasmparser::TypeRef::Table(_) => {
                errors.push(format!(
                    "import `{}` is an imported table (not allowed)",
                    imp.name
                ));
            }
            wasmparser::TypeRef::Memory(_) => {
                errors.push(format!(
                    "import `{}` is an imported memory (not allowed)",
                    imp.name
                ));
            }
            wasmparser::TypeRef::Global(_) => {
                errors.push(format!(
                    "import `{}` is an imported global (not allowed)",
                    imp.name
                ));
            }
            wasmparser::TypeRef::Tag(_) => {
                errors.push(format!(
                    "import `{}` is a tag import (not allowed)",
                    imp.name
                ));
            }
            wasmparser::TypeRef::FuncExact(_) => {
                errors.push(format!(
                    "import `{}` is an exact function-reference import (not allowed)",
                    imp.name
                ));
            }
        }
    }

    // --- No start section. ---
    if m.start.is_some() {
        errors.push("module has a `start` section (not allowed)".to_string());
    }

    // --- Element segments: only the MVP active/function-index form is
    // tolerated (the cleaner drops the table and every element segment, so
    // any survivor is external input or beyond the cleaner's scope). ---
    for (i, el) in m.elements.iter().enumerate() {
        let ok = matches!(
            (&el.kind, &el.items),
            (
                wasmparser::ElementKind::Active {
                    table_index: None | Some(0),
                    ..
                },
                wasmparser::ElementItems::Functions(_)
            )
        );
        if !ok {
            errors.push(format!(
                "element segment {i} is not in the MVP active/function-index form (passive, expression, or multi-table segments are not allowed)"
            ));
        }
    }

    // --- No data-count section. ---
    if m.data_count.is_some() {
        errors.push("module has a data-count section (not allowed)".to_string());
    }

    // --- Passive data segments; memory count. ---
    let mut has_passive_data = false;
    for d in &m.datas {
        if matches!(d.kind, wasmparser::DataKind::Passive) {
            has_passive_data = true;
        }
    }
    if has_passive_data {
        errors.push("module has a passive data segment (not allowed)".to_string());
    }
    let total_memories = m.memories.len()
        + m.imports
            .iter()
            .filter(|i| matches!(i.ty, wasmparser::TypeRef::Memory(_)))
            .count();
    if total_memories > 1 {
        errors.push(format!(
            "module defines {total_memories} memories (at most one is allowed)"
        ));
    } else if total_memories == 0 && !m.datas.is_empty() {
        errors.push("module has data segments but no memory is defined".to_string());
    }

    // --- Float opcodes/types (belt-and-suspenders alongside the generic
    // MVP-feature validation above, with function-level detail). ---
    for (i, body) in m.code.iter().enumerate() {
        let func_idx = m.num_imported_funcs() + i as u32;
        if let Ok(locals) = body.get_locals_reader() {
            for l in locals {
                if let Ok((_, ty)) = l
                    && matches!(ty, wasmparser::ValType::F32 | wasmparser::ValType::F64)
                {
                    errors.push(format!(
                        "function {func_idx} declares a floating-point local"
                    ));
                }
            }
        }
        if let Ok(mut reader) = body.get_operators_reader() {
            while !reader.eof() {
                let Ok((op, offset)) = reader.read_with_offset() else {
                    break;
                };
                let dbg = format!("{op:?}");
                if dbg.contains("F32") || dbg.contains("F64") {
                    errors.push(format!(
                        "function {func_idx} uses a floating-point opcode at offset {offset}"
                    ));
                }
                if matches!(op, wasmparser::Operator::CallIndirect { .. }) {
                    errors.push(format!(
                        "function {func_idx} uses `call_indirect` at offset {offset} (not allowed)"
                    ));
                }
            }
        }
    }

    // --- Recursion: DFS over the direct-call graph must be acyclic. ---
    if let Some(cycle) = ir::find_call_cycle(&m) {
        errors.push(format!(
            "recursive call cycle detected among functions: {cycle:?} (recursion is not allowed)"
        ));
    }

    // --- Guards, R1, R2 (API version 0 only; `docs/DESIGN.md` §6.2b/§6.4). ---
    if opts.api_version == ApiVersion::V0 {
        let g_index = find_g_index(&m);

        // R1: every api-version-0 module must import `_g`, even without any
        // loop — the vendored upstream checker enforces this unconditionally.
        if g_index.is_none() {
            guard_errors.push(
                "module does not import `_g` (env::_g, type (i32,i32)->i32) — required for \
                 every api-version-0 module, even without loops (R1)"
                    .to_string(),
            );
        }

        // R2: every type-section entry must be the type of an import or the
        // `(i32) -> i64` entry-point type. A defined helper function with any
        // other signature (notably compiler_builtins memset/memcpy/bcmp,
        // `(i32,i32,i32) -> i32`) makes the whole module invalid to the
        // upstream checker; the flatten pass (§6.2b) enforces this for
        // api-version-0 modules built through `rshooks-build`.
        let entry_ty = (
            [wasmparser::ValType::I32].as_slice(),
            [wasmparser::ValType::I64].as_slice(),
        );
        let import_shapes: std::collections::HashSet<(
            &[wasmparser::ValType],
            &[wasmparser::ValType],
        )> = m
            .imports
            .iter()
            .filter_map(|imp| match imp.ty {
                wasmparser::TypeRef::Func(idx) => m.types.get(idx as usize),
                _ => None,
            })
            .map(|ty| (ty.params(), ty.results()))
            .collect();
        for (i, ty) in m.types.iter().enumerate() {
            let shape = (ty.params(), ty.results());
            if shape != entry_ty && !import_shapes.contains(&shape) {
                guard_errors.push(format!(
                    "type {i} (`({:?}) -> {:?}`) is neither an import's type nor the entry-point \
                     type `(i32) -> i64` (R2) — this is only reachable if a defined helper \
                     function was left un-inlined",
                    ty.params(),
                    ty.results()
                ));
            }
        }

        for (i, body) in m.code.iter().enumerate() {
            let func_idx = m.num_imported_funcs() + i as u32;
            match scan_function_loops(body, g_index) {
                Ok(sites) => {
                    for site in sites.iter().filter(|s| !s.guarded) {
                        let mut msg = format!(
                            "function {func_idx}, offset {}: `loop` is missing a guard (`i32.const; i32.const; call $_g`)",
                            site.offset
                        );
                        if let Some(hint) = guard_hint(site.guess) {
                            msg.push_str(" — ");
                            msg.push_str(hint);
                        }
                        guard_errors.push(msg);
                    }
                }
                Err(e) => errors.push(format!(
                    "function {func_idx}: failed to scan for guards: {e}"
                )),
            }
        }
    }

    // --- Nesting depth: computed for every defined function, for every api
    // version (so `build`/`check` can always print the module's overall
    // max), but only api-version 0 hard-errors/warns on it — `Guard.h`
    // `NESTING_LIMIT`/`GuardRuleDepth32` is guard-type only; see
    // `docs/DESIGN.md` §6.2c/§6.4. ---
    let mut max_overall_depth: u32 = 0;
    for (i, body) in m.code.iter().enumerate() {
        let func_idx = m.num_imported_funcs() + i as u32;
        match ir::max_nesting_depth(body) {
            Ok(depth) => {
                max_overall_depth = max_overall_depth.max(depth);
                if opts.api_version == ApiVersion::V0 {
                    if depth > MAX_NESTING_DEPTH {
                        guard_errors.push(format!(
                            "function {func_idx}: block/loop/if nesting depth is {depth}, \
                             exceeding the {MAX_NESTING_DEPTH}-level limit (`Guard.h` \
                             `NESTING_LIMIT` under `GuardRuleDepth32`)"
                        ));
                    } else if depth >= NESTING_DEPTH_WARNING_THRESHOLD {
                        warnings.push(format!(
                            "function {func_idx}: block/loop/if nesting depth is {depth}, \
                             approaching the {MAX_NESTING_DEPTH}-level limit"
                        ));
                    }
                }
            }
            Err(e) => errors.push(format!(
                "function {func_idx}: failed to compute nesting depth: {e}"
            )),
        }
    }

    // --- Warning: more than one mutable defined global (beyond the single
    // shadow-stack-pointer pattern). ---
    let mutable_defined_globals = m.globals.iter().filter(|g| g.ty.mutable).count();
    if mutable_defined_globals > 1 {
        warnings.push(format!(
            "module has {mutable_defined_globals} mutable globals (expected at most one, the shadow stack pointer)"
        ));
    }

    if !errors.is_empty() || !guard_errors.is_empty() {
        return Err(ValidationError {
            hard: errors,
            guard: guard_errors,
        });
    }

    Ok(ValidationReport {
        warnings,
        oversize_allowed,
        guard_verdict: None,
        max_nesting_depth: max_overall_depth,
    })
}

/// WASM features restricted to (approximately) the 1.0 MVP, plus explicit
/// floating-point disallowance. `mutable_global` is left enabled: internally
/// mutable globals have been part of the module encoding since the MVP
/// (only cross-module global mutability was the later "mutable globals"
/// proposal's concern).
pub(crate) fn mvp_features() -> wasmparser::WasmFeatures {
    wasmparser::WasmFeatures::MUTABLE_GLOBAL
}

fn check_entry_signature(
    m: &ir::ParsedModule,
    idx: u32,
    export_name: &str,
    errors: &mut Vec<String>,
) {
    let Some(type_idx) = m.func_type_index(idx) else {
        errors.push(format!(
            "`{export_name}` export does not refer to a function"
        ));
        return;
    };
    let Some(ty) = m.types.get(type_idx as usize) else {
        errors.push(format!("`{export_name}` export has an invalid type index"));
        return;
    };
    if ty.params() != [wasmparser::ValType::I32] || ty.results() != [wasmparser::ValType::I64] {
        errors.push(format!(
            "`{export_name}` must have signature `(i32) -> i64`, found `({:?}) -> {:?}`",
            ty.params(),
            ty.results()
        ));
    }
}

fn signature_matches(ty: &wasmparser::FuncType, entry: &crate::whitelist::ApiFn) -> bool {
    if ty.params().len() != entry.params.len() {
        return false;
    }
    for (a, b) in ty.params().iter().zip(entry.params.iter()) {
        if !valtype_eq(*a, *b) {
            return false;
        }
    }
    match ty.results() {
        [r] => valtype_eq(*r, entry.result),
        _ => false,
    }
}

fn valtype_eq(a: wasmparser::ValType, b: wasm_encoder::ValType) -> bool {
    matches!(
        (a, b),
        (wasmparser::ValType::I32, wasm_encoder::ValType::I32)
            | (wasmparser::ValType::I64, wasm_encoder::ValType::I64)
            | (wasmparser::ValType::F32, wasm_encoder::ValType::F32)
            | (wasmparser::ValType::F64, wasm_encoder::ValType::F64)
    )
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    fn wasm(src: &str) -> Vec<u8> {
        wat::parse_str(src).expect("fixture is valid wat")
    }

    fn opts_v0() -> Options {
        Options::default()
    }

    fn opts_v1() -> Options {
        Options {
            api_version: ApiVersion::V1,
            ..Options::default()
        }
    }

    // R1: `_g` import required for every V0 module.
    const NO_G_HOOK: &str = r#"
    (module
      (func $hook (param i32) (result i64) (i64.const 0))
      (export "hook" (func $hook)))
    "#;

    #[test]
    fn r1_v0_module_without_g_errors() {
        let err = validate(&wasm(NO_G_HOOK), &opts_v0()).unwrap_err();
        assert!(err.to_string().contains("R1"), "{err}");
    }

    #[test]
    fn r1_does_not_apply_under_v1() {
        validate(&wasm(NO_G_HOOK), &opts_v1()).expect("V1 has no R1 requirement");
    }

    // R2: every type must be an import's type or the entry-point type.
    const STRAY_TYPE_HOOK: &str = r#"
    (module
      (import "env" "_g" (func $g (param i32 i32) (result i32)))
      (func $unused_helper (param i32 i32 i32) (result i32) (i32.const 0))
      (func $hook (param i32) (result i64) (i64.const 0))
      (export "hook" (func $hook)))
    "#;

    #[test]
    fn r2_v0_stray_type_errors() {
        let err = validate(&wasm(STRAY_TYPE_HOOK), &opts_v0()).unwrap_err();
        assert!(err.to_string().contains("R2"), "{err}");
    }

    #[test]
    fn r2_does_not_apply_under_v1() {
        validate(&wasm(STRAY_TYPE_HOOK), &opts_v1()).expect("V1 has no R2 requirement");
    }

    // -- Nesting depth --------------------------------------------------------

    fn nested_hook_src(depth: u32) -> String {
        let open = "(block ".repeat(depth as usize);
        let close = ")".repeat(depth as usize);
        format!(
            r#"(module
              (import "env" "_g" (func $g (param i32 i32) (result i32)))
              (func $hook (param i32) (result i64)
                {open} {close}
                (i64.const 0))
              (export "hook" (func $hook)))"#
        )
    }

    #[test]
    fn nesting_depth_33_is_a_hard_error_under_v0() {
        let err = validate(&wasm(&nested_hook_src(33)), &opts_v0()).unwrap_err();
        assert!(err.to_string().contains("nesting depth"), "{err}");
    }

    #[test]
    fn nesting_depth_28_is_a_warning_under_v0() {
        let report =
            validate(&wasm(&nested_hook_src(28)), &opts_v0()).expect("28 is not a hard error");
        assert!(
            report.warnings.iter().any(|w| w.contains("approaching")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn nesting_depth_33_is_fine_under_v1_but_still_reported() {
        let report =
            validate(&wasm(&nested_hook_src(33)), &opts_v1()).expect("V1 has no depth limit");
        assert_eq!(report.max_nesting_depth, 33);
    }

    // -- Structural section rules ----------------------------------------------

    #[test]
    fn passive_data_segment_errors() {
        let src = r#"
        (module
          (import "env" "_g" (func $g (param i32 i32) (result i32)))
          (memory 1)
          (data $d "AB")
          (func $hook (param i32) (result i64) (i64.const 0))
          (export "hook" (func $hook)))
        "#;
        let err = validate(&wasm(src), &opts_v0()).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("passive"), "{err}");
    }

    #[test]
    fn data_count_section_errors() {
        // Build a module with an explicit data-count section (id 12) via
        // wasm_encoder: no wat-authored fixture in this crate uses
        // bulk-memory (which is what would emit one), so hand-assembling
        // the raw section directly is simplest.
        let mut module = wasm_encoder::Module::new();
        let mut types = wasm_encoder::TypeSection::new();
        types
            .ty()
            .function([wasm_encoder::ValType::I32], [wasm_encoder::ValType::I64]);
        types.ty().function(
            [wasm_encoder::ValType::I32, wasm_encoder::ValType::I32],
            [wasm_encoder::ValType::I32],
        );
        module.section(&types);
        let mut imports = wasm_encoder::ImportSection::new();
        imports.import("env", "_g", wasm_encoder::EntityType::Function(1));
        module.section(&imports);
        let mut funcs = wasm_encoder::FunctionSection::new();
        funcs.function(0);
        module.section(&funcs);
        let mut mems = wasm_encoder::MemorySection::new();
        mems.memory(wasm_encoder::MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&mems);
        let mut exports = wasm_encoder::ExportSection::new();
        exports.export("hook", wasm_encoder::ExportKind::Func, 1);
        module.section(&exports);
        // Data-count section (id 12), count = 0 data segments.
        module.section(&wasm_encoder::DataCountSection { count: 0 });
        let mut code = wasm_encoder::CodeSection::new();
        let mut f = wasm_encoder::Function::new([]);
        f.instruction(&wasm_encoder::Instruction::I64Const(0));
        f.instruction(&wasm_encoder::Instruction::End);
        code.function(&f);
        module.section(&code);
        module.section(&wasm_encoder::DataSection::new());
        let bytes = module.finish();

        let err = validate(&bytes, &opts_v0()).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("data-count"),
            "{err}"
        );
    }

    #[test]
    fn non_mvp_element_segment_errors() {
        // A passive element segment (non-MVP form: MVP only allows active
        // segments).
        let mut module = wasm_encoder::Module::new();
        let mut types = wasm_encoder::TypeSection::new();
        types
            .ty()
            .function([wasm_encoder::ValType::I32], [wasm_encoder::ValType::I64]);
        types.ty().function(
            [wasm_encoder::ValType::I32, wasm_encoder::ValType::I32],
            [wasm_encoder::ValType::I32],
        );
        module.section(&types);
        let mut imports = wasm_encoder::ImportSection::new();
        imports.import("env", "_g", wasm_encoder::EntityType::Function(1));
        module.section(&imports);
        let mut funcs = wasm_encoder::FunctionSection::new();
        funcs.function(0);
        module.section(&funcs);
        let mut exports = wasm_encoder::ExportSection::new();
        exports.export("hook", wasm_encoder::ExportKind::Func, 1);
        module.section(&exports);
        // Element sections follow export (id 9, per the spec's fixed section
        // order); `ir::parse`/`validate` don't require a table to exist even
        // for a passive segment — only the MVP-shape check under test does.
        let mut elems = wasm_encoder::ElementSection::new();
        elems.segment(wasm_encoder::ElementSegment {
            mode: wasm_encoder::ElementMode::Passive,
            elements: wasm_encoder::Elements::Functions(std::borrow::Cow::Borrowed(&[1])),
        });
        module.section(&elems);
        let mut code = wasm_encoder::CodeSection::new();
        let mut f = wasm_encoder::Function::new([]);
        f.instruction(&wasm_encoder::Instruction::I64Const(0));
        f.instruction(&wasm_encoder::Instruction::End);
        code.function(&f);
        module.section(&code);
        let bytes = module.finish();

        let err = validate(&bytes, &opts_v0()).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("element segment"),
            "{err}"
        );
    }

    #[test]
    fn more_than_one_mutable_defined_global_warns() {
        let src = r#"
        (module
          (import "env" "_g" (func $g (param i32 i32) (result i32)))
          (global $a (mut i32) (i32.const 0))
          (global $b (mut i32) (i32.const 0))
          (func $hook (param i32) (result i64)
            (drop (global.get $a))
            (drop (global.get $b))
            (i64.const 0))
          (export "hook" (func $hook)))
        "#;
        let report =
            validate(&wasm(src), &opts_v0()).expect("multiple mutable globals is a warning");
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("mutable globals")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn size_at_or_above_56kib_warns() {
        // Just under the 65,535-byte hard limit, at/above the 56 KiB
        // warning threshold: pad with a data segment (needs a memory).
        let pad = "x".repeat(56 * 1024);
        let src = format!(
            r#"(module
              (import "env" "_g" (func $g (param i32 i32) (result i32)))
              (memory 2)
              (data (i32.const 0) "{pad}")
              (func $hook (param i32) (result i64) (i64.const 0))
              (export "hook" (func $hook)))"#
        );
        let bytes = wasm(&src);
        assert!(
            bytes.len() < MAX_SIZE,
            "fixture must stay under the hard limit"
        );
        assert!(bytes.len() >= SIZE_WARNING_THRESHOLD);
        let report = validate(&bytes, &opts_v0()).expect("under the hard limit");
        assert!(
            report.warnings.iter().any(|w| w.contains("approaching")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn declared_float_local_errors_mentioning_floating_point_local() {
        let src = r#"
        (module
          (import "env" "_g" (func $g (param i32 i32) (result i32)))
          (func $hook (param i32) (result i64)
            (local $f f32)
            (i64.const 0))
          (export "hook" (func $hook)))
        "#;
        let err = validate(&wasm(src), &opts_v0()).unwrap_err();
        assert!(err.to_string().contains("floating-point local"), "{err}");
    }
}
