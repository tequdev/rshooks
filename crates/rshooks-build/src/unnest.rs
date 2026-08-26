//! The unnest (ladder-flattening) pass: `docs/DESIGN.md` §6.2c.
//!
//! Runs for api-version 0 only, immediately after [`crate::flatten`] and
//! before the guard pass. It exists because LLVM's stackifier lays out every
//! diverging early-exit (`rollback!`-style) as a tail after the end of a
//! dedicated `block` wrapping the whole remaining body — an "error ladder"
//! whose nesting grows linearly with the number of error paths a hook
//! checks. The vendored upstream checker rejects modules whose `block`/
//! `loop`/`if` nesting exceeds 32 levels (`Guard.h` `NESTING_LIMIT`,
//! `GuardRuleDepth32`), which a hook with a few dozen checks would hit
//! regardless of guard correctness.
//!
//! # Algorithm
//!
//! For each defined function body (after flatten, this is exactly the entry
//! points — `hook` and, if present, `cbak` — but the pass itself does not
//! assume that and just processes every defined function):
//!
//! 1. **Find qualifying blocks.** An empty-blocktype `block` qualifies if
//!    its *continuation* — the instructions immediately following its
//!    matching `end`, scanned forward — is a **self-contained diverging
//!    tail**: a symbolic stack simulation starting from an empty stack,
//!    allowing only `i32.const`/`i64.const`, `local.get`, `call` (to an
//!    *imported* function only), and `drop`, never popping below empty,
//!    terminating at `unreachable`. Hitting anything else (a branch, a
//!    nested block, `local.set`, running out of instructions without
//!    `unreachable`) disqualifies it.
//! 2. **Rewrite referencing branches.** Every `br_if` targeting a qualifying
//!    block becomes `if (empty blocktype) { <tail> } end` spliced at the
//!    branch site (the popped condition becomes the `if`'s condition — same
//!    stack effect); every plain `br` becomes the tail spliced directly (no
//!    `if`, since control reaches it unconditionally). The tail is
//!    self-contained and branch-free, so splicing it at a different nesting
//!    depth than where it originally lived changes nothing about its
//!    behavior — that invariant is exactly what qualification buys.
//!    `br_table` is never rewritten (LLVM ladders only ever use `br`/
//!    `br_if`); see "`br_table` safety" below for what this means for
//!    blocks it targets.
//! 3. **Unwrap unreferenced blocks.** Any empty-blocktype block no longer
//!    targeted by *any* branch (this also catches pre-existing unreferenced
//!    wrapper blocks left over from flatten, whose `return`-rewrite never
//!    materialized) is removed — its `block`/`end` tokens are dropped, and
//!    every branch that was nested inside it and targeted a label *outside*
//!    it has its `relative_depth` decremented by 1 (removing the frame means
//!    one fewer level of nesting to cross to reach that same target; a
//!    branch whose target was *inside* the removed block is unaffected,
//!    since the removed frame was never "between" it and its target).
//! 4. **Iterate to fixpoint.** Steps 1–3 repeat until a full pass rewrites
//!    and removes nothing. This terminates because every non-trivial pass
//!    either splices at least one tail (which immediately makes its target
//!    block unreferenced, guaranteeing at least one removal in the very
//!    same pass) or removes at least one pre-existing unreferenced block —
//!    either way the count of `block` tokens remaining strictly decreases,
//!    bounded below by 0. In practice a single ladder (of any depth) is
//!    fully collapsed in one pass: every level's continuation is
//!    independent of the others (each qualifies purely from what follows
//!    its *own* `end`), so all of them are found, rewritten, and removed
//!    together.
//!
//! # `br_table` safety
//!
//! `wasmparser::Operator::BrTable` cannot be reconstructed with a different
//! set of target depths from outside the crate (its `targets` field is a
//! borrowed-reader type with private internals) — so no code path in this
//! pass may ever need to renumber one. Two rules guarantee that:
//!
//! - A block *targeted* by any `br_table` is never treated as qualifying
//!   (never rewritten, and — since its referencing `br_table` is never
//!   touched — it remains referenced and is therefore never removed
//!   either).
//! - A block whose span *contains* a `br_table` anywhere inside it is never
//!   removed (removing it could require renumbering that `br_table`'s
//!   depths). It can still be rewritten if it qualifies (rewriting only
//!   touches branches *referencing* it, not its interior), but if it can
//!   never become removable this way, that rewrite would just add bytes for
//!   no depth benefit — see `unnest_function`'s `unsafe_frames` computation,
//!   used for both exclusions uniformly.
//!
//! This is conservative (a `block` mixing an interior `br_table` — e.g. from
//! a Rust `match`, unrelated to any ladder — with an otherwise-qualifying
//! continuation is left in place) but always correct, and LLVM-generated
//! error ladders only ever use `br`/`br_if`.
//!
//! # Dead-code elimination (post-pass)
//!
//! Step 3 removes a qualifying block's own `block`/`end` tokens, but it does
//! *not* remove the instructions that originally followed that `end` — the
//! very tail step 1 scanned to decide the block qualified in the first
//! place. Those instructions are still needed as a *program text* while the
//! block frame exists (steps 1–2 read them, don't move them), but once the
//! frame is gone in step 3, they're left sitting in straight-line code
//! immediately after whatever unconditional terminator now precedes them at
//! that same nesting level — usually the `unreachable` that used to end the
//! block's own body, or a spliced tail's `unreachable` from step 2. Wasm's
//! operand-stack polymorphism after an unconditional terminator (`Guard.h`'s
//! own checker sums instructions *syntactically*, not by reachability) means
//! this leftover tail stays well-typed and present in the emitted binary
//! even though it can never execute — inflating the reported worst-case
//! instruction count for code that is provably dead.
//!
//! After step 4's fixpoint loop finishes, [`eliminate_dead_code`] makes one
//! more linear pass over the (already-flattened, already-unnested) function
//! body to drop exactly that kind of leftover: every instruction following
//! an unconditional terminator (`unreachable`, `br`, `br_table`, `return`)
//! at the same nesting level, up to (not including) the `end`/`else` that
//! closes that level. A nested `block`/`loop`/`if` encountered while
//! already dead is dropped as one whole unit — its own opening instruction
//! is never emitted and its interior is never inspected — rather than
//! descended into, so no frame is ever *partially* dropped. Combined with
//! the fact that no *enclosing* frame is ever removed (only whole nested
//! spans, and only ones that were themselves unreachable), no surviving
//! branch's `relative_depth` is ever affected: every frame a surviving
//! branch could target either still encloses it exactly as before, or was
//! deleted together with the branch itself (since a branch inside a
//! wholly-dropped span is unreachable code too, and is dropped along with
//! it). One linear pass is exhaustive — unlike steps 1–4, which alternate
//! rewriting and removal because a rewrite can create a *new* removal
//! candidate, dropping dead code never creates more dead code for a later
//! pass to find, since the scan already follows every nesting level (and
//! every wholly-dropped span) to its own closing token in one traversal.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use wasm_encoder::reencode::Reencode;
use wasmparser::{BlockType, Operator};

