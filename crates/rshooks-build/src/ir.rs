//! A small, purpose-built intermediate representation for a parsed WebAssembly
//! module, plus the handful of `wasmparser` -> `wasm-encoder` conversion
//! helpers shared by the cleaner and guard passes.
//!
//! This is *not* a general-purpose wasm IR: it only models what a SetHook
//! module can legally contain (see `docs/DESIGN.md` §2, §6), and it leans on
//! [`wasm_encoder::reencode::Reencode`] to translate instruction streams
//! rather than hand-rolling a match over every WebAssembly opcode.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use wasm_encoder::reencode::{self, Reencode};

/// A parsed module, with each section's items collected into a `Vec` for
/// random access. Borrows names and byte slices from the original input.
pub(crate) struct ParsedModule<'a> {
    pub types: Vec<wasmparser::FuncType>,
    pub imports: Vec<wasmparser::Import<'a>>,
    /// Type index for each *defined* function, in `Function` section order.
    pub defined_func_types: Vec<u32>,
    pub tables: Vec<wasmparser::TableType>,
    pub memories: Vec<wasmparser::MemoryType>,
    /// Defined globals (the global section never contains imported globals;
    /// those live in `imports`).
    pub globals: Vec<wasmparser::Global<'a>>,
    pub exports: Vec<wasmparser::Export<'a>>,
    pub start: Option<u32>,
    pub elements: Vec<wasmparser::Element<'a>>,
    pub data_count: Option<u32>,
    pub datas: Vec<wasmparser::Data<'a>>,
    /// One entry per defined function, parallel to `defined_func_types`.
    pub code: Vec<wasmparser::FunctionBody<'a>>,
    pub had_custom_section: bool,
}

impl<'a> ParsedModule<'a> {
    /// Number of imported functions (the low end of the function index
    /// space).
    pub fn num_imported_funcs(&self) -> u32 {
        self.imports
            .iter()
            .filter(|i| matches!(i.ty, wasmparser::TypeRef::Func(_)))
            .count() as u32
    }

    /// Number of imported globals (the low end of the global index space).
    pub fn num_imported_globals(&self) -> u32 {
        self.imports
            .iter()
            .filter(|i| matches!(i.ty, wasmparser::TypeRef::Global(_)))
            .count() as u32
    }

    /// Total number of functions (imported + defined).
    pub fn total_funcs(&self) -> u32 {
        self.num_imported_funcs() + self.defined_func_types.len() as u32
    }

    /// Total number of globals (imported + defined).
    pub fn total_globals(&self) -> u32 {
        self.num_imported_globals() + self.globals.len() as u32
    }

    /// Returns the type index of function `idx` (import or defined), if it
    /// exists.
    pub fn func_type_index(&self, idx: u32) -> Option<u32> {
        let n_imports = self.num_imported_funcs();
        if idx < n_imports {
            let mut seen = 0u32;
            for imp in &self.imports {
                if let wasmparser::TypeRef::Func(ty) = imp.ty {
                    if seen == idx {
                        return Some(ty);
                    }
                    seen += 1;
                }
            }
            None
        } else {
            self.defined_func_types
                .get((idx - n_imports) as usize)
                .copied()
        }
    }

    /// Returns the function body for defined function `idx`, if `idx` refers
    /// to a defined (non-imported) function.
    pub fn defined_body(&self, idx: u32) -> Option<&wasmparser::FunctionBody<'a>> {
        let n_imports = self.num_imported_funcs();
        if idx < n_imports {
            None
        } else {
            self.code.get((idx - n_imports) as usize)
        }
    }

    /// Finds an import by module+name, returning its function index if it is
    /// a function import.
    pub fn find_func_import(&self, module: &str, name: &str) -> Option<u32> {
        let mut idx = 0u32;
        for imp in &self.imports {
            if let wasmparser::TypeRef::Func(_) = imp.ty {
                if imp.module == module && imp.name == name {
                    return Some(idx);
                }
                idx += 1;
            }
        }
        None
    }
}

