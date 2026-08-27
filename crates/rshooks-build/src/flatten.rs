//! Full inlining for Hook API version 0 modules.
//!
//! The pass inlines non-entry functions, ensures `_g` is imported, and
//! rebuilds the type section so only import and entry-point types remain.
//! Input must already have passed the cleaner.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use wasm_encoder::reencode::Reencode;

use crate::encode;
use crate::ir;

/// Report from a [`flatten`] run: which callee bodies were duplicated into
/// more than one call site (a size cost that is acceptable, per
/// `docs/DESIGN.md` §6.2b, but worth surfacing for transparency).
#[derive(Debug, Clone, Default)]
pub struct FlattenReport {
    /// Human-readable notes, e.g. "function 3 was inlined into 2 call sites
    /// (body duplicated)".
    pub notes: Vec<String>,
}

/// A defined function's state during flattening: its parameter/result
/// types (fixed, from the original type section) and its *current* body
/// and extra (non-parameter) locals, which are progressively rewritten in
/// place as callees are inlined into it. Indexed throughout by the
/// function's *original* (pre-flatten) function index — flatten never
/// renumbers functions until the very end, so callee lookups by original
/// `Call` target just work.
struct FlatFunc<'a> {
    param_types: Vec<wasmparser::ValType>,
    result_type: Option<wasmparser::ValType>,
    extra_locals: Vec<wasmparser::ValType>,
    body: Vec<wasmparser::Operator<'a>>,
}

