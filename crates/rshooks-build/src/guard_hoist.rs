//! The guard-hoist pass: `docs/DESIGN.md` §6.3 (see also `guard.rs`).
//!
//! Runs immediately after [`crate::unnest`] and before the guard check.
//! Binaryen's `wasm-opt -Oz` pass (run early in [`crate::run_pipeline`], see
//! `optimizer.rs`) restructures some loops whose body contains an internal
//! early-exit branch — e.g. a `continue`, or an `if let Err(_) = ..`
//! check — into `loop { block { <guard prologue>; ... } }`: the loop's
//! break/continue logic is wrapped in a `block` so `br_if`/`br` can jump to
//! its end, and that `block` ends up first inside the `loop`, ahead of the
//! guard call the source placed at the very top of the loop. The guard call
//! itself is untouched — still present, unconditional, and still the first
//! *real* instruction the loop runs on every iteration — only its position
//! relative to the `loop` opcode moved, which the upstream checker's exact
//! `loop; i32.const; i32.const; call $_g` prologue match doesn't tolerate.
//!
//! This pass undoes exactly that: when a `loop` is immediately followed by
//! one or more empty-type `block`s whose innermost body opens with the guard
//! prologue `i32.const; i32.const; call $_g; drop`, the prologue is moved
//! out to sit directly after the `loop`, ahead of the `block`s. This is
//! always semantically identical — the prologue is a self-contained,
//! stack-neutral instruction sequence (two pushes, one call that pops both
//! and pushes its i32 result, one drop that pops it) that no label inside
//! the `block`s targets (labels resolve to a `block`'s `end`, never to code
//! preceding it), so moving it earlier changes nothing about what runs or
//! what the operand stack looks like at any point — only that the loop's
//! very first instruction is now the guard call instead of a `block`
//! opener. The trailing `drop` is required, not optional: without it, the
//! call's `i32` result stays on the stack inside the `block` frame (some
//! other instruction in the block's body consumes it there), so moving just
//! the three-instruction prefix ahead of the `block` would leave that
//! consumer without its value and produce an invalid module — a prologue
//! missing its `drop` is left alone. Likewise, a zero `maxiter` prologue is
//! left alone (matching [`crate::guard`]'s own `is_guard_prologue` check):
//! the upstream checker rejects a zero `maxiter` outright, so hoisting one
//! would just report an extra loop as fixed that the checker still
//! rejects. Only rewrites when every intervening `block` has the empty
//! block type (no params/results): a typed block's entry/exit stack shape
//! isn't worth reasoning about here, since none of this crate's own passes
//! nor `wasm-opt -Oz` ever needs a typed block for this shape; a
//! typed-block loop is left for the guard checker to report.
//!
//! Does not touch [`crate::guard::LoopBodyGuess::RotatedGuard`]'s target
//! case: real LLVM loop rotation moves the guard to the loop's *latch*
//! (after the body), not behind a leading `block`, and there is no
//! unconditional, position-independent prologue to hoist there — the
//! `rshooks::guarded_while!` macro (`crates/rshooks/src/macros.rs`) is the
//! source-level fix for that case instead.

use anyhow::{Context, Result};
use wasm_encoder::reencode::Reencode;
use wasmparser::{BlockType, Operator};

use crate::encode;
use crate::guard::find_g_index;
use crate::ir;

/// A defined function's (locals, operator stream) pair — same shape as
/// [`crate::unnest`]'s internal `LocalsAndOps`.
type LocalsAndOps<'a> = (Vec<(u32, wasmparser::ValType)>, Vec<Operator<'a>>);

/// Report from a [`hoist`] run.
#[derive(Debug, Clone, Default)]
pub struct GuardHoistReport {
    /// Human-readable per-function notes, in the same stderr-note style as
    /// [`crate::UnnestReport`]/[`crate::FlattenReport`].
    pub notes: Vec<String>,
    /// Total number of loops whose guard prologue was hoisted, across every
    /// defined function.
    pub loops_hoisted: u32,
}

