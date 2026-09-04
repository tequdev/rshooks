# The `rshooks` CLI

`rshooks` is a single binary with three subcommands: `build` (the
one you'll use for everyday work), `clean` (post-process an
already-compiled wasm without invoking cargo), and `check` (validate any
wasm file against the full SetHook rule set, without modifying it). This
page is the complete flag reference for all three, taken directly from the
CLI's own definitions.

Every subcommand also accepts the standard clap-generated `-h`/`--help`;
`rshooks --version` prints the installed version.

## `rshooks build`

Builds a `#[hooks]` crate for `wasm32v1-none`: one discovery build to read
its declarations, then one `cargo rustc` build per declared index, each
cleaned and validated into its own SetHook-legal binary. This is the
pipeline described in [Building a Hook](../getting-started/building.md) and
[Hook Chains](../concepts/chains.md).

```sh
rshooks build --manifest-path path/to/Cargo.toml
```

| flag | default | description |
|---|---|---|
| `--manifest-path <PATH>` | cargo's default (current directory) | Path to the crate's `Cargo.toml`, forwarded to every cargo invocation. |
| `-p, --package <NAME>` | none | Build only the named package, forwarded to cargo's `-p`. Useful when `--manifest-path` points at a workspace. |
| `--api-version <0\|1>` | `0` | The Hook API version this module targets. `0` is Guard-type (loop guards required, and the only version `#[hooks]` chains currently support); `1` is Gas-type (guard handling skipped) and is rejected for a chain build. |
| `--auto-guard` | off | **Deprecated**, scheduled for removal. Insert missing loop guards instead of treating an unguarded loop as a build error. Applies per index. See [Guards and Loops](../concepts/guards.md) for the source-level alternatives. |
| `--default-maxiter <N>` | `16` | **Deprecated**, scheduled for removal. The `maxiter` value used for auto-inserted guards, when `--auto-guard` is set. See [Guards and Loops](../concepts/guards.md) for the source-level alternatives. |
| `--out <DIR>` | `target/rshooks/<crate-name>` under the workspace's target directory | Output **root**: generation directories (`gen-<N>/`) are written under it, with `current` symlinked to the latest complete, validated one. |
| `--allow-oversize` | off | Write each index's output even if it exceeds the 65,535-byte SetHook size limit. The result is still clearly marked invalid in the printed report. |
| `--no-optimize` | off | Skip the Binaryen `wasm-opt` `-Oz` size-optimization pass that otherwise runs on each entry's raw wasm before cleaning. |
| `--account <r...>` | none | Fill the generated template's `Account` placeholder with this address. |
| `--namespace <64hex>` | none | Fill the generated template's `HookNamespace` placeholder(s) with this value. |
| `--override` | off | Add `hsfOVERRIDE` (`Flags: 1`) to every declared (non-gap) entry in the generated template, permitting replacement of an already-installed Hook at that position. Never applied to gap (`{"Hook": {}}`) entries. |

On success, `build` writes, under `<out-root>/current/`: one
`<index>.<fn>.wasm` and one `<index>.<fn>.metadata.json` per declared entry,
plus `sethook.template.json` and `sethook.template.meta.json` covering the
whole chain — see [Per-Hook Attributes](metadata.md) for the sidecar and
template's exact shape.

## `rshooks clean`

Runs the same post-processing pipeline as `build` — the Binaryen `wasm-opt`
`-Oz` pass, the cleaner (drops custom sections and any export other than
`hook`/`cbak`, then garbage-collects), flatten (inlines every defined
helper function into `hook`/`cbak`), unnest, and the authoritative guard
check — on one already-compiled wasm file from **any** toolchain, without
invoking cargo. Useful for post-processing a single artifact you already
have on disk — for example one index's raw build output from a different
pipeline, or one you want to reprocess with different flags without
rebuilding.

This makes `clean` usable as a post-processor for Hooks written in C and
compiled with clang: C authors can write ordinary (non-inline) helper
functions, and loops with `GUARD` inside them, exactly as they would in any
other C program, and `clean` inlines those helpers into `hook`/`cbak`, so
the type section reduces to the import types plus the entry-point type, as
SetHook requires.

```sh
rshooks clean path/to/artifact.wasm
```

```sh
clang --target=wasm32 -mcpu=mvp -nostdlib -O2 \
  -Wl,--no-entry -Wl,--allow-undefined -Wl,--export=hook -Wl,--export=cbak \
  -o hook.raw.wasm hook.c
rshooks clean hook.raw.wasm -o hook.wasm
```

`-mcpu=mvp` keeps clang from emitting post-MVP instructions (such as
sign-extension ops) in the first place. Without it, `clean` stops before
the `wasm-opt` pass with an error naming the flag, because that pass only
accepts modules within the WebAssembly MVP instruction set; with
`--no-optimize` (or under `check`) such a module may still pass the
authoritative upstream guard checker, but a divergence warning is printed,
since the Rust validator enforces the MVP instruction set. Compile with `-O2` or higher: at `-O0`/`-O1` clang does
not keep the `_g` call as the first instruction of every loop, so the raw
output only passes the guard check when the `wasm-opt` pass is left on.

A helper containing a guarded loop is duplicated at each call site while
keeping one guard id, so size its `maxiter` for the total across all call
sites. See [Guards and Loops](../concepts/guards.md).

| flag | default | description |
|---|---|---|
| `input` (positional) | — | The input wasm file. Required. |
| `-o, --out <PATH>` | `<input>.clean.wasm` | Where to write the cleaned binary. |
| `--api-version <0\|1>` | `0` | The Hook API version this module targets. |
| `--auto-guard` | off | **Deprecated**, scheduled for removal. Insert missing loop guards instead of treating them as an error. See [Guards and Loops](../concepts/guards.md) for the source-level alternatives. |
| `--default-maxiter <N>` | `16` | **Deprecated**, scheduled for removal. `maxiter` used for auto-inserted guards. See [Guards and Loops](../concepts/guards.md) for the source-level alternatives. |
| `--allow-oversize` | off | Write the output even if it exceeds the 65,535-byte SetHook limit. |
| `--no-optimize` | off | Skip the Binaryen `wasm-opt` `-Oz` size-optimization pass that otherwise runs on the raw wasm before cleaning. |

`clean` does not generate a metadata sidecar or a `SetHook` template — those
steps are specific to `build`, since they need the original crate's
`#[hooks]` carriers from cargo's raw discovery artifact, and `clean`
operates on a single already-processed wasm file with no such carrier left
in it.

## `rshooks check`

Validates a wasm file against the full SetHook rule set without modifying
it. Unlike `build`/`clean`, this works on **any** wasm file, including
ones not built by this toolchain at all — for example, a Hook compiled
from C.

```sh
rshooks check path/to/hook.wasm
```

| flag | default | description |
|---|---|---|
| `file` (positional) | — | The wasm file to validate. Required. |
| `--api-version <0\|1>` | `0` | The Hook API version this module targets. |

On success, `check` prints the same worst-case-instruction and
nesting-depth report as `build`/`clean`, followed by `OK: <file> is a
valid SetHook wasm binary` and the size/fee estimate. On failure, it
prints `INVALID: <file> failed validation:` with the specific reasons, and
exits with a non-zero status — making it suitable for a CI gate on hand-
written or third-party wasm as well as this toolchain's own output.