/// Runs the flatten pass on already-cleaned `wasm`. Returns the flattened
/// bytes (with `_g` guaranteed present as an import, and a type section
/// containing exactly {import types} ∪ {entry type}) plus a report of any
/// callee bodies that were duplicated into multiple call sites.
///
/// Errors if the module has no `hook` export, or if the direct-call graph
/// among defined functions contains a cycle (recursion is a hard error,
/// checked here — before flattening even starts — because the inliner's
/// termination depends on the graph being a DAG).
pub fn flatten(wasm: &[u8]) -> Result<(Vec<u8>, FlattenReport)> {
    let m = ir::parse(wasm)?;

    let hook_old = m
        .exports
        .iter()
        .find(|e| e.name == "hook" && matches!(e.kind, wasmparser::ExternalKind::Func))
        .map(|e| e.index)
        .context("module is missing the required `hook` export")?;
    let cbak_old = m
        .exports
        .iter()
        .find(|e| e.name == "cbak" && matches!(e.kind, wasmparser::ExternalKind::Func))
        .map(|e| e.index);

    // Recursion is banned module-wide; the inliner's termination relies on
    // the call graph being a DAG, so this is checked *before* flattening
    // (docs/DESIGN.md §6.2b), not just left to the post-transform validator.
    if let Some(cycle) = ir::find_call_cycle(&m) {
        bail!(
            "recursive call cycle detected among functions: {cycle:?} (recursion is not \
             allowed; the flatten pass cannot terminate on a cyclic call graph)"
        );
    }

    let n_imp_funcs = m.num_imported_funcs();
    let total_funcs = m.total_funcs();

    // --- Build the initial per-function state, keyed by original index. ---
    let mut funcs: HashMap<u32, FlatFunc> = HashMap::new();
    for (i, (&type_idx, body)) in m.defined_func_types.iter().zip(m.code.iter()).enumerate() {
        let old_idx = n_imp_funcs + i as u32;
        let ty = m
            .types
            .get(type_idx as usize)
            .with_context(|| format!("function {old_idx} has an invalid type index"))?;
        let mut extra_locals = Vec::new();
        for l in body.get_locals_reader().context("function locals")? {
            let (count, ty) = l.context("function locals")?;
            for _ in 0..count {
                extra_locals.push(ty);
            }
        }
        let mut ops = Vec::new();
        let mut reader = body.get_operators_reader().context("function body")?;
        while !reader.eof() {
            let op = reader.read().context("function body operator")?;
            // Flatten rebuilds the type section down to {import types} ∪
            // {entry type} but copies function bodies through unchanged, so
            // a `BlockType::FuncType` blocktype (explicit type-index,
            // non-MVP multi-value/param blocks) would end up referencing a
            // type index that no longer exists, or means something else,
            // after this pass runs. wasm32v1-none/LLVM never emits these;
            // reject loudly here rather than let it surface later as a
            // confusing wasmparser-invalid-output error.
            if let wasmparser::Operator::Block { blockty }
            | wasmparser::Operator::Loop { blockty }
            | wasmparser::Operator::If { blockty } = &op
                && matches!(blockty, wasmparser::BlockType::FuncType(_))
            {
                bail!(
                    "function {old_idx}: explicit type-index blocktype (non-MVP \
                     multi-value/param block) is not supported by the flatten pass"
                );
            }
            ops.push(op);
        }
        funcs.insert(
            old_idx,
            FlatFunc {
                param_types: ty.params().to_vec(),
                result_type: ty.results().first().copied(),
                extra_locals,
                body: ops,
            },
        );
    }

    // --- Reverse topological order: callees before their callers, so that
    // by the time a function F is processed (its own call sites inlined),
    // every defined function F calls has *already* been processed and thus
    // contains no more calls to defined functions itself. Standard DFS
    // post-order over a DAG has exactly this property (for every edge
    // u -> v, v finishes before u), which is why the recursion check above
    // runs first. ---
    let post_order = topo_post_order(&m, n_imp_funcs, total_funcs);

    let mut dup_counts: HashMap<u32, u32> = HashMap::new();
    for f_idx in post_order {
        let Some(mut cur) = funcs.remove(&f_idx) else {
            continue;
        };
        let param_count = cur.param_types.len() as u32;
        let orig_body = std::mem::take(&mut cur.body);
        let mut new_body = Vec::with_capacity(orig_body.len());
        for op in orig_body {
            if let wasmparser::Operator::Call { function_index } = op
                && let Some(callee) = funcs.get(&function_index)
            {
                *dup_counts.entry(function_index).or_insert(0) += 1;

                // (a) Spill the arguments into fresh caller locals, in
                // reverse order (the last operand is popped first).
                let arg_base = param_count + cur.extra_locals.len() as u32;
                let arg_locals: Vec<u32> = (0..callee.param_types.len() as u32)
                    .map(|i| arg_base + i)
                    .collect();
                cur.extra_locals.extend(callee.param_types.iter().copied());
                let extra_base = param_count + cur.extra_locals.len() as u32;
                cur.extra_locals.extend(callee.extra_locals.iter().copied());

                emit_inlined(&mut new_body, callee, &arg_locals, extra_base);
                continue;
            }
            new_body.push(op);
        }
        cur.body = new_body;
        funcs.insert(f_idx, cur);
    }

    let mut report = FlattenReport::default();
    for (&idx, &count) in &dup_counts {
        if count > 1 {
            report.notes.push(format!(
                "function {idx} was inlined into {count} call sites (body duplicated {count}x)"
            ));
        }
    }

    // --- R1: ensure `_g` is imported. ---
    let existing_g = m.find_func_import("env", "_g");

    // --- R2: rebuild the type section to exactly {import types} ∪ {entry
    // type}, deduplicated. ---
    let mut new_types: Vec<wasmparser::FuncType> = Vec::new();
    let mut old_type_to_new: HashMap<u32, u32> = HashMap::new();
    for imp in &m.imports {
        if let wasmparser::TypeRef::Func(old_ty) = imp.ty
            && !old_type_to_new.contains_key(&old_ty)
        {
            let ft = m
                .types
                .get(old_ty as usize)
                .context("import has an invalid type index")?;
            let new_idx = ir::find_or_insert_type(&mut new_types, ft.params(), ft.results());
            old_type_to_new.insert(old_ty, new_idx);
        }
    }
    let entry_type_idx = ir::find_or_insert_type(
        &mut new_types,
        &[wasmparser::ValType::I32],
        &[wasmparser::ValType::I64],
    );
    let g_type_idx = if existing_g.is_none() {
        Some(ir::find_or_insert_type(
            &mut new_types,
            &[wasmparser::ValType::I32, wasmparser::ValType::I32],
            &[wasmparser::ValType::I32],
        ))
    } else {
        None
    };

    // --- Assemble the output module. ---
    let mut module = wasm_encoder::Module::new();

    module.section(&encode::encode_type_section(&new_types)?);

    let mut imports_sec = encode::encode_import_section(&m.imports, |_ordinal, old_ty| {
        let new_ty = *old_type_to_new
            .get(&old_ty)
            .context("internal error: import type was not registered")?;
        Ok(Some(wasm_encoder::EntityType::Function(new_ty)))
    })?;
    if let Some(g_type_idx) = g_type_idx {
        imports_sec.import("env", "_g", wasm_encoder::EntityType::Function(g_type_idx));
    }
    module.section(&imports_sec);

    let n_new_func_imports = n_imp_funcs + u32::from(existing_g.is_none());
    let hook_new_idx = n_new_func_imports;
    let cbak_new_idx = cbak_old.map(|_| n_new_func_imports + 1);

    let mut funcs_sec = wasm_encoder::FunctionSection::new();
    funcs_sec.function(entry_type_idx);
    if cbak_old.is_some() {
        funcs_sec.function(entry_type_idx);
    }
    module.section(&funcs_sec);

    // No table section: the cleaner already dropped the table and every
    // element segment (call_indirect is banned), and flatten never adds one.

    module.section(&encode::encode_memory_section(&m.memories));

    let mut remapper = ir::IndexRemapper::new(|x| x, |x| x);
    module.section(&encode::encode_global_section(&m.globals, &mut remapper)?);

    let mut exports_sec = wasm_encoder::ExportSection::new();
    exports_sec.export("hook", wasm_encoder::ExportKind::Func, hook_new_idx);
    if let Some(idx) = cbak_new_idx {
        exports_sec.export("cbak", wasm_encoder::ExportKind::Func, idx);
    }
    module.section(&exports_sec);

    let mut code_sec = wasm_encoder::CodeSection::new();
    let hook_flat = funcs
        .get(&hook_old)
        .context("internal error: hook function state missing after flattening")?;
    code_sec.function(&encode_function(hook_flat)?);
    if let Some(cbak_old) = cbak_old {
        let cbak_flat = funcs
            .get(&cbak_old)
            .context("internal error: cbak function state missing after flattening")?;
        code_sec.function(&encode_function(cbak_flat)?);
    }
    module.section(&code_sec);

    let mut remapper = ir::IndexRemapper::new(|x| x, |x| x);
    module.section(&encode::encode_data_section(&m.datas, &mut remapper)?);

    Ok((module.finish(), report))
}