/// Runs the guard-hoist pass on already-unnested `wasm`. Every defined
/// function's body is processed independently; every other section is
/// copied through unchanged — like [`crate::unnest`], this pass never adds,
/// removes, or renumbers a function, import, global, or type.
pub fn hoist(wasm: &[u8]) -> Result<(Vec<u8>, GuardHoistReport)> {
    let m = ir::parse(wasm)?;
    let g_index = find_g_index(&m);
    let n_imp_funcs = m.num_imported_funcs();

    let mut report = GuardHoistReport::default();
    let mut new_bodies: Vec<LocalsAndOps> = Vec::new();

    for (i, body) in m.code.iter().enumerate() {
        let func_idx = n_imp_funcs + i as u32;

        let mut locals = Vec::new();
        for l in body.get_locals_reader().context("function locals")? {
            let (count, ty) = l.context("function locals")?;
            locals.push((count, ty));
        }
        let mut ops = Vec::new();
        let mut reader = body.get_operators_reader().context("function body")?;
        while !reader.eof() {
            ops.push(reader.read().context("function body operator")?);
        }

        let (new_ops, hoisted) = hoist_function(ops, g_index);
        if hoisted > 0 {
            report.loops_hoisted += hoisted;
            report.notes.push(format!(
                "function {func_idx}: hoisted {hoisted} loop guard(s) out of a leading block"
            ));
        }

        new_bodies.push((locals, new_ops));
    }

    // The common case: nothing to hoist. Return the input unchanged rather
    // than re-encoding a byte-identical module.
    if report.loops_hoisted == 0 {
        return Ok((wasm.to_vec(), report));
    }

    // --- Re-encode. Only the code section's contents change; every other
    // section is a direct structural copy. ---
    let mut module = wasm_encoder::Module::new();

    module.section(&encode::encode_type_section(&m.types)?);

    let imports_sec = encode::encode_import_section(&m.imports, |_ordinal, type_idx| {
        Ok(Some(wasm_encoder::EntityType::Function(type_idx)))
    })?;
    module.section(&imports_sec);

    let mut funcs_sec = wasm_encoder::FunctionSection::new();
    for &type_idx in &m.defined_func_types {
        funcs_sec.function(type_idx);
    }
    module.section(&funcs_sec);

    module.section(&encode::encode_memory_section(&m.memories)?);

    let mut remapper = ir::IndexRemapper::new(|x| x, |x| x);
    module.section(&encode::encode_global_section(&m.globals, &mut remapper)?);

    let mut exports_sec = wasm_encoder::ExportSection::new();
    for e in &m.exports {
        let kind = encode::conv_export_kind(e.kind)?;
        exports_sec.export(e.name, kind, e.index);
    }
    module.section(&exports_sec);

    let mut code_sec = wasm_encoder::CodeSection::new();
    for (locals, ops) in &new_bodies {
        let enc_locals: Vec<(u32, wasm_encoder::ValType)> = locals
            .iter()
            .map(|&(count, ty)| Ok((count, ir::conv_valtype(ty)?)))
            .collect::<Result<_>>()?;
        let mut func = wasm_encoder::Function::new(enc_locals);
        let mut remapper = ir::IndexRemapper::new(|x| x, |x| x);
        for op in ops {
            let translated = remapper
                .instruction(op.clone())
                .map_err(|e| anyhow::anyhow!("failed to translate instruction: {e}"))?;
            func.instruction(&translated);
        }
        code_sec.function(&func);
    }
    module.section(&code_sec);

    let mut remapper = ir::IndexRemapper::new(|x| x, |x| x);
    module.section(&encode::encode_data_section(&m.datas, &mut remapper)?);

    Ok((module.finish(), report))
}

