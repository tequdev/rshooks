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
        sec.ty().func_type(&ir::conv_functype(ty)?);
    }
    Ok(sec)
}

/// Encodes an import section from `imports`, in declaration order. The
/// Table/Memory/Global arms and the Tag/FuncExact hard errors are identical
/// across every pass; `func_entity` decides the Func arm: called with the
/// function import's ordinal (0-based, among function imports only) and its
/// original type index, returning the entity type to import, or `None` to
/// drop this import (the cleaner's GC case). Callers append any extra
/// imports (`_g`) after this returns.
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
                sec.import(imp.module, imp.name, ir::conv_memtype(mt)?);
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
) -> Result<wasm_encoder::MemorySection> {
    let mut sec = wasm_encoder::MemorySection::new();
    for &mem in memories {
        sec.memory(ir::conv_memtype(mem)?);
    }
    Ok(sec)
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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    // -- conv_export_kind -----------------------------------------------------

    #[test]
    fn conv_export_kind_maps_each_mvp_kind() {
        assert_eq!(
            conv_export_kind(wasmparser::ExternalKind::Func).unwrap(),
            wasm_encoder::ExportKind::Func
        );
        assert_eq!(
            conv_export_kind(wasmparser::ExternalKind::Table).unwrap(),
            wasm_encoder::ExportKind::Table
        );
        assert_eq!(
            conv_export_kind(wasmparser::ExternalKind::Memory).unwrap(),
            wasm_encoder::ExportKind::Memory
        );
        assert_eq!(
            conv_export_kind(wasmparser::ExternalKind::Global).unwrap(),
            wasm_encoder::ExportKind::Global
        );
    }

    #[test]
    fn conv_export_kind_rejects_func_exact() {
        assert!(conv_export_kind(wasmparser::ExternalKind::FuncExact).is_err());
    }

    // -- encode_import_section -------------------------------------------------

    #[test]
    fn encode_import_section_func_entity_can_filter_a_function_import() {
        let imports = vec![
            wasmparser::Import {
                module: "env",
                name: "kept",
                ty: wasmparser::TypeRef::Func(0),
            },
            wasmparser::Import {
                module: "env",
                name: "dropped",
                ty: wasmparser::TypeRef::Func(1),
            },
        ];
        // Drop the second function import (ordinal 1); keep the first.
        let sec = encode_import_section(&imports, |ordinal, type_idx| {
            if ordinal == 0 {
                Ok(Some(wasm_encoder::EntityType::Function(type_idx)))
            } else {
                Ok(None)
            }
        })
        .expect("encode succeeds");

        let mut module = wasm_encoder::Module::new();
        module.section(&wasm_encoder::TypeSection::new());
        module.section(&sec);
        let bytes = module.finish();
        let mut names = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            if let wasmparser::Payload::ImportSection(reader) = payload.expect("valid wasm") {
                for imp in reader.into_imports() {
                    names.push(imp.expect("valid import").name.to_string());
                }
            }
        }
        assert_eq!(names, vec!["kept".to_string()]);
    }

    #[test]
    fn encode_import_section_passes_through_table_memory_global() {
        let imports = vec![
            wasmparser::Import {
                module: "env",
                name: "t",
                ty: wasmparser::TypeRef::Table(wasmparser::TableType {
                    element_type: wasmparser::RefType::FUNCREF,
                    table64: false,
                    initial: 1,
                    maximum: None,
                    shared: false,
                }),
            },
            wasmparser::Import {
                module: "env",
                name: "m",
                ty: wasmparser::TypeRef::Memory(wasmparser::MemoryType {
                    memory64: false,
                    shared: false,
                    initial: 1,
                    maximum: None,
                    page_size_log2: None,
                }),
            },
            wasmparser::Import {
                module: "env",
                name: "g",
                ty: wasmparser::TypeRef::Global(wasmparser::GlobalType {
                    content_type: wasmparser::ValType::I32,
                    mutable: false,
                    shared: false,
                }),
            },
        ];
        let sec = encode_import_section(&imports, |_, ty| {
            Ok(Some(wasm_encoder::EntityType::Function(ty)))
        })
        .expect("encode succeeds");

        let mut module = wasm_encoder::Module::new();
        module.section(&wasm_encoder::TypeSection::new());
        module.section(&sec);
        let bytes = module.finish();
        let mut kinds = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            if let wasmparser::Payload::ImportSection(reader) = payload.expect("valid wasm") {
                for imp in reader.into_imports() {
                    let imp = imp.expect("valid import");
                    kinds.push(match imp.ty {
                        wasmparser::TypeRef::Table(_) => "table",
                        wasmparser::TypeRef::Memory(_) => "memory",
                        wasmparser::TypeRef::Global(_) => "global",
                        _ => "other",
                    });
                }
            }
        }
        assert_eq!(kinds, vec!["table", "memory", "global"]);
    }

    #[test]
    fn encode_import_section_rejects_tag_import() {
        let imports = vec![wasmparser::Import {
            module: "env",
            name: "t",
            ty: wasmparser::TypeRef::Tag(wasmparser::TagType {
                kind: wasmparser::TagKind::Exception,
                func_type_idx: 0,
            }),
        }];
        let err = encode_import_section(&imports, |_, ty| {
            Ok(Some(wasm_encoder::EntityType::Function(ty)))
        })
        .unwrap_err();
        assert!(err.to_string().contains("tag"));
    }

    #[test]
    fn encode_import_section_ordinals_advance_past_dropped_and_nonfunc_imports() {
        // The func-import ordinal must advance for every function import —
        // dropped ones included — and must not advance for interspersed
        // non-function imports, so that it always equals the import's
        // function index.
        let imports = vec![
            wasmparser::Import {
                module: "env",
                name: "a",
                ty: wasmparser::TypeRef::Func(0),
            },
            wasmparser::Import {
                module: "env",
                name: "b",
                ty: wasmparser::TypeRef::Func(1),
            },
            wasmparser::Import {
                module: "env",
                name: "g",
                ty: wasmparser::TypeRef::Global(wasmparser::GlobalType {
                    content_type: wasmparser::ValType::I32,
                    mutable: false,
                    shared: false,
                }),
            },
            wasmparser::Import {
                module: "env",
                name: "c",
                ty: wasmparser::TypeRef::Func(2),
            },
        ];
        let mut seen = Vec::new();
        let sec = encode_import_section(&imports, |ordinal, type_idx| {
            seen.push((ordinal, type_idx));
            // Drop `b` (ordinal 1); keep `a` and `c`.
            Ok((ordinal != 1).then_some(wasm_encoder::EntityType::Function(type_idx)))
        })
        .expect("encode succeeds");
        assert_eq!(seen, vec![(0, 0), (1, 1), (2, 2)]);

        let mut module = wasm_encoder::Module::new();
        module.section(&wasm_encoder::TypeSection::new());
        module.section(&sec);
        let bytes = module.finish();
        let mut names = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            if let wasmparser::Payload::ImportSection(reader) = payload.expect("valid wasm") {
                for imp in reader.into_imports() {
                    names.push(imp.expect("valid import").name.to_string());
                }
            }
        }
        assert_eq!(
            names,
            vec!["a".to_string(), "g".to_string(), "c".to_string()]
        );
    }
}