use crate::ir;

/// A defined function's (locals, operator stream) pair, as produced by the
/// per-function unnest transform and consumed by the final re-encode step.
type LocalsAndOps<'a> = (Vec<(u32, wasmparser::ValType)>, Vec<Operator<'a>>);

/// Report from an [`unnest`] run.
#[derive(Debug, Clone, Default)]
pub struct UnnestReport {
    /// Human-readable per-function notes (block/tail counts, depth
    /// before/after), in the same stderr-note style as
    /// [`crate::FlattenReport`].
    pub notes: Vec<String>,
    /// Total number of `block` instructions removed (unwrapped) across
    /// every defined function.
    pub blocks_removed: u32,
    /// Total number of branch sites (`br`/`br_if`) rewritten into a spliced
    /// tail — each rewrite duplicates that tail's instructions once.
    pub tails_duplicated: u32,
    /// Total number of instructions dropped by the post-fixpoint dead-code
    /// elimination pass (see the module doc comment) across every defined
    /// function — leftover unreachable tails from step 3's frame removal.
    pub dead_ops_removed: u32,
}

/// Runs the unnest pass on already-flattened `wasm`. Every defined
/// function's body is processed independently; every other section (types,
/// imports, function/table/memory/global declarations, exports, data) is
/// copied through unchanged — unnest never adds, removes, or renumbers a
/// function, import, global, or type; it only rewrites code bodies.
pub fn unnest(wasm: &[u8]) -> Result<(Vec<u8>, UnnestReport)> {
    let m = ir::parse(wasm)?;
    let n_imp_funcs = m.num_imported_funcs();

    // Resolves a `call` target's (param count, result count) iff it is an
    // *imported* function — per docs/DESIGN.md §6.2c step 1, only calls to
    // imports may appear in a qualifying tail. After flatten there should be
    // no defined callees left at all, but this pass does not assume that.
    let import_arity = |function_index: u32| -> Option<(u32, u32)> {
        if function_index >= n_imp_funcs {
            return None;
        }
        let ty_idx = m.func_type_index(function_index)?;
        let ty = m.types.get(ty_idx as usize)?;
        Some((ty.params().len() as u32, ty.results().len() as u32))
    };

    let mut report = UnnestReport::default();
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

        let depth_before = max_depth(&ops);
        let (new_ops, stats) = unnest_function(ops, &import_arity)?;
        let depth_after = max_depth(&new_ops);

        report.blocks_removed += stats.blocks_removed;
        report.tails_duplicated += stats.tails_duplicated;
        report.dead_ops_removed += stats.dead_ops_removed;
        if stats.blocks_removed > 0 || stats.tails_duplicated > 0 || stats.dead_ops_removed > 0 {
            report.notes.push(format!(
                "function {func_idx}: unnest removed {} block(s), duplicated {} tail(s), \
                 eliminated {} dead instruction(s) (max nesting depth {depth_before} -> {depth_after})",
                stats.blocks_removed, stats.tails_duplicated, stats.dead_ops_removed
            ));
        }

        new_bodies.push((locals, new_ops));
    }

    // --- Re-encode. Only the code section's contents change; every other
    // section is a direct structural copy (no index space changes at all,
    // unlike cleaner/flatten). ---
    let mut module = wasm_encoder::Module::new();

    let mut types_sec = wasm_encoder::TypeSection::new();
    for ty in &m.types {
        let (params, results) = ir::conv_functype(ty)?;
        types_sec.ty().function(params, results);
    }
    module.section(&types_sec);

    let mut imports_sec = wasm_encoder::ImportSection::new();
    for imp in &m.imports {
        match imp.ty {
            wasmparser::TypeRef::Func(type_idx) => {
                imports_sec.import(
                    imp.module,
                    imp.name,
                    wasm_encoder::EntityType::Function(type_idx),
                );
            }
            wasmparser::TypeRef::Table(t) => {
                imports_sec.import(imp.module, imp.name, ir::conv_tabletype(t)?);
            }
            wasmparser::TypeRef::Memory(mt) => {
                imports_sec.import(imp.module, imp.name, ir::conv_memtype(mt));
            }
            wasmparser::TypeRef::Global(gt) => {
                imports_sec.import(imp.module, imp.name, ir::conv_globaltype(gt)?);
            }
            wasmparser::TypeRef::Tag(_) => {
                anyhow::bail!("unsupported import: tag imports are not supported");
            }
            wasmparser::TypeRef::FuncExact(_) => {
                anyhow::bail!(
                    "unsupported import: exact function-reference imports are not supported"
                );
            }
        }
    }
    module.section(&imports_sec);

    let mut funcs_sec = wasm_encoder::FunctionSection::new();
    for &type_idx in &m.defined_func_types {
        funcs_sec.function(type_idx);
    }
    module.section(&funcs_sec);

    let mut mem_sec = wasm_encoder::MemorySection::new();
    for &mem in &m.memories {
        mem_sec.memory(ir::conv_memtype(mem));
    }
    module.section(&mem_sec);

    let mut globals_sec = wasm_encoder::GlobalSection::new();
    {
        let mut remapper = ir::IndexRemapper::new(|x| x, |x| x);
        for g in &m.globals {
            let ty = ir::conv_globaltype(g.ty)?;
            let expr = ir::remap_const_expr(&g.init_expr, &mut remapper)?;
            globals_sec.global(ty, &expr);
        }
    }
    module.section(&globals_sec);

    let mut exports_sec = wasm_encoder::ExportSection::new();
    for e in &m.exports {
        let kind = match e.kind {
            wasmparser::ExternalKind::Func => wasm_encoder::ExportKind::Func,
            wasmparser::ExternalKind::Table => wasm_encoder::ExportKind::Table,
            wasmparser::ExternalKind::Memory => wasm_encoder::ExportKind::Memory,
            wasmparser::ExternalKind::Global => wasm_encoder::ExportKind::Global,
            wasmparser::ExternalKind::Tag => wasm_encoder::ExportKind::Tag,
            wasmparser::ExternalKind::FuncExact => anyhow::bail!(
                "unsupported export: exact function-reference exports are not supported"
            ),
        };
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

    let mut data_sec = wasm_encoder::DataSection::new();
    {
        let mut remapper = ir::IndexRemapper::new(|x| x, |x| x);
        for d in &m.datas {
            match &d.kind {
                wasmparser::DataKind::Active {
                    memory_index,
                    offset_expr,
                } => {
                    let expr = ir::remap_const_expr(offset_expr, &mut remapper)?;
                    data_sec.active(*memory_index, &expr, d.data.iter().copied());
                }
                wasmparser::DataKind::Passive => {
                    data_sec.passive(d.data.iter().copied());
                }
            }
        }
    }
    module.section(&data_sec);

    Ok((module.finish(), report))
}