/// Classifies a callee body's `return` shape, per `docs/DESIGN.md` §6.2b's
/// final paragraph: whether inlining it needs the `block`-wrapper + `return`
/// -> `br` rewrite at all, or can splice the body in bare.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReturnShape {
    /// No `return` anywhere in the body: nothing to rewrite, splice bare.
    NoReturn,
    /// Exactly one `return`, and it is the very last instruction before the
    /// body's own trailing `end`, at depth 0 (top level, not nested in any
    /// `block`/`loop`/`if`): dropping it and falling through is equivalent,
    /// so splice bare.
    TrailingOnly,
    /// Anything else (multiple returns, or a `return` that is nested or not
    /// the final instruction): keep the `block`-wrapper + `br` rewrite.
    NeedsWrapper,
}

/// Pre-scans a callee body to classify its [`ReturnShape`] (`docs/DESIGN.md`
/// §6.2b final paragraph).
fn classify_returns(body: &[wasmparser::Operator]) -> ReturnShape {
    let mut depth: i32 = 0;
    let mut count: usize = 0;
    let mut trailing = false;
    let n = body.len();
    for (i, op) in body.iter().enumerate() {
        match op {
            wasmparser::Operator::Block { .. }
            | wasmparser::Operator::Loop { .. }
            | wasmparser::Operator::If { .. } => depth += 1,
            wasmparser::Operator::End => depth -= 1,
            wasmparser::Operator::Return => {
                count += 1;
                // Trailing iff this `return` sits right before the body's
                // own final `end` (i.e. is the second-to-last operator) and
                // is not nested in any open block/loop/if.
                trailing = depth == 0 && i + 2 == n;
            }
            _ => {}
        }
    }
    match count {
        0 => ReturnShape::NoReturn,
        1 if trailing => ReturnShape::TrailingOnly,
        _ => ReturnShape::NeedsWrapper,
    }
}

