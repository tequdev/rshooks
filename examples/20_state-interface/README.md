# state-interface

## What you'll learn

The Hook State Interface (`docs/STATE_INTERFACE_DESIGN.md`): a
`HookParameters` convention that exposes a Hook's state layout as a
machine-readable, typed key/value schema, without any protocol change. In
`rshooks`, this is a new `#[state_interface(id = .., key(..), value(..))]`
chain-struct field attribute, sitting alongside `#[state(..)]` in the same
`State` namespace — this example declares one keyed entry and one singleton
entry, the design doc's own worked example.

The surface is still a draft, so this crate's `rshooks` dependency enables
the `unstable-state-interface` feature (see this example's own
`Cargo.toml`) — without it, `#[state_interface(..)]` is a compile error.

## The hook

Credits the sender's balance by 1 on every invocation, keyed by
`(account, token)` (`token` fixed to `0` here — one balance per sender), and
makes sure a singleton `config` entry exists:

```rust
#[hooks]
pub struct Treasury {
    #[state_interface(
        id = 0,
        key(account: AccountId, token: u32),
        value(amount: u64, updated: u32)
    )]
    balances: State<Balance>,

    #[state_interface(id = 1, value(paused: u8))]
    config: State<Config>,
}
```

`balances`/`config` are ordinary `State<V>` fields — `.at(..)`/`.get()`/
`.set()` work exactly like a hand-rolled `#[state(key_by = ..)]` declaration
(see [Hook State](../../book/src/data/state.md)). The difference is entirely
in what gets generated alongside them: `Balance`/`Config` are themselves
generated from the `value(..)` schemas (not written by hand), and the
struct's `#[hooks]` carrier gains a machine-readable description of both
entries' on-ledger layout — see "Code walkthrough" below.

## Code walkthrough

`Balance`/`Config` are generated directly from the `value(..)` schemas —
`pub` fields in declaration order, encoded **big-endian** (unlike this
crate's ordinary little-endian `HookData` convention: a state interface
value is protocol-facing, schema-aware-client-readable data, not a
hook-private encoding):

```rust,ignore
#[derive(Clone, Copy)]
pub struct Balance {
    pub amount: u64,
    pub updated: u32,
}
```

The keyed field's marker generates a `StateSpec` impl whose `encode_key`
builds the full 32-byte key locally — `StateID || account || token || zero
padding` — rather than relying on rshooks' ordinary short-key convention
(see `crates/rshooks/src/si.rs`'s module doc comment, "Why the full 32-byte
key"): the interface fixes the physical key layout as part of its wire
contract, so a schema-aware external client can parse it without knowing
anything about this particular hook. The singleton `config` field's key is
just `StateID || 31 zero bytes`, promoted to a compile-time constant the
same way a literal `#[state(key = b"...")]` is.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/20_state-interface/Cargo.toml --out examples/20_state-interface/out
```

No extra flags. `rshooks check` on the built `0.main.wasm`: worst-case
instructions `374`, size `1054` bytes, max nesting depth `3` (see
`metrics.json`).

Both `#[state_interface(..)]` declarations produce a `HookParameters` entry
in `sethook.template.json`, on every non-gap entry (chain-level, since both
entries share the same account/namespace state) — coexisting with, and
appearing after, any signature-parameter declarations
(`examples/19_param-signature`) the same entry might also carry:

```json
{
  "Hooks": [
    {
      "Hook": {
        "CreateCode": "…hex of 0.main.wasm…",
        "HookNamespace": "<NAMESPACE>",
        "HookApiVersion": 0,
        "HookParameters": [
          {
            "HookParameter": {
              "HookParameterName": "5F534900000208076163636F756E740205746F6B656E",
              "HookParameterValue": "020306616D6F756E74020775706461746564"
            }
          },
          {
            "HookParameter": {
              "HookParameterName": "5F5349000100",
              "HookParameterValue": "011006706175736564"
            }
          }
        ]
      }
    }
  ]
}
```

Unlike a signature-parameter declaration (`HookParameterValue = "00"`), a
state interface declaration's `HookParameterValue` carries the real value
schema — the design doc's own worked spec vector: `balances`' declared name
decodes as `_SI\x00` + State ID `0x00` + 2 key fields (`account: STI_ACCOUNT`,
`token: STI_UINT32`), and its declared value as 2 value fields (`amount:
STI_UINT64`, `updated: STI_UINT32`).

A conforming hook that actually writes `Balance { amount: 1000, updated:
12345 }` for `account =
4B4E9C06F24296074F7BC48F92A97916C6DC5EA9, token = 42` produces the exact
on-ledger `HookStateKey`/`HookStateData` the design doc's §7 spec vector
pins:

- `HookStateKey`: `004B4E9C06F24296074F7BC48F92A97916C6DC5EA90000002A00000000000000`
- `HookStateData`: `00000000000003E800003039` (no field-count prefix or
  separators — unlike the *declaration* value above, the on-ledger data is
  the fields' encodings concatenated directly, per §1.7)

See `crates/rshooks-testenv/tests/state_interface.rs` for this exact
worked example driven end-to-end through `TestEnv::invoke`, with both
vectors asserted against the raw stored bytes.

## Expected behavior

Every `Invoke` credits the sender's balance by 1 and bumps `updated` by 1,
persisting across invocations (`state-interface: credited`, accept code =
the new balance). A `state_set` failure (forced, e.g., by capping the
environment's max state value length in a test) rolls back with
`"state-interface: state_set failed"`.

## Error codes

`StateInterfaceError` (`rshooks::hook_errors!`, see `src/lib.rs`):

| variant | code | meaning |
|---|---|---|
| `MissingAccount` | 1 | the originating transaction has no `sfAccount` field |
| `StateSetFailed` | 2 | the updated balance (or the singleton config) could not be persisted |