/// Per-function statistics accumulated by [`unnest_function`].
#[derive(Default)]
struct FuncStats {
    blocks_removed: u32,
    tails_duplicated: u32,
    dead_ops_removed: u32,
}

/// One `block`/`loop`/`if` frame found while scanning a function body,
/// identified by the position (in the flat operator array) of its own
/// opening opcode — which never changes for a frame that survives a given
/// pass, so it doubles as a stable identity.
#[derive(Clone, Copy)]
struct Frame {
    start: usize,
    end: usize,
    kind: FrameKind,
    blockty: BlockType,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Block,
    Loop,
    If,
}

/// The result of a single scan over a function body's flat operator array:
/// every `block`/`loop`/`if` frame (matched to its `end`), the target frame
/// (by `start` position, `None` = implicit function-level scope) of every
/// `br`/`br_if`, and the two `br_table`-safety sets described in the module
/// doc comment.
struct Analysis {
    frames: Vec<Frame>,
    /// Op index (of a `Br`/`BrIf`) -> its target frame's `start`, or `None`
    /// if it targets the implicit function-level scope.
    br_target: HashMap<usize, Option<usize>>,
    /// Frame `start` positions targeted by *any* `br_table` (default or any
    /// table entry).
    br_table_targets: HashSet<usize>,
    /// Frame `start` positions that (directly or transitively) enclose *any*
    /// `br_table` instruction.
    br_table_enclosing: HashSet<usize>,
}