/// Splices an already-fully-flattened callee's body into `out`.
/// `arg_locals[i]` is the fresh caller local holding callee parameter `i`;
/// `extra_local_base` is the first fresh caller local holding callee's own
/// (non-parameter) locals.
///
/// Per `docs/DESIGN.md` §6.2b's final paragraph, the callee's [`ReturnShape`]
/// decides how the body is spliced:
///
/// - [`ReturnShape::NeedsWrapper`]: wrapped in a `block` of the callee's
///   result type; every `return` becomes a `br` to that wrapper block, at a
///   depth computed by tracking `block`/`loop`/`if`/`end` nesting as the
///   callee's body is streamed through — the wrapper block is the outermost
///   frame of the spliced body, so a `return` at that top level becomes
///   `br 0`. Every other `br`/`br_if`/`br_table` targets a label internal to
///   the callee's own original nesting and is left unchanged, since wrapping
///   the whole body in one more block adds no *additional* enclosing scope
///   for anything but `return`. The callee body's own trailing `end` closes
///   this wrapper — no extra `end` is ever appended for it.
/// - [`ReturnShape::NoReturn`] / [`ReturnShape::TrailingOnly`]: spliced bare,
///   with no wrapper block at all. The callee body's own trailing `end` is
///   always dropped in this case (there is no wrapper for it to close — left
///   in place it would incorrectly close an *enclosing* caller construct
///   instead). For `TrailingOnly`, the sole trailing `return` is also
///   dropped: with nothing after it and no wrapper collecting a value,
///   letting the computed result simply fall through is equivalent.
fn emit_inlined<'a>(
    out: &mut Vec<wasmparser::Operator<'a>>,
    callee: &FlatFunc<'a>,
    arg_locals: &[u32],
    extra_local_base: u32,
) {
    // (a) Spill arguments: last operand is popped first, so locals are set
    // in reverse parameter order.
    for &local_index in arg_locals.iter().rev() {
        out.push(wasmparser::Operator::LocalSet { local_index });
    }

    let param_count = callee.param_types.len() as u32;
    let remap_local = |local_index: u32| -> u32 {
        if local_index < param_count {
            // Safe: `arg_locals` has exactly `param_count` entries (built
            // from `callee.param_types` at the call site above).
            arg_locals
                .get(local_index as usize)
                .copied()
                .unwrap_or(local_index)
        } else {
            extra_local_base + (local_index - param_count)
        }
    };

    match classify_returns(&callee.body) {
        ReturnShape::NeedsWrapper => {
            // (b) Splice the body, wrapped in a block of the callee's
            // result type.
            let blockty = match callee.result_type {
                None => wasmparser::BlockType::Empty,
                Some(t) => wasmparser::BlockType::Type(t),
            };
            out.push(wasmparser::Operator::Block { blockty });

            let mut depth: u32 = 0;
            for op in &callee.body {
                match op {
                    // (d) `return` -> `br <depth>`.
                    wasmparser::Operator::Return => {
                        out.push(wasmparser::Operator::Br {
                            relative_depth: depth,
                        });
                    }
                    // (c) Remap local references (params -> spill locals,
                    // callee's own locals -> appended caller locals).
                    wasmparser::Operator::LocalGet { local_index } => {
                        out.push(wasmparser::Operator::LocalGet {
                            local_index: remap_local(*local_index),
                        });
                    }
                    wasmparser::Operator::LocalSet { local_index } => {
                        out.push(wasmparser::Operator::LocalSet {
                            local_index: remap_local(*local_index),
                        });
                    }
                    wasmparser::Operator::LocalTee { local_index } => {
                        out.push(wasmparser::Operator::LocalTee {
                            local_index: remap_local(*local_index),
                        });
                    }
                    wasmparser::Operator::Block { .. }
                    | wasmparser::Operator::Loop { .. }
                    | wasmparser::Operator::If { .. } => {
                        depth += 1;
                        out.push(op.clone());
                    }
                    wasmparser::Operator::End => {
                        // The callee body's own trailing `end` (the one that
                        // would otherwise close its function-level implicit
                        // block) closes our synthetic wrapper `block`
                        // instead — no extra `end` is ever appended for the
                        // wrapper. Depth bottoms out at 0 exactly on that
                        // final operator.
                        depth = depth.saturating_sub(1);
                        out.push(wasmparser::Operator::End);
                    }
                    // Everything else (arithmetic, `br`/`br_if`/`br_table`
                    // internal to the callee's own nesting, `unreachable`,
                    // calls to imports, ...) passes through unchanged.
                    other => out.push(other.clone()),
                }
            }
        }
        shape @ (ReturnShape::NoReturn | ReturnShape::TrailingOnly) => {
            // No wrapper block at all: the callee's own trailing `end` is
            // always dropped (nothing wraps it), and for `TrailingOnly` the
            // sole trailing `return` is dropped too.
            let n = callee.body.len();
            let end_at = n.saturating_sub(1);
            let drop_return_at =
                (shape == ReturnShape::TrailingOnly).then(|| end_at.saturating_sub(1));

            for (i, op) in callee.body.iter().enumerate() {
                if i == end_at || Some(i) == drop_return_at {
                    continue;
                }
                match op {
                    wasmparser::Operator::LocalGet { local_index } => {
                        out.push(wasmparser::Operator::LocalGet {
                            local_index: remap_local(*local_index),
                        });
                    }
                    wasmparser::Operator::LocalSet { local_index } => {
                        out.push(wasmparser::Operator::LocalSet {
                            local_index: remap_local(*local_index),
                        });
                    }
                    wasmparser::Operator::LocalTee { local_index } => {
                        out.push(wasmparser::Operator::LocalTee {
                            local_index: remap_local(*local_index),
                        });
                    }
                    other => out.push(other.clone()),
                }
            }
        }
    }
}