/// Rewrites one function body, hoisting every qualifying loop's guard
/// prologue. Returns the rewritten operator stream and the number of loops
/// hoisted.
fn hoist_function<'a>(ops: Vec<Operator<'a>>, g_index: Option<u32>) -> (Vec<Operator<'a>>, u32) {
    let Some(g_index) = g_index else {
        return (ops, 0);
    };

    let mut out = Vec::with_capacity(ops.len());
    let mut hoisted = 0u32;
    let mut i = 0usize;
    while let Some(op) = ops.get(i) {
        if matches!(op, Operator::Loop { .. })
            && let Some((prologue, openers, next_i)) = hoist_candidate(&ops, i, g_index)
        {
            out.push(op.clone());
            out.extend(prologue.iter().cloned());
            out.extend(openers.iter().cloned());
            hoisted += 1;
            i = next_i;
            continue;
        }
        out.push(op.clone());
        i += 1;
    }
    (out, hoisted)
}

/// If the loop opening at `ops[loop_idx]` is immediately followed by one or
/// more empty `block`s whose innermost body opens with a guard prologue,
/// returns `(the prologue, the block openers, the index just past the
/// prologue)` — everything the caller needs to rewrite `loop; block...;
/// guard` into `loop; guard; block...`.
fn hoist_candidate<'a, 'b>(
    ops: &'b [Operator<'a>],
    loop_idx: usize,
    g_index: u32,
) -> Option<(&'b [Operator<'a>], &'b [Operator<'a>], usize)> {
    let block_start = loop_idx + 1;
    let mut j = block_start;
    while matches!(
        ops.get(j),
        Some(Operator::Block {
            blockty: BlockType::Empty
        })
    ) {
        j += 1;
    }
    if j == block_start {
        return None; // no block immediately after the loop
    }
    let rest = ops.get(j..)?;
    let prologue_len = guard_prologue_len(rest, g_index)?;
    let prologue = ops.get(j..j + prologue_len)?;
    let openers = ops.get(block_start..j)?;
    Some((prologue, openers, j + prologue_len))
}

