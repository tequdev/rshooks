//! Shared section re-encoders used by the cleaner, flatten, unnest, and
//! guard passes. Each of those passes parses a module into an
//! `ir::ParsedModule`, transforms some subset of it, and re-encodes a full
//! module; several sections are emitted identically (or near-identically)
//! by every pass, so the `wasm-encoder` call sequence for each is written
//! once here rather than four times.

use anyhow::{Result, bail};

use crate::ir::{self, IndexRemapper};

/// Encodes a type section from `types`, in order.
pub(crate) fn encode_type_section(
    types: &[wasmparser::FuncType],
) -> Result<wasm_encoder::TypeSection> {
    let mut sec = wasm_encoder::TypeSection::new();
    for ty in types {
        let (params, results) = ir::conv_functype(ty)?;
        sec.ty().function(params, results);
    }
    Ok(sec)
}

/// Encodes an import section from `imports`, in declaration order. The
/// Table/Memory/Global arms, and the Tag/FuncExact hard errors, are
/// identical across every pass; `func_entity` decides the Func arm: it is
/// called with the function import's ordinal (0-based, among function
/// imports only) and its original type index, and returns the entity type
/// to import, or `None` to drop this import entirely (the cleaner's GC
/// case). Callers append any extra imports (`_g`) after this returns.
pub(crate) fn encode_import_section(
    imports: &[wasmparser::Import],
    mut func_entity: impl FnMut(u32, u32) -> Result<Option<wasm_encoder::EntityType>>,
) -> Result<wasm_encoder::ImportSection> {
    let mut sec = wasm_encoder::ImportSection::new();
    let mut func_ordinal = 0u32;
    for imp in imports {
        match imp.ty {
            wasmparser::TypeRef::Func(type_idx) => {
                let ordinal = func_ordinal;
                func_ordinal += 1;
                if let Some(entity) = func_entity(ordinal, type_idx)? {
                    sec.import(imp.module, imp.name, entity);
                }
            }
            wasmparser::TypeRef::Table(t) => {
                sec.import(imp.module, imp.name, ir::conv_tabletype(t)?);
            }
            wasmparser::TypeRef::Memory(mt) => {
                sec.import(imp.module, imp.name, ir::conv_memtype(mt));
            }
            wasmparser::TypeRef::Global(gt) => {
                sec.import(imp.module, imp.name, ir::conv_globaltype(gt)?);
            }
            wasmparser::TypeRef::Tag(_) => {
                bail!("unsupported import: tag imports are not supported");
            }
            wasmparser::TypeRef::FuncExact(_) => {
                bail!("unsupported import: exact function-reference imports are not supported");
            }
        }
    }
    Ok(sec)
}

/// Encodes a memory section from `memories`, in order.
pub(crate) fn encode_memory_section(
    memories: &[wasmparser::MemoryType],
) -> wasm_encoder::MemorySection {
    let mut sec = wasm_encoder::MemorySection::new();
    for &mem in memories {
        sec.memory(ir::conv_memtype(mem));
    }
    sec
}

/// Encodes a global section from `globals`, in order, remapping each
/// initializer expression through `remapper`. This is the plain
/// pass-through used by flatten/unnest/guard, none of which drop or
/// reorder globals; the cleaner's global-section emission filters by GC
/// reachability first and stays a custom loop.
pub(crate) fn encode_global_section(
    globals: &[wasmparser::Global],
    remapper: &mut IndexRemapper,
) -> Result<wasm_encoder::GlobalSection> {
    let mut sec = wasm_encoder::GlobalSection::new();
    for g in globals {
        let ty = ir::conv_globaltype(g.ty)?;
        let expr = ir::remap_const_expr(&g.init_expr, remapper)?;
        sec.global(ty, &expr);
    }
    Ok(sec)
}

/// Encodes a data section from `datas`, in order, remapping each active
/// segment's offset expression through `remapper`. This is the plain
/// pass-through used by flatten/unnest/guard; the cleaner's trailing-zero
/// trim logic needs per-segment decisions the shared loop doesn't make, so
/// it stays a custom loop.
pub(crate) fn encode_data_section(
    datas: &[wasmparser::Data],
    remapper: &mut IndexRemapper,
) -> Result<wasm_encoder::DataSection> {
    let mut sec = wasm_encoder::DataSection::new();
    for d in datas {
        match &d.kind {
            wasmparser::DataKind::Active {
                memory_index,
                offset_expr,
            } => {
                let expr = ir::remap_const_expr(offset_expr, remapper)?;
                sec.active(*memory_index, &expr, d.data.iter().copied());
            }
            wasmparser::DataKind::Passive => {
                sec.passive(d.data.iter().copied());
            }
        }
    }
    Ok(sec)
}

/// Converts a `wasmparser` export kind to the `wasm-encoder` equivalent.
/// `FuncExact` (exact function-reference exports) is rejected: it is
/// outside the WASM MVP.
pub(crate) fn conv_export_kind(kind: wasmparser::ExternalKind) -> Result<wasm_encoder::ExportKind> {
    Ok(match kind {
        wasmparser::ExternalKind::Func => wasm_encoder::ExportKind::Func,
        wasmparser::ExternalKind::Table => wasm_encoder::ExportKind::Table,
        wasmparser::ExternalKind::Memory => wasm_encoder::ExportKind::Memory,
        wasmparser::ExternalKind::Global => wasm_encoder::ExportKind::Global,
        wasmparser::ExternalKind::Tag => wasm_encoder::ExportKind::Tag,
        wasmparser::ExternalKind::FuncExact => {
            bail!("unsupported export: exact function-reference exports are not supported")
        }
    })
}