fn analyze(ops: &[Operator]) -> Analysis {
    let mut frames: Vec<Frame> = Vec::new();
    let mut open: Vec<usize> = Vec::new(); // indices into `frames`, outer..inner
    let mut br_target = HashMap::new();
    let mut br_table_targets = HashSet::new();
    let mut br_table_enclosing = HashSet::new();

    let resolve = |open: &[usize], frames: &[Frame], relative_depth: u32| -> Option<usize> {
        let len = open.len();
        if (relative_depth as usize) < len {
            open.get(len - 1 - relative_depth as usize)
                .and_then(|&fi| frames.get(fi))
                .map(|f| f.start)
        } else {
            None
        }
    };

    for (i, op) in ops.iter().enumerate() {
        match op {
            Operator::Block { blockty } => {
                frames.push(Frame {
                    start: i,
                    end: usize::MAX,
                    kind: FrameKind::Block,
                    blockty: *blockty,
                });
                open.push(frames.len() - 1);
            }
            Operator::Loop { blockty } => {
                frames.push(Frame {
                    start: i,
                    end: usize::MAX,
                    kind: FrameKind::Loop,
                    blockty: *blockty,
                });
                open.push(frames.len() - 1);
            }
            Operator::If { blockty } => {
                frames.push(Frame {
                    start: i,
                    end: usize::MAX,
                    kind: FrameKind::If,
                    blockty: *blockty,
                });
                open.push(frames.len() - 1);
            }
            Operator::Else => {}
            Operator::End => {
                if let Some(fi) = open.pop()
                    && let Some(f) = frames.get_mut(fi)
                {
                    f.end = i;
                }
                // Otherwise this is the function's own trailing `end`
                // (stack already empty at the top level) — no frame to
                // close.
            }
            Operator::Br { relative_depth } | Operator::BrIf { relative_depth } => {
                let target = resolve(&open, &frames, *relative_depth);
                br_target.insert(i, target);
            }
            Operator::BrTable { targets } => {
                for &fi in &open {
                    if let Some(f) = frames.get(fi) {
                        br_table_enclosing.insert(f.start);
                    }
                }
                if let Some(t) = resolve(&open, &frames, targets.default()) {
                    br_table_targets.insert(t);
                }
                for entry in targets.targets() {
                    // `targets()` only errors on a malformed encoding, which
                    // cannot occur here: `ops` was itself produced by
                    // successfully reading every operator out of a body that
                    // already round-tripped through the operators reader.
                    if let Ok(d) = entry
                        && let Some(t) = resolve(&open, &frames, d)
                    {
                        br_table_targets.insert(t);
                    }
                }
            }
            _ => {}
        }
    }

    Analysis {
        frames,
        br_target,
        br_table_targets,
        br_table_enclosing,
    }
}