/// Length (in operators) of a guard prologue — `i32.const; i32.const; call
/// $_g; drop` — starting at the front of `ops`, or `None` if `ops` doesn't
/// start with one. `g_index` is the `_g` import's function index; a `call`
/// to anything else doesn't count. Requires both the trailing `drop` (see
/// this module's doc comment for why a prologue missing it must not be
/// hoisted) and a non-zero `maxiter` on the second `i32.const`, matching
/// [`crate::guard`]'s own `is_guard_prologue` check.
fn guard_prologue_len(ops: &[Operator], g_index: u32) -> Option<usize> {
    let mut idx = 0;
    if !matches!(ops.get(idx)?, Operator::I32Const { .. }) {
        return None;
    }
    idx += 1;
    if !matches!(ops.get(idx)?, Operator::I32Const { value } if *value != 0) {
        return None;
    }
    idx += 1;
    match ops.get(idx)? {
        Operator::Call { function_index } if *function_index == g_index => {}
        _ => return None,
    }
    idx += 1;
    if !matches!(ops.get(idx)?, Operator::Drop) {
        return None;
    }
    idx += 1;
    Some(idx)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const G_INDEX: u32 = 0;

    fn guard_ops<'a>() -> Vec<Operator<'a>> {
        vec![
            Operator::I32Const { value: 1 },
            Operator::I32Const { value: 10 },
            Operator::Call {
                function_index: G_INDEX,
            },
            Operator::Drop,
        ]
    }

    fn empty_block<'a>() -> Operator<'a> {
        Operator::Block {
            blockty: BlockType::Empty,
        }
    }

    #[test]
    fn loop_block_guard_is_hoisted() {
        let mut ops = vec![
            Operator::Loop {
                blockty: BlockType::Empty,
            },
            empty_block(),
        ];
        ops.extend(guard_ops());
        ops.push(Operator::End); // closes block
        ops.push(Operator::End); // closes loop

        let (out, hoisted) = hoist_function(ops, Some(G_INDEX));
        assert_eq!(hoisted, 1);

        let mut expected = vec![Operator::Loop {
            blockty: BlockType::Empty,
        }];
        expected.extend(guard_ops());
        expected.push(empty_block());
        expected.push(Operator::End);
        expected.push(Operator::End);
        assert_eq!(out, expected);
    }

    #[test]
    fn loop_nested_blocks_guard_is_hoisted_through_both() {
        let mut ops = vec![
            Operator::Loop {
                blockty: BlockType::Empty,
            },
            empty_block(),
            empty_block(),
        ];
        ops.extend(guard_ops());
        ops.push(Operator::End);
        ops.push(Operator::End);
        ops.push(Operator::End);

        let (out, hoisted) = hoist_function(ops, Some(G_INDEX));
        assert_eq!(hoisted, 1);

        let mut expected = vec![Operator::Loop {
            blockty: BlockType::Empty,
        }];
        expected.extend(guard_ops());
        expected.push(empty_block());
        expected.push(empty_block());
        expected.push(Operator::End);
        expected.push(Operator::End);
        expected.push(Operator::End);
        assert_eq!(out, expected);
    }

    #[test]
    fn loop_block_non_guard_is_untouched() {
        let ops = vec![
            Operator::Loop {
                blockty: BlockType::Empty,
            },
            empty_block(),
            Operator::I32Const { value: 1 },
            Operator::Drop,
            Operator::End,
            Operator::End,
        ];
        let (out, hoisted) = hoist_function(ops.clone(), Some(G_INDEX));
        assert_eq!(hoisted, 0);
        assert_eq!(out, ops);
    }

    #[test]
    fn loop_guard_already_at_head_is_untouched() {
        let mut ops = vec![Operator::Loop {
            blockty: BlockType::Empty,
        }];
        ops.extend(guard_ops());
        ops.push(Operator::End);
        let (out, hoisted) = hoist_function(ops.clone(), Some(G_INDEX));
        assert_eq!(hoisted, 0);
        assert_eq!(out, ops);
    }

    #[test]
    fn no_g_import_is_a_no_op() {
        let mut ops = vec![
            Operator::Loop {
                blockty: BlockType::Empty,
            },
            empty_block(),
        ];
        ops.extend(guard_ops());
        ops.push(Operator::End);
        ops.push(Operator::End);
        let (out, hoisted) = hoist_function(ops.clone(), None);
        assert_eq!(hoisted, 0);
        assert_eq!(out, ops);
    }

    #[test]
    fn prologue_missing_drop_is_untouched() {
        // Without the trailing `drop`, the call's result is consumed by
        // something else inside the block frame — hoisting just the
        // `i32.const; i32.const; call` prefix ahead of the block would
        // leave that consumer without its value.
        let ops = vec![
            Operator::Loop {
                blockty: BlockType::Empty,
            },
            empty_block(),
            Operator::I32Const { value: 1 },
            Operator::I32Const { value: 10 },
            Operator::Call {
                function_index: G_INDEX,
            },
            Operator::Nop, // stands in for "something else consumes the result"
            Operator::End,
            Operator::End,
        ];
        let (out, hoisted) = hoist_function(ops.clone(), Some(G_INDEX));
        assert_eq!(hoisted, 0);
        assert_eq!(out, ops);
    }

    #[test]
    fn zero_maxiter_prologue_is_untouched() {
        // The upstream checker rejects a zero `maxiter` outright, so
        // hoisting this prologue would just report a loop as fixed that
        // the checker still rejects.
        let ops = vec![
            Operator::Loop {
                blockty: BlockType::Empty,
            },
            empty_block(),
            Operator::I32Const { value: 1 },
            Operator::I32Const { value: 0 },
            Operator::Call {
                function_index: G_INDEX,
            },
            Operator::Drop,
            Operator::End,
            Operator::End,
        ];
        let (out, hoisted) = hoist_function(ops.clone(), Some(G_INDEX));
        assert_eq!(hoisted, 0);
        assert_eq!(out, ops);
    }

    #[test]
    fn typed_block_is_untouched() {
        let ops = vec![
            Operator::Loop {
                blockty: BlockType::Empty,
            },
            Operator::Block {
                blockty: BlockType::Type(wasmparser::ValType::I32),
            },
            Operator::I32Const { value: 1 },
            Operator::End,
            Operator::Drop,
            Operator::End,
        ];
        let (out, hoisted) = hoist_function(ops.clone(), Some(G_INDEX));
        assert_eq!(hoisted, 0);
        assert_eq!(out, ops);
    }
}
