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
//! `mvp_features_only` option configures an initially empty feature set on
//! the `Module` it constructs, but Binaryen's binary reader then ORs in
//! every feature listed in the input module's `target_features` custom
//! section, overriding that configuration. A `target_features` section
//! (emitted by clang for any non-MVP target CPU, e.g. `+sign-ext` or
//! `+mutable-globals`) therefore re-enables post-MVP instructions for the
//! `-Oz` pass, which can then synthesize a post-MVP opcode (e.g.
//! `i32.extend8_s`) into output whose source had none. Every custom section
//! is stripped before the module reaches `wasm-opt` to close this hole; the
//! cleaner drops all custom sections later anyway, so this loses no
//! information the rest of the pipeline needs.

use anyhow::{Context, Result};
use wasm_opt::OptimizationOptions;

/// Runs Binaryen's `-Oz` (aggressive size) optimization over `wasm`,
/// restricted to WebAssembly MVP features, and returns the optimized bytes.
pub fn optimize(wasm: &[u8]) -> Result<Vec<u8>> {
    let stripped = strip_custom_sections(wasm)?;

    // wasm-opt rejects post-MVP input under the MVP-only feature set with
    // an internal validator dump; surface the cause and the fix instead.
    if let Err(e) = wasmparser::Validator::new_with_features(crate::validator::mvp_features())
        .validate_all(&stripped)
    {
        anyhow::bail!(
            "input uses a WebAssembly feature outside the MVP instruction set ({e}); the \
             wasm-opt pass only accepts MVP modules. Compile without post-MVP features \
             (clang: `-mcpu=mvp`), or pass --no-optimize to skip wasm-opt and let validation \
             report the offending instruction"
        );
    }

    let dir = tempfile::tempdir().context("creating wasm-opt temp directory")?;
    let input_path = dir.path().join("input.wasm");
    let output_path = dir.path().join("output.wasm");
    std::fs::write(&input_path, &stripped).context("writing wasm-opt input file")?;

    OptimizationOptions::new_optimize_for_size_aggressively()
        .mvp_features_only()
        .run(&input_path, &output_path)
        .map_err(|error| anyhow::anyhow!("wasm-opt optimization failed: {error}"))?;

    std::fs::read(&output_path).context("reading wasm-opt output file")
}

/// Re-encodes `wasm` with every custom section (including `target_features`
/// and `producers`) removed, preserving every other section byte-for-byte
/// and in order. See the module doc comment for why this must run before
/// `wasm-opt`.
fn strip_custom_sections(wasm: &[u8]) -> Result<Vec<u8>> {
    let mut module = wasm_encoder::Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let payload = payload.context("parsing wasm module for custom-section stripping")?;
        if matches!(payload, wasmparser::Payload::CustomSection(_)) {
            continue;
        }
        let Some((id, range)) = payload.as_section() else {
            continue;
        };
        let data = wasm
            .get(range)
            .context("section byte range out of bounds while stripping custom sections")?;
        module.section(&wasm_encoder::RawSection { id, data });
    }
    Ok(module.finish())
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

    fn write_leb128(mut n: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }

    /// Appends a raw custom section (id 0) to an existing wasm binary.
    fn append_custom_section(wasm: &[u8], name: &str, payload: &[u8]) -> Vec<u8> {
        let mut content = Vec::new();
        write_leb128(name.len() as u64, &mut content);
        content.extend_from_slice(name.as_bytes());
        content.extend_from_slice(payload);

        let mut out = wasm.to_vec();
        out.push(0x00);
        write_leb128(content.len() as u64, &mut out);
        out.extend_from_slice(&content);
        out
    }

    /// Encodes a `target_features` custom-section payload (the
    /// `wasm-features-section` proposal's format: a feature count followed
    /// by one `(prefix, name)` entry per feature) declaring each of
    /// `features` as required (`+`).
    fn target_features_payload(features: &[&str]) -> Vec<u8> {
        let mut payload = Vec::new();
        write_leb128(features.len() as u64, &mut payload);
        for feature in features {
            payload.push(b'+');
            write_leb128(feature.len() as u64, &mut payload);
            payload.extend_from_slice(feature.as_bytes());
        }
        payload
    }

    /// A `target_features` section declaring `+sign-ext` must not cause
    /// `wasm-opt` to synthesize sign-extension opcodes into a module whose
    /// source contains none, even though `-Oz` is configured
    /// `mvp_features_only`: the Binaryen reader ORs the section's features
    /// into the configured set. This is the shape clang emits for any
    /// non-`mvp` target CPU.
    #[test]
    fn optimize_strips_target_features_section_before_wasm_opt() {
        let base = wat::parse_str(
            r#"
            (module
              (func $hook (param i32) (result i64)
                (i64.extend_i32_s
                  (i32.shr_s
                    (i32.shl (local.get 0) (i32.const 24))
                    (i32.const 24))))
              (memory (export "memory") 1)
              (export "hook" (func $hook)))
            "#,
        )
        .expect("valid fixture");

        let with_target_features = append_custom_section(
            &base,
            "target_features",
            &target_features_payload(&["sign-ext"]),
        );

        let out = optimize(&with_target_features).expect("optimize succeeds");

        for payload in wasmparser::Parser::new(0).parse_all(&out) {
            let payload = payload.expect("valid wasm payload");
            assert!(
                !matches!(payload, wasmparser::Payload::CustomSection(_)),
                "custom sections must not survive optimization"
            );
            let wasmparser::Payload::CodeSectionEntry(body) = payload else {
                continue;
            };
            let mut ops = body.get_operators_reader().expect("operators reader");
            while !ops.eof() {
                let op = ops.read().expect("operator");
                assert!(
                    !matches!(
                        op,
                        wasmparser::Operator::I32Extend8S
                            | wasmparser::Operator::I32Extend16S
                            | wasmparser::Operator::I64Extend8S
                            | wasmparser::Operator::I64Extend16S
                            | wasmparser::Operator::I64Extend32S
                    ),
                    "sign-extension opcode leaked into MVP-targeted output: {op:?}"
                );
            }
        }

        wasmparser::Validator::new_with_features(crate::validator::mvp_features())
            .validate_all(&out)
            .expect("output must validate under the MVP-restricted feature set");
    }

    /// Input that already contains a post-MVP instruction (clang output
    /// built without `-mcpu=mvp`) is rejected before wasm-opt runs, with a
    /// message naming the fix.
    #[test]
    fn optimize_rejects_post_mvp_input_with_actionable_error() {
        let wasm = wat::parse_str(
            r#"
            (module
              (func $hook (param i32) (result i64)
                (i64.extend_i32_s (i32.extend8_s (local.get 0))))
              (memory (export "memory") 1)
              (export "hook" (func $hook)))
            "#,
        )
        .expect("valid fixture");

        let err = optimize(&wasm).expect_err("post-MVP input must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("-mcpu=mvp"),
            "message names the clang fix: {msg}"
        );
        assert!(
            msg.contains("--no-optimize"),
            "message names the flag: {msg}"
        );
    }
}