/// Parses `wasm` into a [`ParsedModule`]. This only accepts the section
/// shapes a SetHook-legal module (or a plausible near-miss fixture) can
/// contain; anything from the GC, component-model, or exception-handling
/// proposals is rejected with a descriptive error rather than silently
/// mis-handled.
pub(crate) fn parse(wasm: &[u8]) -> Result<ParsedModule<'_>> {
    let mut m = ParsedModule {
        types: Vec::new(),
        imports: Vec::new(),
        defined_func_types: Vec::new(),
        tables: Vec::new(),
        memories: Vec::new(),
        globals: Vec::new(),
        exports: Vec::new(),
        start: None,
        elements: Vec::new(),
        data_count: None,
        datas: Vec::new(),
        code: Vec::new(),
        had_custom_section: false,
    };

    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let payload = payload.context("failed to parse wasm module")?;
        match payload {
            wasmparser::Payload::Version { .. } => {}
            wasmparser::Payload::TypeSection(reader) => {
                for rec_group in reader {
                    let rec_group = rec_group.context("type section")?;
                    if rec_group.is_explicit_rec_group() {
                        bail!(
                            "unsupported type section: explicit recursion groups (GC proposal) are not supported"
                        );
                    }
                    for sub in rec_group.into_types() {
                        if !sub.is_final || sub.supertype_idx.is_some() {
                            bail!("unsupported type: GC subtyping is not supported");
                        }
                        match sub.composite_type.inner {
                            wasmparser::CompositeInnerType::Func(ft) => m.types.push(ft),
                            _ => bail!("unsupported type: only plain function types are supported"),
                        }
                    }
                }
            }
            wasmparser::Payload::ImportSection(reader) => {
                for imp in reader.into_imports() {
                    m.imports.push(imp.context("import section")?);
                }
            }
            wasmparser::Payload::FunctionSection(reader) => {
                for f in reader {
                    m.defined_func_types.push(f.context("function section")?);
                }
            }
            wasmparser::Payload::TableSection(reader) => {
                for t in reader {
                    m.tables.push(t.context("table section")?.ty);
                }
            }
            wasmparser::Payload::MemorySection(reader) => {
                for mem in reader {
                    m.memories.push(mem.context("memory section")?);
                }
            }
            wasmparser::Payload::TagSection(_) => {
                bail!("unsupported section: tags (exception-handling proposal) are not supported");
            }
            wasmparser::Payload::GlobalSection(reader) => {
                for g in reader {
                    m.globals.push(g.context("global section")?);
                }
            }
            wasmparser::Payload::ExportSection(reader) => {
                for e in reader {
                    m.exports.push(e.context("export section")?);
                }
            }
            wasmparser::Payload::StartSection { func, .. } => {
                m.start = Some(func);
            }
            wasmparser::Payload::ElementSection(reader) => {
                for e in reader {
                    m.elements.push(e.context("element section")?);
                }
            }
            wasmparser::Payload::DataCountSection { count, .. } => {
                m.data_count = Some(count);
            }
            wasmparser::Payload::DataSection(reader) => {
                for d in reader {
                    m.datas.push(d.context("data section")?);
                }
            }
            wasmparser::Payload::CodeSectionStart { .. } => {}
            wasmparser::Payload::CodeSectionEntry(body) => {
                m.code.push(body);
            }
            wasmparser::Payload::CustomSection(_) => {
                m.had_custom_section = true;
            }
            wasmparser::Payload::End(_) => {}
            other => {
                bail!(
                    "unsupported module: contains a component-model or unrecognized section ({other:?})"
                );
            }
        }
    }

    Ok(m)
}

/// Converts a `wasmparser` value type to the `wasm-encoder` equivalent.
/// Reference types and `v128` are rejected: they are outside the WASM MVP
/// (and thus outside what a SetHook-legal module may use).
pub(crate) fn conv_valtype(v: wasmparser::ValType) -> Result<wasm_encoder::ValType> {
    match v {
        wasmparser::ValType::I32 => Ok(wasm_encoder::ValType::I32),
        wasmparser::ValType::I64 => Ok(wasm_encoder::ValType::I64),
        wasmparser::ValType::F32 => Ok(wasm_encoder::ValType::F32),
        wasmparser::ValType::F64 => Ok(wasm_encoder::ValType::F64),
        wasmparser::ValType::V128 => bail!("unsupported value type: v128 (SIMD is not MVP)"),
        wasmparser::ValType::Ref(_) => {
            bail!("unsupported value type: reference type (not MVP)")
        }
    }
}

/// Converts a `wasmparser::FuncType` into `wasm-encoder` param/result vectors.
pub(crate) fn conv_functype(
    ft: &wasmparser::FuncType,
) -> Result<(Vec<wasm_encoder::ValType>, Vec<wasm_encoder::ValType>)> {
    let params = ft
        .params()
        .iter()
        .copied()
        .map(conv_valtype)
        .collect::<Result<Vec<_>>>()?;
    let results = ft
        .results()
        .iter()
        .copied()
        .map(conv_valtype)
        .collect::<Result<Vec<_>>>()?;
    Ok((params, results))
}

