# Anatomy of a Hook

A Xahau Hook is a small WebAssembly module with one required export and one
optional export. This page walks through what an `rshooks` hook crate looks
like from top to bottom: the crate shape, the `#[hooks]` struct/impl
declaration, how a hook's execution model shapes the way you write code,
and the statics idiom used for templates and large buffers. Understanding
this shape first makes the rest of the book — data access, errors, guards —
much easier to place. This page covers a single-entry crate; [Hook
Chains](chains.md) extends the same shape to a crate declaring more than
one Hook.

## The crate shape

Every hook crate is a `no_std` `cdylib`:

```rust,ignore
#![no_std]

use rshooks::prelude::*;
use rshooks::*;

#[hooks(description = "Accepts every transaction selected by HookOn.")]
pub struct AcceptAll;

#[hooks]
impl AcceptAll {
    #[hook(0, name = "accept", on = [Invoke])]
    fn main() -> i64 {
        trace!(b"accept-all: accepting transaction");
        accept!()
    }
}
```

(adapted from `examples/01_accept-all/src/lib.rs`.) A few things to notice:

- `#![no_std]` — there is no heap, no OS, no `std::` anything. `rshooks`'s
  `prelude` module gives you the ergonomic surface (typed accessors, macros,
  common types) without depending on `std`.
- The **struct** (`AcceptAll`) is this crate's chain-declaration vessel — a
  place to name shared state/parameter fields (none here) and carry a
  build-only `description`. It's never constructed and holds no runtime
  data; see "The struct has no runtime instance" below.
- The **impl block**, also annotated `#[hooks]`, is where the actual entry
  functions live, each marked with `#[hook(<index>, ...)]` or
  `#[cbak(<index>)]`. `name`/`on`/`can_emit`/`description` are per-entry
  attributes now, rather than a separate top-level declaration — covered in
  full in [Per-Hook Attributes](../build/metadata.md).

## `#[hooks]`: struct and impl, always as a pair

Every chain needs **exactly one** `#[hooks]` struct and **exactly one**
`#[hooks]` impl block for it, in the same module — the two halves are
linked by name (`impl AcceptAll` refers back to `struct AcceptAll`), and
the macros generate a compile-time handshake between them, so an `impl`
with no matching annotated `struct` (or vice versa) fails to compile with a
dedicated error rather than silently doing nothing.