/// Step 1 of the algorithm: attempts to extract a self-contained diverging
/// tail starting at operator index `from` (right after a candidate block's
/// matching `end`). Returns `None` if the scan hits anything disallowed
/// (including simply running out of instructions) before reaching
/// `unreachable`.
fn extract_tail<'a>(
    ops: &[Operator<'a>],
    from: usize,
    import_arity: &impl Fn(u32) -> Option<(u32, u32)>,
) -> Option<Vec<Operator<'a>>> {
    let mut i = from;
    let mut stack_depth: i64 = 0;
    let mut tail = Vec::new();
    loop {
        let op = ops.get(i)?;
        match op {
            Operator::I32Const { .. } | Operator::I64Const { .. } | Operator::LocalGet { .. } => {
                stack_depth += 1;
                tail.push(op.clone());
            }
            Operator::Drop => {
                if stack_depth <= 0 {
                    return None; // would pop below empty
                }
                stack_depth -= 1;
                tail.push(op.clone());
            }
            Operator::Call { function_index } => {
                let (params, results) = import_arity(*function_index)?;
                if stack_depth < i64::from(params) {
                    return None; // would pop below empty
                }
                stack_depth = stack_depth - i64::from(params) + i64::from(results);
                tail.push(op.clone());
            }
            Operator::Unreachable => {
                tail.push(op.clone());
                return Some(tail);
            }
            // A branch, a nested block/loop/if, `local.set`, or anything
            // else not explicitly allowed above disqualifies the tail.
            _ => return None,
        }
        i += 1;
    }
}