pub(crate) fn conv_memtype(m: wasmparser::MemoryType) -> wasm_encoder::MemoryType {
    wasm_encoder::MemoryType {
        minimum: m.initial,
        maximum: m.maximum,
        memory64: m.memory64,
        shared: m.shared,
        page_size_log2: m.page_size_log2,
    }
}

pub(crate) fn conv_globaltype(g: wasmparser::GlobalType) -> Result<wasm_encoder::GlobalType> {
    Ok(wasm_encoder::GlobalType {
        val_type: conv_valtype(g.content_type)?,
        mutable: g.mutable,
        shared: g.shared,
    })
}

/// Converts a `wasmparser::TableType` into a `wasm-encoder` entity type.
/// Only `funcref`/`externref` element types are supported (anything else is
/// outside the WASM MVP).
pub(crate) fn conv_tabletype(t: wasmparser::TableType) -> Result<wasm_encoder::EntityType> {
    let element_type = match t.element_type.heap_type() {
        wasmparser::HeapType::Abstract {
            shared: false,
            ty: wasmparser::AbstractHeapType::Func,
        } => wasm_encoder::RefType::FUNCREF,
        wasmparser::HeapType::Abstract {
            shared: false,
            ty: wasmparser::AbstractHeapType::Extern,
        } => wasm_encoder::RefType::EXTERNREF,
        _ => bail!("unsupported table element type (only funcref/externref are supported)"),
    };
    Ok(wasm_encoder::EntityType::Table(wasm_encoder::TableType {
        element_type,
        table64: t.table64,
        minimum: t.initial,
        maximum: t.maximum,
        shared: t.shared,
    }))
}

/// References collected from a single function body: which functions it
/// calls directly, which globals it touches, and whether it uses
/// `call_indirect` (which never contributes a reachability edge, since its
/// target is dynamic).
pub(crate) struct BodyRefs {
    pub calls: BTreeSet<u32>,
    pub globals: BTreeSet<u32>,
    pub has_call_indirect: bool,
}

/// Scans a function body for direct `call` targets and `global.get`/
/// `global.set` targets.
pub(crate) fn scan_refs(body: &wasmparser::FunctionBody) -> Result<BodyRefs> {
    let mut refs = BodyRefs {
        calls: BTreeSet::new(),
        globals: BTreeSet::new(),
        has_call_indirect: false,
    };
    let mut reader = body.get_operators_reader().context("function body")?;
    while !reader.eof() {
        match reader.read().context("function body operator")? {
            wasmparser::Operator::Call { function_index } => {
                refs.calls.insert(function_index);
            }
            wasmparser::Operator::CallIndirect { .. } => {
                refs.has_call_indirect = true;
            }
            wasmparser::Operator::GlobalGet { global_index }
            | wasmparser::Operator::GlobalSet { global_index } => {
                refs.globals.insert(global_index);
            }
            _ => {}
        }
    }
    Ok(refs)
}

/// Scans a const expression (global initializer or data-segment offset) for
/// a `global.get` reference.
pub(crate) fn const_expr_global_ref(expr: &wasmparser::ConstExpr) -> Result<Option<u32>> {
    let mut reader = expr.get_operators_reader();
    let op = reader.read().context("const expr")?;
    if let wasmparser::Operator::GlobalGet { global_index } = op {
        Ok(Some(global_index))
    } else {
        Ok(None)
    }
}

/// A [`Reencode`] implementation that remaps function and global indices
/// through arbitrary caller-supplied closures, leaving every other index
/// space (types, memories, tables) as an identity mapping. Used by both the
/// cleaner (arbitrary GC-driven remap, via a lookup table) and the guard
/// pass (a uniform "+1 if inserting `_g`" shift).
pub(crate) struct IndexRemapper<'f> {
    func: Box<dyn FnMut(u32) -> u32 + 'f>,
    global: Box<dyn FnMut(u32) -> u32 + 'f>,
}

impl<'f> IndexRemapper<'f> {
    pub fn new(func: impl FnMut(u32) -> u32 + 'f, global: impl FnMut(u32) -> u32 + 'f) -> Self {
        Self {
            func: Box::new(func),
            global: Box::new(global),
        }
    }
}

