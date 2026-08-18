# Your First Hook

This chapter walks through `accept-all`, the minimal starter Hook: it
traces a short message, then unconditionally accepts the transaction that
triggered it. No loops, no state, no emitted transactions — a good template
to copy for a new Hook and a good way to see every required piece in one
small file.

Before the code, two concepts this whole book builds on:

- **The struct is a declaration vessel, not a runtime instance.** A
  `#[hooks]` struct never gets constructed and never holds real data at
  runtime — it exists so a name, its fields, and the Hook entries that use
  them can all be declared in one place. `AcceptAll` declares no fields, so
  its entry has nothing to receive and takes no receiver at all — later
  chapters, once a chain has declared fields worth reading, show the `&self`
  form an entry uses to reach them.
- **Every Hook entry has an explicit index.** The index is this crate's
  Hook's position in the account's `Hooks` array (the one a `SetHook`
  transaction installs into) — position `0`, `1`, and so on, up to `9`.
  Even a crate with exactly one Hook must say which position it occupies;
  there's no implicit "the only one."

## The source

Create `src/lib.rs` in the crate you set up in [Installation](installation.md):

```rust
#![no_std]

use rshooks::*;

#[hooks(description = "Accepts every transaction selected by HookOn.")]
pub struct AcceptAll;

#[hooks]
impl AcceptAll {
    /// Accepts every triggering transaction.
    #[hook(0, name = "accept", on = [Invoke])]
    fn main() -> i64 {
        trace!(b"accept-all: accepting transaction");
        accept!()
    }
}
```

To see the `trace!` line actually run, enable the `trace` feature in
`Cargo.toml` alongside `rshooks`:

```toml
[dependencies]
rshooks = { version = "0.0.1", features = ["trace", "host-panic-handler"] }
```

### `#![no_std]`

Every Hook crate is `#![no_std]`: there's no allocator and no `std` on the
Hook host, and `rshooks` itself is `no_std` so it can be linked into one.

### `use rshooks::*;`

This glob import brings in everything declared at `rshooks`'s crate root:
the `#[hooks]` attribute macro, the `XFL!`/`account_id!` macros, the
`accept!`/`rollback!`/`trace!`/`guard!` macro family, and every top-level
module (`api`, `types`, `xfl`, ...) by name. It does **not** bring the
functions inside those modules into scope — a Hook that calls typed API
functions like `otxn_field` or `state` also needs `use rshooks::prelude::*;`,
which this minimal example doesn't, since it never reads the transaction or
touches state. Later chapters that do add that import.

### `#[hooks(description = "...")]` on the struct

