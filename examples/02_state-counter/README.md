# state-counter

Maintains a persistent counter in Hook state: reads the current `u64`
count (defaulting to zero if absent or of unexpected size), increments it,
writes it back, and accepts with the new count as the return-code
payload.

This is the minimal tutorial for `rshooks`'s **typed storage layer**, now
declared as a `#[state]`-attributed field on the hook's `#[hooks]` struct —
no hand-rolled `[0u8; 8]` buffer, no manual `from_le_bytes`/`to_le_bytes`,
no length check:

```rust
#[hooks]
pub struct StateCounter {
    #[state(key = b"counter")]
    counter: State<u64>,
}

#[hooks]
impl StateCounter {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let count = self.state.counter.get().unwrap_or(Some(0)).unwrap_or(0);
        let next = count.wrapping_add(1);
        self.state.counter.set(&next);
        // ...
    }
}
```

`#[state(key = <expr>)]` is the **constant-key** form: `<expr>` is any
const expression whose type implements `StateKeyEncode` — a byte-string
literal like `b"counter"` works directly, because `StateKeyEncode` is
implemented for `[u8; N]`. The struct macro expands this into:

- a hidden per-field marker type carrying the key spec as a `StateSpec`
  trait impl (so distinct `State<V>` fields, even ones sharing a value
  type, each get their own key encoding — see
  `docs/MULTI_HOOK_STRUCT_DESIGN.md` §5.4 for why a marker type is needed
  at all),
- the field's type rewritten to `State<u64, __marker>` behind the scenes
  (the `State<u64>` you write is sugar over that),
- a `static StateCounter: StateCounter` value binding (the struct's field
  values are all zero-sized, so this is free). Inside the `#[hooks] impl`,
  an entry declares a `&self` receiver and that same static is passed in,
  so `self.state.counter` and `StateCounter.state.counter` name the identical value —
  `self.` is just the ordinary Rust spelling for "the receiver a method was
  called on." Because it's a reference to a zero-sized value, `&self`
  optimizes away entirely, compiling to the same wasm as reading the static
  directly. Code *outside* the `impl` — a free function, another module — has no
  `self` to borrow, and reaches the same static by its struct name instead:
  `StateCounter.state.counter`.

Because this field's `KeyArgs` is `()` (a constant key, not a per-instance
one), `.get()`/`.set()`/`.update()`/`.delete()` are available directly on
`self.state.counter` — no `.at(key)` call needed. See `examples/12_typed-data`
for the keyed form (`#[state(key_by = SomeKey)]`), which does need
`.at(key)`.

## Same slot as before: real-length encoding, host left-pads

The key sent is exactly `counter`'s own 7 bytes (see `rshooks::state`'s
module doc comment, "Key length and padding," and `docs/DESIGN.md` §5.7) —
never locally zero-padded up to the fixed 32-byte key space. That lands on
the same host-left-padded on-ledger slot as the C hook
`state(&v, 8, "counter", 7)`.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/02_state-counter/Cargo.toml
```

No extra flags needed — this example is guard-clean without `--auto-guard`.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p state-counter
```

`tests/counter.rs` drives the real `StateCounter` chain through
`rshooks_testenv::TestEnv::invoke` — no wasm build, no node: a first-invoke
assertion, persistence across two invocations, and a forced `state_set`
failure proving the rollback path leaves no trace. See
`book/src/testing/unit-tests.md` for the full walkthrough and what this
harness does and does not model.

## Error codes

`StateCounterError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `StateSetFailed` | 1 | `state_set` failed to persist the incremented counter |

## Cost of the typed layer, here

The typed layer's convenience (no hand-written buffer/length-check/
byte-order code) isn't free: `state_get_typed`/`state_set_typed` go
through `crate::state`'s generic, 32-byte-scratch-buffer machinery
(`MAX_TYPED_STATE_LEN`), rather than this hook reading/writing a plain
8-byte buffer via the raw `state`/`state_set` calls directly. Measured
(`rshooks build`/`check`): 251 worst-case instructions / 736 bytes,
versus 58 / 349 for a hand-rolled-buffer version of this same hook. Still
guard-clean at the source level — no `--auto-guard`/
`--default-maxiter` needed. For a hook this simple (one `u64` counter,
one key), the raw layer is the cheaper choice; this example uses the
typed layer anyway because its purpose is to be the smallest possible
tutorial for it — see `examples/12_typed-data` for the typed layer's
actual selling point (a *composite*, multi-field key/value pair, where
hand-packing would be far more error-prone than the cost shown here).
