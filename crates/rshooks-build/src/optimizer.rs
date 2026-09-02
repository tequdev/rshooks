//! Binaryen `wasm-opt` size optimization, run as the first pipeline step.
//!
//! Must run on the raw per-entry wasm, before [`crate::cleaner::clean`]: the
//! cleaner strips the `memory` export, and `wasm-opt`'s dead-code
//! elimination would then treat the memory/data section as unreachable and
//! remove it — the result passes every static check but traps at runtime.
//! It also must run before [`crate::flatten::flatten`], which requires an
//! import-only type section.
//!
//! Restricted to WebAssembly MVP instructions only: the `wasm-opt` crate's
//! default feature set enables post-MVP instructions xahaud's SetHook
//! validation rejects.

use anyhow::{Context, Result};
use wasm_opt::OptimizationOptions;

/// Runs Binaryen's `-Oz` (aggressive size) optimization over `wasm`,
/// restricted to WebAssembly MVP features, and returns the optimized bytes.
pub fn optimize(wasm: &[u8]) -> Result<Vec<u8>> {
    let dir = tempfile::tempdir().context("creating wasm-opt temp directory")?;
    let input_path = dir.path().join("input.wasm");
    let output_path = dir.path().join("output.wasm");
    std::fs::write(&input_path, wasm).context("writing wasm-opt input file")?;

    OptimizationOptions::new_optimize_for_size_aggressively()
        .mvp_features_only()
        .run(&input_path, &output_path)
        .map_err(|error| anyhow::anyhow!("wasm-opt optimization failed: {error}"))?;

    std::fs::read(&output_path).context("reading wasm-opt output file")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn optimize_produces_a_valid_smaller_or_equal_module() {
        let wasm = wat::parse_str(
            r#"
            (module
              (func $unused (param i32) (result i32) (local.get 0))
              (func $hook (param i32) (result i64)
                (drop (call $unused (i32.const 1)))
                (i64.const 0))
              (memory (export "memory") 1)
              (export "hook" (func $hook)))
            "#,
        )
        .expect("valid fixture");

        let out = optimize(&wasm).expect("optimize succeeds");

        wasmparser::Validator::new()
            .validate_all(&out)
            .expect("optimized output is valid wasm");
        assert!(
            out.len() <= wasm.len(),
            "-Oz should not grow a module with a dead function to eliminate"
        );

        // The `memory` export must survive: downstream cleaning identifies
        // live host imports/exports by name, and this pass must not have
        // treated it as dead-code-eliminable.
        let mut has_memory_export = false;
        for payload in wasmparser::Parser::new(0).parse_all(&out) {
            let wasmparser::Payload::ExportSection(reader) = payload.expect("valid wasm payload")
            else {
                continue;
            };
            for export in reader {
                let export = export.expect("valid export entry");
                if export.name == "memory" {
                    has_memory_export = true;
                }
            }
        }
        assert!(has_memory_export, "memory export must survive optimization");
    }
}