`#[hooks]` on `struct AcceptAll;` declares this crate's Hook chain: a
container for shared state/parameter fields (none here — `AcceptAll` is a
[unit struct](https://doc.rust-lang.org/reference/items/structs.html), so
there's nothing to declare) plus the optional `description`, free-form text
carried into the build's generated sidecar. The struct name (`AcceptAll`)
is yours to choose; it plays no on-ledger role.

### `#[hooks]` on the `impl` block, and `#[hook(0, ...)]`

The second `#[hooks]` attribute, on `impl AcceptAll`, marks this as the
chain's entry-point block — exactly one such `impl` is required per
`#[hooks]` struct. Inside it, `#[hook(0, name = "accept", on = [Invoke])]`
declares one Hook entry:

- **`0`** — the required, positional first argument: this Hook occupies
  position `0` in the `Hooks` array. Explained further in [Hook
  Chains](../concepts/chains.md).
- **`name = "accept"`** — the on-ledger `HookName` this Hook installs
  with (optional; omit it for an unnamed Hook).
- **`on = [Invoke]`** — this Hook fires only for `Invoke` transactions.
  Omitting `on` entirely is also legal — see [Per-Hook
  Attributes](../build/metadata.md) for what that means.

The annotated function itself is a plain, argument-less associated function
returning `i64`. `AcceptAll` declares no fields, so there's nothing for a
receiver to reach — omitting it entirely is the natural, minimal form for a
Hook like this one. `#[hooks]` expands it into the wasm export the Hook
host requires:

```rust,ignore
#[unsafe(no_mangle)]
pub extern "C" fn hook(_reserved: u32) -> i64 {
    AcceptAll::main()
}
```

The function's own name (`main` here) is just a convention; what matters is
the `hook` export it produces for this entry's build. A hook entry may
optionally take `&self` — the one receiver form `#[hooks]` accepts, covered
once a chain has fields worth reaching in [Hook State](../data/state.md) —
but any other receiver shape (`self`, `mut self`, `&mut self`, `self: T`)
or a non-`i64` return type is rejected at compile time with a pointed error
rather than a malformed export. Use `#[cbak(0)]` the same way, on the same
index, to declare the optional settlement callback for this entry.

### `accept!()`

`accept!()` calls the host's `accept` function and never returns — its
return type is `!`. `accept!(msg, code)` additionally carries a trace
message and a caller-chosen result code; the bare form used here accepts
with no message and code `0`. Its counterpart, `rollback!(msg, code)`,
rejects the transaction instead. Both are covered in more depth in [Accept,
Rollback, and Errors](../concepts/errors.md).

## Building it

From the crate's own directory:

```sh
rshooks build
```

or from elsewhere, pointing at its manifest:

```sh
rshooks build --manifest-path my-hook/Cargo.toml
```

This compiles your crate once to discover its declared Hook(s), then once
more per declared index, and post-processes each result — see [Building a
Hook](building.md) for exactly what that pipeline does. A successful build
prints something like:

```text
[0] main: worst-case instructions: hook=15 cbak=0
[0] main: max nesting depth: 0
[0] main: wrote out/current/0.main.wasm
[0] main: size: 174 bytes
[0] main: estimated SetHook fee: 870000 drops (0.870000 XAH)
[0] main: wrote out/current/0.main.metadata.json
wrote out/current/sethook.template.json
wrote out/current/sethook.template.meta.json
```

## What lands in `out/`

`rshooks build` writes into a generation directory under `out/`, with
`out/current` symlinked to the latest one (see [Hook
Chains](../concepts/chains.md) for why generations exist). For `AcceptAll`,
`out/current/` contains:

- **`0.main.wasm`** — the cleaned, SetHook-valid binary for index `0`:
  cargo's raw `cdylib` output with the `memory` export stripped and every
  Hook API rule (§ single `hook`/`cbak` export, guarded loops, MVP-only
  instructions) validated. The file name is `<index>.<fn>.wasm` — one file
  per declared entry, so a multi-Hook chain gets one independent binary per
  index.
- **`0.main.metadata.json`** — this entry's metadata sidecar:

```json
{
  "index": 0,
  "hook_fn": "main",
  "cbak_fn": null,
  "name": "main",
  "description": null,
  "HookOn": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF7FFFFFFFFFFFFFFFFFFBFFFFF",
  "HookCanEmit": null,
  "HookName": "616363657074",
  "HookHash": "DCE6A3F81224AE89C557F04D73420D808D9009BCF1CFC1474396CD2DA2D4DF16",
  "WCE": {
    "hook": 15,
    "cbak": 0
  },
  "human": {
    "HookOn": ["Invoke"],
    "HookCanEmit": null,
    "HookName": "accept"
  },
  "chain": {
    "struct": "AcceptAll",
    "description": "Accepts every transaction selected by HookOn.",
    "decls": { "state": [], "hook_params": [], "otxn_params": [] }
  }
}
```

- **`sethook.template.json`** / **`sethook.template.meta.json`** — a
  ready-to-edit `SetHook` transaction template covering every index this
  crate declares, plus a sidecar recording how it was generated. Covered in
  full in [Hook Chains](../concepts/chains.md) and [Per-Hook
  Attributes](../build/metadata.md).

The **`WCE`** (worst-case execution) numbers are the static, guard-derived
upper bound on instructions the host will ever execute for this entry's
`hook`/`cbak` — the same figures the pipeline printed to the terminal. The
**`HookHash`** is Xahau's hash of the deployed binary: the uppercase hex of
the first 32 bytes of the wasm's SHA-512 digest — this is what identifies
the exact Hook code on-ledger, independent of which account installed it.
The **`chain`** object transcribes this crate's shared struct-level schema
(empty here, since `AcceptAll` declares no fields) — every entry's sidecar
carries the same `chain` object, since the schema is shared across the
whole crate, not owned by any one entry.

From here, [Building a Hook](building.md) explains what each pipeline
stage actually does, [Hook Chains](../concepts/chains.md) covers the
multi-Hook model this build pipeline exists for, and [The `rshooks`
CLI](../build/cli.md) is the complete flag reference for every subcommand.