/// Applies the unnest algorithm (steps 1–4) to one function's body, looping
/// to fixpoint. `import_arity` resolves an imported call target's (param
/// count, result count); see [`extract_tail`].
fn unnest_function<'a>(
    mut ops: Vec<Operator<'a>>,
    import_arity: &impl Fn(u32) -> Option<(u32, u32)>,
) -> Result<(Vec<Operator<'a>>, FuncStats)> {
    let mut stats = FuncStats::default();

    // Safety bound on the fixpoint loop: every iteration that changes
    // anything strictly reduces the number of `block` tokens remaining (see
    // the module doc comment's termination argument), so this can never
    // legitimately be exceeded — it only guards against a logic bug turning
    // into an infinite loop instead of a clear error.
    let max_iters = ops.len().saturating_mul(2).saturating_add(16);

    for _ in 0..max_iters {
        let analysis = analyze(&ops);
        let unsafe_frames: HashSet<usize> = analysis
            .br_table_targets
            .iter()
            .chain(analysis.br_table_enclosing.iter())
            .copied()
            .collect();

        // --- Step 1: find qualifying candidates. ---
        let mut tails: HashMap<usize, Vec<Operator<'a>>> = HashMap::new();
        for f in &analysis.frames {
            // `f.end == usize::MAX` would mean this frame's `end` was never
            // matched, which cannot happen for a well-formed body (every
            // block/loop/if here came from wasm that already round-tripped
            // through full module validation) — skip defensively rather
            // than wrap.
            if f.kind == FrameKind::Block
                && f.blockty == BlockType::Empty
                && !unsafe_frames.contains(&f.start)
                && f.end != usize::MAX
                && let Some(tail) = extract_tail(&ops, f.end + 1, import_arity)
            {
                tails.insert(f.start, tail);
            }
        }

        // --- Step 2: rewrite every branch targeting a qualifying block. ---
        let mut rewritten = Vec::with_capacity(ops.len());
        let mut any_rewrite = false;
        for (i, op) in ops.iter().enumerate() {
            match op {
                Operator::Br { .. } => {
                    if let Some(Some(target)) = analysis.br_target.get(&i)
                        && let Some(tail) = tails.get(target)
                    {
                        rewritten.extend(tail.iter().cloned());
                        any_rewrite = true;
                        stats.tails_duplicated += 1;
                        continue;
                    }
                    rewritten.push(op.clone());
                }
                Operator::BrIf { .. } => {
                    if let Some(Some(target)) = analysis.br_target.get(&i)
                        && let Some(tail) = tails.get(target)
                    {
                        rewritten.push(Operator::If {
                            blockty: BlockType::Empty,
                        });
                        rewritten.extend(tail.iter().cloned());
                        rewritten.push(Operator::End);
                        any_rewrite = true;
                        stats.tails_duplicated += 1;
                        continue;
                    }
                    rewritten.push(op.clone());
                }
                _ => rewritten.push(op.clone()),
            }
        }
        ops = rewritten;

        // --- Step 3: unwrap every now-unreferenced empty block (also
        // catches pre-existing unreferenced blocks, e.g. flatten leftover
        // wrapper blocks whose `return` rewrite never materialized). ---
        let analysis2 = analyze(&ops);
        let unsafe_frames2: HashSet<usize> = analysis2
            .br_table_targets
            .iter()
            .chain(analysis2.br_table_enclosing.iter())
            .copied()
            .collect();
        let referenced: HashSet<usize> = analysis2
            .br_target
            .values()
            .flatten()
            .copied()
            .chain(analysis2.br_table_targets.iter().copied())
            .collect();
        let removable: HashSet<usize> = analysis2
            .frames
            .iter()
            .filter(|f| {
                f.kind == FrameKind::Block
                    && f.blockty == BlockType::Empty
                    && !referenced.contains(&f.start)
                    && !unsafe_frames2.contains(&f.start)
            })
            .map(|f| f.start)
            .collect();

        if removable.is_empty() {
            if !any_rewrite {
                break; // fixpoint: nothing rewritten, nothing to remove
            }
            continue;
        }

        stats.blocks_removed += removable.len() as u32;
        ops = remove_frames(&ops, &removable);
    }

    // --- Post-pass: drop the leftover dead tails step 3's frame removal
    // creates (see the module doc comment). Runs once, after the fixpoint
    // above settles — it never creates a new removal opportunity for steps
    // 1–3, since it only deletes code that was already unreachable. ---
    let (ops, dead_ops_removed) = eliminate_dead_code(ops);
    stats.dead_ops_removed = dead_ops_removed;

    Ok((ops, stats))
}

