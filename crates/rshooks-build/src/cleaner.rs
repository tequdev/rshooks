//! The hook-cleaner pass: `docs/DESIGN.md` §6.2.
//!
//! Turns cargo's raw wasm output into a SetHook-shaped module: drops custom
//! sections, restricts exports to `hook`/`cbak`, garbage-collects
//! unreachable functions and globals, trims trailing zero bytes off active
//! data segments, and re-encodes the module with a whole-module index remap
//! (one table per index space, applied everywhere that space is
//! referenced).

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result, bail};

use crate::Options;
use crate::encode;
use crate::ir::{self, IndexRemapper};

/// Cleans `wasm`, producing a module whose only exports are `hook` (and
/// `cbak`, if present in the input), with all reachable-from-those-exports
/// functions and globals kept (and everything else — custom sections, the
/// table, element segments, unreachable code — dropped), and every active
/// data segment trimmed to end at its last non-zero byte (or dropped
/// entirely, if all-zero) — see [`trim_trailing_zeros`]. Trimming is
/// skipped wholesale when segments overlap or use non-`i32.const` offsets
/// (see [`data_trim_is_safe`]).
///
/// Errors if the `hook` export is missing, or if `hook`/`cbak` do not have
/// the required `(i32) -> i64` signature.
pub fn clean(wasm: &[u8], _opts: &Options) -> Result<Vec<u8>> {
    let m = ir::parse(wasm)?;

    // --- Locate and validate the retained exports (GC roots). ---
    let hook_old_idx = m
        .exports
        .iter()
        .find(|e| e.name == "hook" && matches!(e.kind, wasmparser::ExternalKind::Func))
        .map(|e| e.index)
        .context("module is missing the required `hook` export")?;
    check_entry_signature(&m, hook_old_idx, "hook")?;

    let cbak_old_idx = m
        .exports
        .iter()
        .find(|e| e.name == "cbak" && matches!(e.kind, wasmparser::ExternalKind::Func))
        .map(|e| e.index);
    if let Some(idx) = cbak_old_idx {
        check_entry_signature(&m, idx, "cbak")?;
    }

    let mut roots = vec![hook_old_idx];
    if let Some(idx) = cbak_old_idx {
        roots.push(idx);
    }

    // --- Reachability: functions reachable via direct `call`, globals
    // touched by any reachable function. ---
    let mut reachable_funcs: BTreeSet<u32> = BTreeSet::new();
    let mut used_globals: BTreeSet<u32> = BTreeSet::new();
    let mut stack = roots.clone();
    while let Some(f) = stack.pop() {
        if !reachable_funcs.insert(f) {
            continue;
        }
        if let Some(body) = m.defined_body(f) {
            let refs = ir::scan_refs(body)?;
            for c in refs.calls {
                stack.push(c);
            }
            used_globals.extend(refs.globals);
        }
    }

    // Active data segments are always retained, so any global their offset
    // expression references is reachable too.
    for d in &m.datas {
        if let wasmparser::DataKind::Active { offset_expr, .. } = &d.kind
            && let Some(g) = ir::const_expr_global_ref(offset_expr)?
        {
            used_globals.insert(g);
        }
    }

    // Fixed point: a kept global's own initializer may reference another
    // (earlier) global.
    loop {
        let mut added = false;
        for (i, g) in m.globals.iter().enumerate() {
            let old_idx = m.num_imported_globals() + i as u32;
            if used_globals.contains(&old_idx)
                && let Some(r) = ir::const_expr_global_ref(&g.init_expr)?
                && used_globals.insert(r)
            {
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    // --- Build whole-module index remaps. Imported functions are eligible
    // for GC exactly like defined ones (their relative order is preserved);
    // imported tables/memories/globals are always kept as-is (they are
    // hard errors downstream regardless, so GC'ing them is not worth the
    // complexity). ---
    let total_funcs = m.total_funcs();
    let mut func_new_index: HashMap<u32, u32> = HashMap::new();
    {
        let mut next = 0u32;
        for old in 0..total_funcs {
            if reachable_funcs.contains(&old) {
                func_new_index.insert(old, next);
                next += 1;
            }
        }
    }
    let n_imp_globals = m.num_imported_globals();
    let total_globals = m.total_globals();
    let mut global_new_index: HashMap<u32, u32> = HashMap::new();
    {
        let mut next = 0u32;
        for old in 0..total_globals {
            let keep = old < n_imp_globals || used_globals.contains(&old);
            if keep {
                global_new_index.insert(old, next);
                next += 1;
            }
        }
    }
    let func_map = |old: u32| -> u32 { *func_new_index.get(&old).unwrap_or(&old) };
    let global_map = |old: u32| -> u32 { *global_new_index.get(&old).unwrap_or(&old) };

    if !m.datas.is_empty() && m.memories.is_empty() {
        bail!("module has data segments but no memory is defined");
    }

    // --- Emit the cleaned module. ---
    let mut module = wasm_encoder::Module::new();

    module.section(&encode::encode_type_section(&m.types)?);

    let imports_sec = encode::encode_import_section(&m.imports, |ordinal, type_idx| {
        if reachable_funcs.contains(&ordinal) {
            Ok(Some(wasm_encoder::EntityType::Function(type_idx)))
        } else {
            Ok(None)
        }
    })?;
    module.section(&imports_sec);

    let n_imp_funcs = m.num_imported_funcs();
    let mut funcs_sec = wasm_encoder::FunctionSection::new();
    let mut kept_defined: Vec<(u32, &wasmparser::FunctionBody)> = Vec::new();
    for (i, (&type_idx, body)) in m.defined_func_types.iter().zip(m.code.iter()).enumerate() {
        let old_idx = n_imp_funcs + i as u32;
        if reachable_funcs.contains(&old_idx) {
            funcs_sec.function(type_idx);
            kept_defined.push((old_idx, body));
        }
    }
    module.section(&funcs_sec);

    // Tables and element segments are always dropped (call_indirect is
    // banned, so a table can never be a legitimate reachability root).

    module.section(&encode::encode_memory_section(&m.memories));

    let mut globals_sec = wasm_encoder::GlobalSection::new();
    {
        let mut remapper = IndexRemapper::new(func_map, global_map);
        for (i, g) in m.globals.iter().enumerate() {
            let old_idx = n_imp_globals + i as u32;
            if global_new_index.contains_key(&old_idx) {
                let ty = ir::conv_globaltype(g.ty)?;
                let expr = ir::remap_const_expr(&g.init_expr, &mut remapper)?;
                globals_sec.global(ty, &expr);
            }
        }
    }
    module.section(&globals_sec);

    let mut exports_sec = wasm_encoder::ExportSection::new();
    exports_sec.export(
        "hook",
        wasm_encoder::ExportKind::Func,
        func_map(hook_old_idx),
    );
    if let Some(idx) = cbak_old_idx {
        exports_sec.export("cbak", wasm_encoder::ExportKind::Func, func_map(idx));
    }
    module.section(&exports_sec);

    // No start section (banned outright; validator will reject a stray one
    // in `check` input, but the cleaner never re-emits one it saw).

    // Section order is fixed by the wasm spec: code precedes data.
    let mut code_sec = wasm_encoder::CodeSection::new();
    {
        let mut remapper = IndexRemapper::new(func_map, global_map);
        for (_old_idx, body) in &kept_defined {
            if ir::body_needs_remap(body, &func_map, &global_map)? {
                let built = ir::rebuild_function_body(body, &mut remapper, |_, _, _| Ok(()))?;
                code_sec.function(&built);
            } else {
                code_sec.raw(body.as_bytes());
            }
        }
    }
    module.section(&code_sec);

    let mut data_sec = wasm_encoder::DataSection::new();
    {
        let trim_safe = data_trim_is_safe(&m.datas);
        let mut remapper = IndexRemapper::new(func_map, global_map);
        for d in &m.datas {
            match &d.kind {
                wasmparser::DataKind::Active {
                    memory_index,
                    offset_expr,
                } => {
                    // Trailing-zero trim (`docs/DESIGN.md` §6.2 step 3): wasm
                    // linear memory is zero-initialized by definition, so
                    // trailing zero bytes in an active data segment are pure
                    // dead weight (5000 drops/byte SetHook fee) — untouched
                    // memory reads as zero either way, and memory size comes
                    // from the memory section, not segment lengths. Only the
                    // payload shrinks from the tail; the offset expression is
                    // untouched.
                    //
                    // Guarded by `trim_safe`: active segments apply in order
                    // and may legally OVERLAP, in which case a trailing zero
                    // can be a deliberate overwrite of an earlier segment's
                    // non-zero byte — trimming it would change memory
                    // contents. LLVM/wasm-ld never emit overlapping segments,
                    // but `clean` accepts arbitrary wasm, so trimming is
                    // skipped unless every offset is a plain `i32.const` and
                    // no two segment ranges intersect.
                    if trim_safe {
                        if let Some(trimmed) = trim_trailing_zeros(d.data) {
                            let expr = ir::remap_const_expr(offset_expr, &mut remapper)?;
                            data_sec.active(*memory_index, &expr, trimmed.iter().copied());
                        }
                    } else {
                        let expr = ir::remap_const_expr(offset_expr, &mut remapper)?;
                        data_sec.active(*memory_index, &expr, d.data.iter().copied());
                    }
                }
                wasmparser::DataKind::Passive => {
                    // Never produced by cargo's wasm output for this target,
                    // and a hard error downstream (the validator rejects any
                    // passive segment outright) — passed through unchanged
                    // rather than trimmed, since the trim above is only
                    // justified for active segments (memory contents, not an
                    // explicit `memory.init` payload whose exact length a
                    // reachable instruction could depend on).
                    data_sec.passive(d.data.iter().copied());
                }
            }
        }
    }
    module.section(&data_sec);

    Ok(module.finish())
}

/// Reads an active-segment offset expression, returning its value only when
/// it is exactly `i32.const <n>; end` (anything else — e.g. `global.get` —
/// disqualifies trimming, conservatively).
fn const_expr_offset(expr: &wasmparser::ConstExpr) -> Option<u64> {
    let mut r = expr.get_operators_reader();
    let wasmparser::Operator::I32Const { value } = r.read().ok()? else {
        return None;
    };
    matches!(r.read().ok()?, wasmparser::Operator::End).then_some(value as u32 as u64)
}

/// Whether the trailing-zero trim may be applied: every active segment must
/// have a plain `i32.const` offset, and no two segments' `[offset,
/// offset+len)` ranges may intersect. Overlapping segments apply in
/// declaration order, so a later segment's trailing zeros can be a
/// deliberate overwrite of earlier non-zero bytes — trimming would then
/// change memory contents.
fn data_trim_is_safe(datas: &[wasmparser::Data<'_>]) -> bool {
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for d in datas {
        match &d.kind {
            wasmparser::DataKind::Active { offset_expr, .. } => {
                let Some(start) = const_expr_offset(offset_expr) else {
                    return false;
                };
                ranges.push((start, start.saturating_add(d.data.len() as u64)));
            }
            wasmparser::DataKind::Passive => {}
        }
    }
    ranges.sort_unstable();
    ranges.windows(2).all(|w| match w {
        [(_, end_a), (start_b, _)] => end_a <= start_b,
        _ => true,
    })
}

/// Returns the shortest prefix of `data` that still ends in a non-zero
/// byte, or `None` if `data` is empty or entirely zero (in which case the
/// whole segment should be dropped).
fn trim_trailing_zeros(data: &[u8]) -> Option<&[u8]> {
    let last_nonzero = data.iter().rposition(|&b| b != 0)?;
    data.get(..=last_nonzero)
}

/// Verifies that function `idx` has the required `hook`/`cbak` signature:
/// exactly `(i32) -> i64`.
fn check_entry_signature(m: &ir::ParsedModule, idx: u32, export_name: &str) -> Result<()> {
    let type_idx = m
        .func_type_index(idx)
        .with_context(|| format!("`{export_name}` export does not refer to a function"))?;
    let ty = m
        .types
        .get(type_idx as usize)
        .with_context(|| format!("`{export_name}` export has an invalid type index"))?;
    if ty.params() != [wasmparser::ValType::I32] || ty.results() != [wasmparser::ValType::I64] {
        bail!(
            "`{export_name}` must have signature `(i32) -> i64`, found `({:?}) -> {:?}`",
            ty.params(),
            ty.results()
        );
    }
    Ok(())
}
