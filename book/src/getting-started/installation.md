# Installation

Building a Hook needs two things beyond a normal Rust setup: the
`wasm32v1-none` compilation target, and the `rshooks` CLI that
post-processes cargo's output into a SetHook-valid binary. This page sets
up both, plus the shape of a new Hook crate's `Cargo.toml`.

## Rust toolchain

`rshooks` targets a stable Rust toolchain, edition 2024. `wasm32v1-none` has
been stable since Rust 1.84; the `rshooks` repository itself pins a specific
version via `rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.89.0"
targets = ["wasm32v1-none"]
components = ["rustfmt", "clippy"]
```

If you're not using `rustup`'s toolchain-file auto-detection, add the
target explicitly:

```sh
rustup target add wasm32v1-none
```

## Installing the build CLI

```sh
cargo install rshooks-build
```

This installs a binary named `rshooks` (from the `rshooks-build` package)
used throughout this book. It wraps `cargo build --target wasm32v1-none`
and does not replace your regular `cargo` — you still need a working Rust
install on `PATH`.

## Adding `rshooks` to a new crate

```sh
cargo add rshooks
```

or add it to `Cargo.toml` directly. A minimal Hook crate looks like this:

```toml
[package]
name = "my-hook"
version = "0.1.1"
edition = "2024"

[lib]
crate-type = ["cdylib"]
# no_std cdylibs have no `test` crate for wasm32v1-none; disable the
# (impossible) unit-test harness target.
test = false

[dependencies]
rshooks = "0.1.1"

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
```

A few things worth noting about this shape:

- **`crate-type = ["cdylib"]`** — a Hook compiles to a C-compatible dynamic
  library; that's the artifact `rshooks` post-processes into a `.wasm`
  binary. Plain `cargo build` output from a `cdylib` also exports `memory`,
  which `rshooks` strips along the way (SetHook rejects a module that
  exports anything besides `hook`/`cbak`).
- **The crate itself is `#![no_std]`** (declared in `src/lib.rs`, not
  `Cargo.toml`) — there is no allocator, no `std`, and no panic machinery on
  the Hook host.
- **The release profile matters**, not just for size but for correctness:
  `opt-level = 3` (not the smaller `"z"`) raises the byte threshold below
  which LLVM lowers a stack zero-init to inline stores instead of an
  unguarded `memset`-style loop, which avoids a class of build failures the
  guard checker would otherwise reject. `lto = "fat"`, `codegen-units = 1`,
  `panic = "abort"`, and `strip = "symbols"` all reduce final binary size,
  which matters directly: SetHook's fee scales with the deployed binary's
  byte count. This mirrors the profile the `examples/` workspace itself
  uses — see the [Building a Hook](building.md) chapter for the pipeline
  this feeds into.

## The `host-panic-handler` feature

`rshooks` ships a default `panic-handler` feature that rolls a Hook back on
panic instead of leaving undefined behavior on the wasm target. That
handler is gated to `target_arch = "wasm32"` and does nothing useful for a
plain host `cargo check` — which matters because `no_std` `cdylib` crates
like this one otherwise fail to type-check outside the wasm target (there's
no `std`, and no panic handler for the host target either). Enabling
`host-panic-handler` provides a host-only panic handler purely so tools
like rust-analyzer can run `cargo check` against your Hook crate on your
own machine:

```toml
[dependencies]
rshooks = { version = "0.1.1", features = ["host-panic-handler"] }
```

Never enable this feature from a `std` context — it's meant only to make
host-side tooling work for an otherwise `no_std` Hook crate. It has no
effect on the actual `wasm32v1-none` build.

With the toolchain, the CLI, and a crate shaped like this in place, you're
ready to write your [first Hook](first-hook.md).