/// Post-fixpoint dead-code elimination: see the module doc comment's
/// "Dead-code elimination (post-pass)" section for the full rationale.
///
/// Makes one linear pass over `ops`, tracking a single `dead` flag scoped to
/// whatever nesting level is currently being scanned (there is never a need
/// for a *stack* of these — a new level is only ever entered while `dead` is
/// `false`, since a level entered while `dead` is `true` is instead dropped
/// whole via `skip_depth` below, so the level a `block`/`loop`/`if`'s `end`
/// returns to is always `false` by construction). Once an unconditional
/// terminator (`unreachable`, `br`, `br_table`, `return`) is emitted while
/// alive, `dead` flips to `true` and every following instruction at the same
/// level is dropped until the `else`/`end` that closes it, which resets
/// `dead` to `false` for the next region (the `else` arm, or whatever
/// follows the closing `end`).
///
/// `skip_depth` handles the one case a flat `dead` flag alone can't: a
/// nested `block`/`loop`/`if` whose own *opening* instruction is itself
/// already dead. That whole span — however deeply nested internally — is
/// dropped as a single unit: its instructions are never emitted and never
/// individually inspected, so no branch inside it (necessarily also dead
/// code, since a branch can only target a frame that lexically encloses it)
/// needs its own handling, and no frame outside the span is touched at all.
fn eliminate_dead_code<'a>(ops: Vec<Operator<'a>>) -> (Vec<Operator<'a>>, u32) {
    let mut out = Vec::with_capacity(ops.len());
    let mut removed: u32 = 0;
    let mut dead = false;
    // >0 while dropping a whole nested `block`/`loop`/`if` ... `end` span
    // that is itself dead code; counts that span's own internal nesting so
    // its matching `end` is found without emitting or inspecting anything
    // in between. `else` doesn't change this count: it doesn't close the
    // `if` frame, only `end` does.
    let mut skip_depth: u32 = 0;

    for op in ops {
        if skip_depth > 0 {
            match op {
                Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => {
                    skip_depth += 1;
                }
                Operator::End => skip_depth -= 1,
                _ => {}
            }
            removed += 1;
            continue;
        }

        match op {
            Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => {
                if dead {
                    // Drop the whole span rather than descending into it —
                    // see the doc comment above.
                    skip_depth = 1;
                    removed += 1;
                    continue;
                }
                out.push(op);
                // `dead` is already `false` here (the `if dead` branch
                // above returns before reaching this point) — the newly
                // opened region starts alive, as it must.
            }
            Operator::Else | Operator::End => {
                // Closes the current region (an `if`'s `then` arm, or the
                // enclosing `block`/`loop`/`if`/function scope): always
                // kept even when `dead`, since it's the boundary token the
                // dead run stops at, not part of the dead run itself. The
                // next region starts alive.
                out.push(op);
                dead = false;
            }
            Operator::Unreachable
            | Operator::Br { .. }
            | Operator::BrTable { .. }
            | Operator::Return => {
                if dead {
                    // Already unreachable code following an earlier
                    // terminator at this same level — drop it like any
                    // other dead instruction, same as the `_` arm below.
                    removed += 1;
                    continue;
                }
                out.push(op);
                dead = true;
            }
            _ => {
                if dead {
                    removed += 1;
                    continue;
                }
                out.push(op);
            }
        }
    }

    (out, removed)
}