/// Encodes a fully-flattened entry function's current state (locals + body)
/// into a `wasm_encoder::Function`. No index remapping is needed here: by
/// construction, a fully-flattened entry's body contains no more `call`
/// targets other than imports, and import indices never change during
/// flatten (only defined non-entry functions are dropped, and `_g` — if
/// added — is appended after every existing import).
fn encode_function(f: &FlatFunc) -> Result<wasm_encoder::Function> {
    // Run-length-encode the flat extra-locals list into (count, type) pairs
    // (not required for correctness, but keeps the encoding compact).
    let mut locals: Vec<(u32, wasm_encoder::ValType)> = Vec::new();
    for ty in &f.extra_locals {
        let ty = ir::conv_valtype(*ty)?;
        match locals.last_mut() {
            Some((count, last_ty)) if *last_ty == ty => *count += 1,
            _ => locals.push((1, ty)),
        }
    }
    let mut func = wasm_encoder::Function::new(locals);
    let mut remapper = ir::IndexRemapper::new(|x| x, |x| x);
    for op in &f.body {
        let translated = remapper
            .instruction(op.clone())
            .map_err(|e| anyhow::anyhow!("failed to translate instruction: {e}"))?;
        func.instruction(&translated);
    }
    Ok(func)
}

/// Computes a reverse topological order (callees before callers) over the
/// direct-call graph restricted to defined functions (imports are leaves
/// and never appear in the result). Standard iterative DFS post-order: for
/// a DAG, every edge `u -> v` has `v` finish before `u`, which is exactly
/// the property the flatten pass's single-pass processing loop relies on.
/// Assumes the graph is already known to be acyclic (checked by the caller
/// via [`ir::find_call_cycle`] beforehand).
fn topo_post_order(m: &ir::ParsedModule, n_imp_funcs: u32, total_funcs: u32) -> Vec<u32> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Unvisited,
        InProgress,
        Done,
    }

    let mut state = vec![State::Unvisited; total_funcs as usize];
    let get = |state: &[State], idx: u32| state.get(idx as usize).copied().unwrap_or(State::Done);
    let set = |state: &mut [State], idx: u32, s: State| {
        if let Some(slot) = state.get_mut(idx as usize) {
            *slot = s;
        }
    };
    let defined_edges = |idx: u32| -> Vec<u32> {
        ir::out_edges(m, idx)
            .into_iter()
            .filter(|&c| c >= n_imp_funcs)
            .collect()
    };

    let mut order = Vec::new();
    for start in n_imp_funcs..total_funcs {
        if get(&state, start) != State::Unvisited {
            continue;
        }
        let mut stack: Vec<(u32, Vec<u32>)> = vec![(start, defined_edges(start))];
        set(&mut state, start, State::InProgress);
        while let Some((node, edges)) = stack.last_mut() {
            let node = *node;
            if let Some(next) = edges.pop() {
                if get(&state, next) == State::Unvisited {
                    set(&mut state, next, State::InProgress);
                    let next_edges = defined_edges(next);
                    stack.push((next, next_edges));
                }
                // `InProgress` would mean a cycle (excluded by the caller's
                // pre-check); `Done` needs no further action.
            } else {
                set(&mut state, node, State::Done);
                order.push(node);
                stack.pop();
            }
        }
    }
    order
}