The struct itself can be a plain [unit
struct](https://doc.rust-lang.org/reference/items/structs.html) (`struct
AcceptAll;`, as above, when there's no state or parameters to declare) or a
named-field struct whose fields carry `#[state]`/`#[hook_param]`/
`#[otxn_param]` attributes — covered in [Hook State](../data/state.md) and
[Hook and Transaction Parameters](../data/parameters.md). Moving from one
to the other is exactly "replace the trailing `;` with a field block";
nothing else about the declaration changes.

### The struct has no runtime instance

This is worth stating plainly, because Rust's struct/`impl` syntax normally
implies an object with methods that take `self`. Here it doesn't: a
`#[hooks]` struct is never constructed at runtime, and its entry functions
never take `self` — they're **stateless associated functions**, called the
same way `String::new()` is. Writing `&self` on an entry is a compile
error with a dedicated message ("Hook entrypoints are stateless associated
functions") rather than a type mismatch, since the honest complaint isn't
about types — it's that there's no instance to borrow.

What the struct's fields *do* give you, when it has any, is a single
`static` value (named the same as the struct) whose fields you call
accessor methods on — `AcceptAll.some_field.get()`, for instance. Those
accessors read and write real on-ledger state or parameters; the struct
value itself is a zero-sized handle with nothing to initialize, not a
cache or a session object.

## Entry functions: `#[hook(<index>, ...)]` and `#[cbak(<index>)]`

The Hook host requires a wasm export shaped like
`extern "C" fn hook(_reserved: u32) -> i64`. Writing that by hand means an
`unsafe extern "C"` function signature in every hook crate. `#[hooks]`
avoids that: it takes a plain associated function and generates the export
for you, per selected build (see [Building a Hook](../getting-started/building.md)
for what "per selected build" means).

```rust,ignore
#[hooks]
impl AcceptAll {
    #[hook(0)]
    fn main() -> i64 {
        0
    }
}
```

expands, for index `0`'s own build, to the original function unchanged,
plus:

```rust,ignore
#[unsafe(export_name = "hook")]
pub extern "C" fn __rshooks_hook_sel_0(_reserved: u32) -> i64 {
    AcceptAll::main()
}
```

The macro enforces the annotated item's shape exactly, and reports any
violation as a `compile_error!` at the offending token rather than a panic:

- no arguments, and a return type of exactly `-> i64`;
- no `self` (see above), no `async`/`unsafe`/`const`/`extern` modifiers;
- no generics, no `where` clause.

The annotated function's own name is arbitrary — `main` is just a
convention carried through every example in this book. What matters is the
`hook` export it produces for its declared index.

`#[cbak(<index>)]` is the counterpart for the same index: a Hook entry can
optionally have one, generating a `cbak` export instead of `hook`. The host
invokes it when a transaction the hook previously emitted (via `emit`)
later settles on ledger, so the hook can react to its own emission's
outcome. See [Emitting Transactions](../emit/emitting.md) for a worked
`#[cbak]` example. Both attributes take **one required argument** — the
index — plus, for `#[hook]`, the optional named metadata arguments covered
in [Per-Hook Attributes](../build/metadata.md).

## Execution model

Each Hook invocation runs in a **freshly instantiated wasm instance**: there
is no persistent process, no threads, and no state carried in memory from
one invocation to the next (anything that needs to persist belongs in
[Hook State](../data/state.md), not a Rust `static`'s runtime value). A hook
must always terminate by calling into the host's `accept` or `rollback` —
covered in [Accept, Rollback, and Errors](errors.md) — there is no implicit
"fall off the end and succeed."

This single-threaded, single-shot model is also what makes the statics
idiom below sound: nothing else can be running concurrently, or left over
from a previous call, that could alias a `static`'s contents.

## Source style: what a hook avoids

Because a hook has a small, fixed instruction budget enforced by the guard
system (see [Guards and Loops](guards.md)) and no defined behavior for an
unhandled panic on the Hook host, `rshooks` hooks are written to avoid
panicking operations entirely, not merely to survive them:

- No slice indexing or range-slicing with a non-literal index (use
  `.get()`/`.get_mut()`, which return `Option`, instead) — a literal,
  provably in-bounds index on a fixed-size array is fine.
- No `format!`/`core::fmt` — `trace!`, `accept!`, and `rollback!` all take
  raw byte slices, not formatted strings.
- No `unwrap`/`expect`/`panic!` — every `Result` is handled explicitly,
  typically by rolling back on `Err`.
- Runtime arithmetic uses checked or wrapping operators
  (`.wrapping_add()`, `.checked_add()`, and so on) rather than bare `+`/`-`/
  `*` on non-constant values, which can panic on overflow.

These rules are what the panic handler below exists to catch when something
still slips through — not the primary correctness mechanism.

## The panic handler

`rshooks` ships a `#[panic_handler]` for wasm builds, enabled by the
default-on `panic-handler` feature: if a hook ever does panic, it rolls
back with a fixed message (`b"panic"`) and a distinctive code,
`-999_999`, chosen well outside the documented Hook API error range
(`-1..=-45`, plus `-10024`) so it can never be confused with a real error
code. This is a last-resort backstop for an unhandled panic, not something
to design around — the style rules above are what keep a hook out of this
path in the first place. A hook can disable the default feature and supply
its own handler instead.

A second, non-default feature, `host-panic-handler`, exists purely so a
`no_std` hook crate can be `cargo check`ed on a host target (what
rust-analyzer runs for completion and diagnostics) — a `no_std` crate needs
some `#[panic_handler]` even for host analysis, but the wasm handler above
is target-gated. Enable it only for host analysis; it is never reached in
an actual hook execution, since a host build of a hook crate is for
analysis only.

## Statics for templates and large buffers

Constant byte templates and large output buffers should live in `static`s,
not stack locals. The reason is codegen, not style: a stack-local array
literal is materialized at runtime by a chain of store instructions (real
code bytes, counted against the worst-case instruction count), while a
`static` template becomes a wasm **data segment** costing exactly its own
bytes — no runtime code at all. A large zero-initialized stack buffer is
worse: it compiles to a `compiler_builtins` `memset`-style loop, an
unguarded loop that never appears as a `loop` keyword anywhere in your
source, while a zero-initialized `static` lands in linear memory's BSS —
zero bytes of data segment, zero code, because wasm memory is
zero-initialized by definition.

`rshooks::static_cell::HookStatic` (re-exported from the prelude) is the
safe way to declare one:

```rust,ignore
static TXN: HookStatic<Payment> = HookStatic::new(Payment::new());
```

```rust,ignore
let Some(txn) = TXN.take() else {
    // already taken — see below for why this can only happen once
};
```

(adapted from `examples/10_emit-txn/src/lib.rs`.) `HookStatic::new` is
`const`, so the value's bytes land in a data segment (or BSS, if
all-zero) exactly as described above. `take()` hands out the buffer's one
exclusive `&'static mut` on the first call; every call after that returns
`None`.

That exclusivity is what makes `HookStatic` sound without any `unsafe` at
the call site: two aliasing `&mut` references to the same static can never
be produced, because at most one caller can ever win the take. This safety
argument leans directly on the execution model above — a hook runs
single-threaded, and every invocation gets a freshly instantiated wasm
instance, so "handed out at most once" really does mean "at most once,
ever," with no way for a left-over reference from a prior call to still be
alive. There is deliberately no "give back" operation on `HookStatic`: a
hook runs once and exits.

Converting a real example (`emit-txn`) to this idiom removed its only
compiler-generated loops entirely and cut its worst-case instruction count
by an order of magnitude — see [Guards and Loops](guards.md) for the guard
system this interacts with, and why compiler-generated loops are worth
avoiding rather than just guarding after the fact.

## Where to go next

- [Hook Chains](chains.md) extends this shape to a crate declaring more
  than one Hook — a shared struct, several indexed entries, and what
  changes about the build.
- [Accept, Rollback, and Errors](errors.md) covers how a hook actually
  terminates, and how to give it a meaningful error-code system.
- [Guards and Loops](guards.md) covers the guard system that every loop —
  hand-written or compiler-generated — must satisfy.
- [Reading the Originating Transaction](../data/otxn.md) and
  [Hook State](../data/state.md) cover the data-access APIs a hook's body
  actually calls.