/// Removes every `block` frame whose `start` is in `removable` (dropping
/// both its opening `block` and matching `end`), decrementing every
/// surviving `br`/`br_if`'s `relative_depth` that targeted a label *outside*
/// a removed frame by the number of removed frames it had to cross. Per the
/// module doc comment's `br_table` safety argument, no frame in `removable`
/// ever contains or is targeted by a `br_table`, so `br_table` instructions
/// are always copied through unchanged (their depths never need adjusting).
fn remove_frames<'a>(ops: &[Operator<'a>], removable: &HashSet<usize>) -> Vec<Operator<'a>> {
    let mut out = Vec::with_capacity(ops.len());
    // Parallel to the currently-open frame stack: `true` at position `k`
    // means the frame at that stack position is being removed.
    let mut skip_stack: Vec<bool> = Vec::new();

    for (i, op) in ops.iter().enumerate() {
        match op {
            Operator::Block { blockty } => {
                let skip = *blockty == BlockType::Empty && removable.contains(&i);
                skip_stack.push(skip);
                if !skip {
                    out.push(op.clone());
                }
            }
            Operator::Loop { .. } | Operator::If { .. } => {
                skip_stack.push(false);
                out.push(op.clone());
            }
            Operator::Else => out.push(op.clone()),
            Operator::End => {
                let skip = skip_stack.pop().unwrap_or(false);
                if !skip {
                    out.push(op.clone());
                }
            }
            Operator::Br { relative_depth } => {
                out.push(Operator::Br {
                    relative_depth: adjust_depth(&skip_stack, *relative_depth),
                });
            }
            Operator::BrIf { relative_depth } => {
                out.push(Operator::BrIf {
                    relative_depth: adjust_depth(&skip_stack, *relative_depth),
                });
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// Computes a branch's new `relative_depth` after removing the frames
/// marked `true` in `skip_stack` (the currently-open frame stack, outer to
/// inner, *before* removal — i.e. exactly as seen by the branch at the point
/// it is encountered). The target frame's stack position is
/// `len - 1 - relative_depth` (or "before everything", conceptually -1, if
/// `relative_depth` reaches past the whole stack — the implicit
/// function-level scope). Every removed frame whose stack position is
/// *deeper* than the target's (i.e. sits between the branch and its target)
/// contributes exactly one level of decrement; a removed frame at or before
/// the target's position leaves the branch's depth to that target
/// unaffected (see the module doc comment for the derivation).
fn adjust_depth(skip_stack: &[bool], relative_depth: u32) -> u32 {
    let len = skip_stack.len();
    let target_pos: i64 = if (relative_depth as usize) < len {
        (len - 1 - relative_depth as usize) as i64
    } else {
        -1
    };
    let decrement = skip_stack
        .iter()
        .enumerate()
        .filter(|&(k, &skip)| skip && (k as i64) > target_pos)
        .count() as u32;
    relative_depth - decrement
}

/// The maximum simultaneous `block`/`loop`/`if` nesting depth in `ops`. Must
/// stay in sync with [`ir::max_nesting_depth`], which computes the same
/// thing over a `wasmparser::FunctionBody` reader rather than an in-memory
/// `Vec<Operator>`.
fn max_depth(ops: &[Operator]) -> u32 {
    let mut depth: u32 = 0;
    let mut max_depth: u32 = 0;
    for op in ops {
        match op {
            Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            Operator::End => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max_depth
}