impl Reencode for IndexRemapper<'_> {
    type Error = core::convert::Infallible;

    fn function_index(&mut self, func: u32) -> Result<u32, reencode::Error<Self::Error>> {
        Ok((self.func)(func))
    }

    fn global_index(&mut self, global: u32) -> Result<u32, reencode::Error<Self::Error>> {
        Ok((self.global)(global))
    }
}

/// Rebuilds a function body via the given remapper, translating every
/// instruction. This is used whenever a raw byte-copy is unsafe (some
/// `call`/`global.get`/`global.set` target changed) or when instructions
/// must be inserted (guard auto-insertion).
///
/// `on_operator` is invoked after each *translated* instruction has been
/// pushed, given the original (pre-translation) operator and its byte
/// offset within the body; it may push additional instructions (used to
/// splice in an auto-inserted guard immediately after a `loop`).
pub(crate) fn rebuild_function_body(
    body: &wasmparser::FunctionBody,
    remapper: &mut IndexRemapper,
    mut on_operator: impl FnMut(&wasmparser::Operator, usize, &mut wasm_encoder::Function) -> Result<()>,
) -> Result<wasm_encoder::Function> {
    let locals_reader = body.get_locals_reader().context("function locals")?;
    let mut locals = Vec::new();
    for l in locals_reader {
        let (count, ty) = l.context("function locals")?;
        locals.push((count, conv_valtype(ty)?));
    }
    let mut func = wasm_encoder::Function::new(locals);

    let mut reader = body.get_operators_reader().context("function body")?;
    while !reader.eof() {
        let (op, offset) = reader
            .read_with_offset()
            .context("function body operator")?;
        if matches!(op, wasmparser::Operator::CallIndirect { .. }) {
            bail!("call_indirect is not supported (function offset {offset})");
        }
        let translated = remapper
            .instruction(op.clone())
            .map_err(|e| anyhow::anyhow!("failed to translate instruction: {e}"))?;
        func.instruction(&translated);
        on_operator(&op, offset, &mut func)?;
    }
    Ok(func)
}

/// Returns `true` if raw-copying this function body's original bytes would
/// be unsound: i.e. some `call`, `global.get`, or `global.set` target it
/// contains would map to a different index under `func_map`/`global_map`.
pub(crate) fn body_needs_remap(
    body: &wasmparser::FunctionBody,
    func_map: &impl Fn(u32) -> u32,
    global_map: &impl Fn(u32) -> u32,
) -> Result<bool> {
    let refs = scan_refs(body)?;
    if refs.calls.iter().any(|&f| func_map(f) != f) {
        return Ok(true);
    }
    if refs.globals.iter().any(|&g| global_map(g) != g) {
        return Ok(true);
    }
    Ok(false)
}

/// Remaps a const expression (global initializer / data-segment offset)
/// through the given remapper.
pub(crate) fn remap_const_expr(
    expr: &wasmparser::ConstExpr,
    remapper: &mut IndexRemapper,
) -> Result<wasm_encoder::ConstExpr> {
    remapper
        .const_expr(expr.clone())
        .map_err(|e| anyhow::anyhow!("failed to translate const expr: {e}"))
}

