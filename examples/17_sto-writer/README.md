# sto-writer

## What you'll learn

`rshooks::sto_writer::StoWriter`: a bounded, allocation-free writer for a
runtime-sized `STObject`/`STArray`, for transactions `txn_template!` can't
describe because their shape isn't known at compile time. See
[The `StoWriter` API](../../book/src/emit/sto-writer.md) for the full
walkthrough of this crate's `build_remit`, buffer sizing, and
`prepare_for_emit()`/`Prepared::emit()` lifecycle.

## Specific to this example

`examples/22_txn-template-optional` covers the *fixed-shape* alternative
to a conditional entry like this one's optional issued-amount: NOP-padded
`optional` kinds let a field, or a whole nested container, be
present-or-absent while keeping every other field's byte offset
compile-time-fixed, with no `StoWriter` needed — the tradeoff is a
63-NOP-per-container budget that a genuinely open-ended runtime element
count (this crate's own subject) still exceeds.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/17_sto-writer/Cargo.toml
```

No extra flags needed. The backing buffer is a `HookStatic` (a wasm data
segment / BSS, not a stack local), the same static-buffer idiom
`10_emit-txn`'s README describes — recommended for any hook with a buffer
this large, since it avoids a compiler-generated zero-init loop.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p sto-writer
```

Two equivalent layouts exercise the real `StoWriterRemit` entry through
`rshooks_testenv::TestEnv::invoke` — no wasm build, no node: `tests/remit.rs`
(an integration test against the crate as a library) and an in-crate
`#[cfg(test)]` module at the bottom of `src/lib.rs`. See
`book/src/testing/unit-tests.md` for the full walkthrough of both layouts.
Both cover the full `prepare_for_emit()`/`Prepared::emit()` path with the
`sfAmounts` array present — native-only and native-plus-issued shapes, the
`DEST`-missing rollback, and `cbak` — plus, in-crate only,
`build_remit`/`prepare_for_emit`'s own byte-level correctness against a
small local `HostBackend` mock (`build_remit` is private, so only an
in-crate test can call it directly).

## Error codes

`StoWriterError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with — each variant's
doc comment states its meaning.

## Cost

Current WCE, wasm size, and max nesting depth live in
[`metrics.json`](./metrics.json). Higher than `10_emit-txn`'s
fixed-template Payment (see its own `metrics.json`) — expected, since this
hook does strictly more work at runtime (two hook-parameter reads, a
conditional issued-amount branch, and `StoWriter`'s own bounds/duplicate
checks on every field, versus a `const fn`-baked template with none of
that at runtime).
