# Introduction

`rshooks` is a Rust toolchain for writing [Xahau](https://xahau.network/)
Hooks — small WebAssembly programs that attach to an account and run
alongside transactions that touch it. It covers the whole path from source
to a deployable binary: an ergonomic Rust API for the Hook host functions,
procedural macros that remove the WASM export boilerplate, and a build CLI
that turns a `cargo build` artifact into a SetHook-valid `.wasm` file.

## What is a Xahau Hook?

A Hook is a tiny WebAssembly module installed on an account via a `SetHook`
transaction. Once installed, it runs synchronously whenever a transaction
matching its trigger set (`HookOn`) touches that account — before the
transaction is finally applied, for incoming or outgoing transactions or
both. The Hook inspects the transaction and the ledger through a fixed set
of host-provided API functions, then either accepts (lets the transaction
proceed) or rolls back (rejects it), optionally emitting new transactions
of its own along the way.

Because a Hook runs as part of consensus, the host imposes strict rules a
compiled binary must satisfy: it may export only `hook` (and optionally a
`cbak` settlement callback), every loop must carry an explicit guard so the
host can statically bound its worst-case execution, the instruction set is
plain WebAssembly 1.0 with no floating-point opcodes at all, the call graph
must be acyclic, and the whole module must fit in 65,535 bytes. These
constraints are why Hooks aren't written like ordinary Rust programs —
`rshooks` exists to meet them without making the developer hand-encode WASM
exports or floating-point-free arithmetic by hand.

## The four crates

`rshooks` is a small monorepo, layered so each crate has one job:

| crate | description |
|---|---|
| `rshooks-core` | `no_std`, zero-logic FFI layer: raw Hook API declarations and every constant from the xahaud `hook/` headers, translated 1:1 into Rust. |
| `rshooks-macros` | Procedural macros for `rshooks` (the `#[hooks]` struct/impl attribute, XFL literals, and more). |
| `rshooks` | `no_std`, ergonomic wrapper over `rshooks-core` — `Result`-based APIs, typed buffers, the `XFL` decimal-float type, guard/trace macros, and a panic handler. |
| `rshooks-build` | The CLI that turns a Rust crate into one or more SetHook-valid WASM binaries: a discovery build plus one build per declared Hook, each post-processed by a hook-cleaner and guard-checker, natively in Rust. |

**This book focuses on the ergonomic layer** — the `rshooks` crate and its
macros — since that's what Hook authors write against day to day. The raw
`rshooks-core` FFI bindings are covered briefly in the [reference
chapter](reference/raw.md) for when you need to drop down to the bare host
call, but every worked example in this book uses `rshooks`'s typed,
`Result`-returning wrappers.

## How this book is organized

- **Getting Started** walks through installing the toolchain and building
  your first Hook, the minimal `accept-all` example, end to end.
- **Core Concepts** covers the shape every Hook shares: the `#[hooks]`
  struct/impl declaration, entry points, accept/rollback and the error
  model, the loop-guard system, tracing, and the multi-Hook chain model.
- **Working with Data** covers reading the originating transaction, Hook
  state, parameters, typed derives, the `XFL` decimal-float type, and the
  typed slot/keylet layers for reading ledger objects.
- **Emitting Transactions** covers building and submitting a new
  transaction from inside a Hook.
- **Build Toolchain** documents the `rshooks` CLI itself and the per-hook
  attributes it reads to generate a SetHook template.
- **Reference** is a lookup appendix: the full macro list, the prelude's
  contents, the raw FFI layer, and an index of the runnable examples in the
  repository.

Every code sample in this book is adapted from a real, runnable example in
the `rshooks` repository's `examples/` directory — see the [Examples
Index](reference/examples.md) for the complete, numbered list.