/// The direct-call out-edges of function `idx` (imports are leaves with no
/// out-edges; a malformed/out-of-range `idx` also has none — the generic
/// wasmparser validator separately rejects genuinely malformed modules).
/// Shared by [`find_call_cycle`] (recursion is a hard error, checked both
/// pre- and post-flatten) and the flatten pass's own call-graph traversal.
pub(crate) fn out_edges(m: &ParsedModule, idx: u32) -> Vec<u32> {
    match m.defined_body(idx) {
        Some(body) => match scan_refs(body) {
            // `BodyRefs::calls` is already a `BTreeSet`; collecting it
            // directly (rather than routing through a `HashSet`) keeps
            // edge order deterministic (sorted ascending).
            Ok(refs) => refs.calls.into_iter().collect(),
            Err(_) => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// DFS-based cycle detection over the direct-call graph (imports are leaves
/// with no out-edges). Returns one cycle (as a `Vec` of function indices) if
/// any exists. Used by the validator (recursion is a hard error) and by the
/// flatten pass (its inlining algorithm requires the call graph to be a DAG,
/// checked *before* flattening — see `docs/DESIGN.md` §6.2b).
pub(crate) fn find_call_cycle(m: &ParsedModule) -> Option<Vec<u32>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let total = m.total_funcs();
    let mut color = vec![Color::White; total as usize];
    let mut path: Vec<u32> = Vec::new();

    let get = |color: &[Color], idx: u32| color.get(idx as usize).copied().unwrap_or(Color::Black);
    let set = |color: &mut [Color], idx: u32, c: Color| {
        if let Some(slot) = color.get_mut(idx as usize) {
            *slot = c;
        }
    };

    // Iterative DFS to avoid unbounded recursion on adversarial input.
    for start in 0..total {
        if get(&color, start) != Color::White {
            continue;
        }
        let mut stack: Vec<(u32, Vec<u32>)> = vec![(start, out_edges(m, start))];
        set(&mut color, start, Color::Gray);
        path.push(start);
        while let Some((node, edges)) = stack.last_mut() {
            let node = *node;
            if let Some(next) = edges.pop() {
                match get(&color, next) {
                    Color::White => {
                        set(&mut color, next, Color::Gray);
                        path.push(next);
                        let next_edges = out_edges(m, next);
                        stack.push((next, next_edges));
                    }
                    Color::Gray => {
                        let cycle_start = path.iter().position(|&p| p == next).unwrap_or(0);
                        let mut cycle = path.get(cycle_start..).unwrap_or(&[]).to_vec();
                        cycle.push(next);
                        return Some(cycle);
                    }
                    Color::Black => {}
                }
            } else {
                set(&mut color, node, Color::Black);
                path.pop();
                stack.pop();
            }
        }
    }
    None
}

/// Tracks `block`/`loop`/`if` nesting depth across a stream of operators fed
/// one at a time via [`DepthTracker::step`]. This is the one place that
/// counts nesting depth the way the vendored upstream checker does
/// (`Guard.h` `NESTING_LIMIT` / `GuardRuleDepth32` — see `docs/DESIGN.md`
/// §6.2c/§6.4): depth starts at 0 at the function's top level and
/// increments on every `block`/`loop`/`if` entered, decrementing on the
/// matching `end` (`else` does not change depth — it stays inside the same
/// `if` frame). [`max_nesting_depth`] drives it from a `FunctionBody`
/// reader; the unnest pass drives it from an in-memory `Vec<Operator>` via
/// [`DepthTracker::of_slice`].
#[derive(Default)]
pub(crate) struct DepthTracker {
    depth: u32,
    max: u32,
}

impl DepthTracker {
    pub fn step(&mut self, op: &wasmparser::Operator) {
        match op {
            wasmparser::Operator::Block { .. }
            | wasmparser::Operator::Loop { .. }
            | wasmparser::Operator::If { .. } => {
                self.depth += 1;
                self.max = self.max.max(self.depth);
            }
            wasmparser::Operator::End => self.depth = self.depth.saturating_sub(1),
            _ => {}
        }
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    /// Feeds an entire operator slice through [`step`](Self::step), in
    /// order, returning the resulting maximum depth.
    pub fn of_slice(ops: &[wasmparser::Operator]) -> u32 {
        let mut tracker = Self::default();
        for op in ops {
            tracker.step(op);
        }
        tracker.max()
    }
}

/// Computes the maximum simultaneous `block`/`loop`/`if` nesting depth
/// reached in a function body (0 if it contains no such construct at all).
/// Shared by the unnest pass (which reports before/after depth per function
/// it rewrites) and the validator (which enforces the hard limit) — see
/// [`DepthTracker`].
pub(crate) fn max_nesting_depth(body: &wasmparser::FunctionBody) -> Result<u32> {
    let mut tracker = DepthTracker::default();
    let mut reader = body.get_operators_reader().context("function body")?;
    while !reader.eof() {
        tracker.step(&reader.read().context("function body operator")?);
    }
    Ok(tracker.max())
}

/// Finds a function type matching the given params/results exactly, or
/// appends a new one. Returns its type index. Type indices are otherwise
/// never remapped or reordered (only ever appended to), so this is the only
/// way the cleaner/guard passes touch the type space.
pub(crate) fn find_or_insert_type(
    types: &mut Vec<wasmparser::FuncType>,
    params: &[wasmparser::ValType],
    results: &[wasmparser::ValType],
) -> u32 {
    for (i, ty) in types.iter().enumerate() {
        if ty.params() == params && ty.results() == results {
            return i as u32;
        }
    }
    let idx = types.len() as u32;
    types.push(wasmparser::FuncType::new(
        params.iter().copied(),
        results.iter().copied(),
    ));
    idx
}
