# Examples Index

`examples/` is a runnable catalog of Hooks written with `rshooks`, built
with `rshooks` (from the `rshooks-build` package) — its own Cargo workspace, separate from the root
workspace, because these crates are `no_std` `cdylib`s with a Hook-specific
release profile that must not leak into `rshooks-core`/`rshooks`/
`rshooks-build`, and they don't build for host targets. Every code sample in
this book is adapted from one of these.

## Reading order: 01–21

Numbered in **suggested reading order** — start at `01_accept-all` and work
down; each one builds on ideas from the examples before it. The `example`
column is each crate's actual package name (Cargo package names can't start
with a digit, so only the directory is prefixed).

{{#include ../../../examples/README.md:examples-table}}

There is no `11` in the numbering — the numbering follows the historical
example order, with gaps where an example was retired.

## 80+: production hooks in Rust

Unlike `01`–`21` (one concept each, in suggested reading order), the `80`+
series are behavior-equivalent Rust ports of real, deployed xahaud C hooks —
read them after `01`–`21`, not instead of them. Each has its own README with
a behavior-equivalence note against its C source.

{{#include ../../../examples/README.md:examples-table-80}}

`80_governance` is also this book's worked example for the multi-Hook
chain model's real nesting-budget limit and its raw-API escape hatch — see
[Hook Chains](../concepts/chains.md#a-real-limit-typed-accessor-density-inside-one-entry)
and the example's own `README.md`.

## Building

Build every example (this is also the toolchain's own end-to-end test: each
one is built via `cargo run -p rshooks-build -- build ...` from the root
workspace, and the resulting `out/<name>.wasm` is re-validated with
`rshooks check`):

```sh
mise run build-examples
```

Build a single example directly:

```sh
cargo run -p rshooks-build -- build --manifest-path examples/02_state-counter/Cargo.toml
```

See [The rshooks CLI](../build/cli.md) for the CLI itself, and each
example's own README for its exact command.

## E2E tests

`e2e/` deploys the examples' `rshooks-build` output to a real, standalone
`xahaud` node (via `SetHook`) and asserts on the resulting transaction
metadata and ledger state — proof of runtime behavior, not just that the
binaries are SetHook-valid.
