# param-signature

## What you'll learn

The Hook Parameter Signature Interface (`docs/PARAM_SIGNATURE_DESIGN.md`):
a `HookParameterName` wire convention that turns a Hook's declared
parameters into a machine-readable, typed function signature. In `rshooks`,
this is the entry fn's own signature — extra arguments after `&self` on a
`#[hook(..)]` fn declare the parameters, decoded before the body ever runs.
This example is the interface draft's own worked example,
`increment(account: AccountID, count: UInt16)`, kept deliberately small so
its generated declarations (in `sethook.template.json`) are easy to read in
full.

The surface is still a draft, so this crate's `rshooks` dependency enables
the `unstable-param-sig-interface` feature (see this example's own
`Cargo.toml`) — without it, an extra `#[hook(..)]` fn argument is a compile
error.

## The hook

Adds `count` to a per-`account` counter in hook state, and accepts with the
new total:

```rust
#[hook(0, on = [Invoke])]
fn increment(&self, account: AccountId, count: u16) -> HookResult {
    let counter = self.state.counters.at(CounterKey { account });
    let current = counter.get().unwrap_or(Some(0)).unwrap_or(0);
    let next = current.wrapping_add(u64::from(count));

    if counter.set(&next).is_err() {
        rollback!(
            b"param-signature: state_set failed",
            ParamSignatureError::StateSetFailed
        );
    }

    accept!(b"param-signature: incremented", next as i64)
}
```

`account`/`count` need no accessor call inside the body at all — by the
time `increment` starts, they're already a plain `AccountId` and `u16`,
decoded by the `#[hooks]`-generated prologue (see "Code walkthrough"
below). The counter itself is ordinary typed hook state, keyed per account
via a single-field `#[derive(HookKey)]` struct — see [Hook
State](../../book/src/data/state.md) and `examples/12_typed-data` for that
part of the model; this example's own subject is the two fn arguments.

## Code walkthrough

Declaring `account`/`count` as extra `#[hook(..)]` fn arguments —

```rust
#[hook(0, on = [Invoke])]
fn increment(&self, account: AccountId, count: u16) -> HookResult { .. }
```

— is exactly equivalent to the macro generating this prologue ahead of the
body, once per argument, in declaration order:

```rust,ignore
let account: AccountId = match ::rshooks::sig::otxn_sig_param::<AccountId>(&const {
    ::rshooks::sig::sig_param_name::<14>(0, 0x08, b"account")
}) {
    Ok(v) => v,
    Err(_) => rollback!(b"rshooks: bad sig param 'account'", 0i64),
};
let count: u16 = match ::rshooks::sig::otxn_sig_param::<u16>(&const {
    ::rshooks::sig::sig_param_name::<12>(1, 0x01, b"count")
}) {
    Ok(v) => v,
    Err(_) => rollback!(b"rshooks: bad sig param 'count'", 1i64),
};
```

`sig_param_name::<N>(index, type_byte, name)` builds the declared
`HookParameterName` — `0x5F 0x50 0x53 | 0x00 | index | type_byte | name.len() | name`
— entirely at compile time (a `const` block), and every MUST of the wire
format (index `<= 0x0F`, a supported type byte, the name's charset/length)
is a `const`-evaluable assert, so a malformed declaration is a compile
error, never a runtime surprise. `otxn_sig_param` reads and decodes the
value big-endian — a signature parameter's value crosses the same protocol
boundary a raw `otxn_field`/`otxn_param` read does, unlike this crate's own
little-endian `ParamValue` wire format (see
`crates/rshooks/src/sig.rs`'s module doc comment, "Why big-endian"). On any
`Err` — absent, or the wrong length for the declared type — `rollback!`
fires immediately, with the argument's own 0-based index as the code, and
`increment`'s body never runs with a partially-decoded invocation.

### The low-level escape hatch

Per the standing rule that every macro surface documents its raw
counterpart, the same read is available directly, without an fn-argument
declaration at all:

```rust,ignore
use rshooks::sig::otxn_sig_param;
use rshooks::sig_name;

// Exactly the declared name `increment`'s prologue builds for `count`
// (index 1, `u16`, `STI_UINT16`).
const COUNT_NAME: [u8; 12] = sig_name!(1, u16, b"count");

let count: rshooks::error::Result<u16> = otxn_sig_param(&COUNT_NAME);
```

See `crates/rshooks-testenv/tests/sig_params.rs` for a hook with the same
declared signature (`account`/`count`, on its own `Increment` chain, not
this crate's) driven end-to-end through `TestEnv::invoke` — including both
rollback paths (a missing argument, a wrong-length value) and a successful
invocation reaching the body with both arguments already decoded.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/19_param-signature/Cargo.toml --out examples/19_param-signature/out
```

No extra flags — this hook has no compiler-generated loop at this
optimization level (see `examples/README.md`'s "On `--auto-guard`"
section). `rshooks check` on the built `0.increment.wasm`: worst-case
instructions `280`, size `910` bytes, max nesting depth `1`.

Because `increment` declares signature parameters, `rshooks build` emits a
`HookParameters` block in `sethook.template.json` for this entry —
superseding the general "`HookParameters` is never generated" rule
(`docs/MULTI_HOOK_STRUCT_DESIGN.md` §9) *only* for declared signature
parameters (see `book/src/build/metadata.md`):

```json
{
  "Hooks": [
    {
      "Hook": {
        "CreateCode": "…hex of 0.increment.wasm…",
        "HookNamespace": "<NAMESPACE>",
        "HookApiVersion": 0,
        "HookParameters": [
          {
            "HookParameter": {
              "HookParameterName": "5F5053000008076163636F756E74",
              "HookParameterValue": "00"
            }
          },
          {
            "HookParameter": {
              "HookParameterName": "5F505300010105636F756E74",
              "HookParameterValue": "00"
            }
          }
        ]
      }
    }
  ]
}
```

These are **declaration** entries, not invocation values: `HookParameterName`
is the full 8..=23-byte wire encoding (`account`'s is `0x5F 0x50 0x53 0x00
0x00 0x08 0x07` + `"account"` = 14 bytes; `count`'s is `0x5F 0x50 0x53 0x00
0x01 0x01 0x05` + `"count"` = 12 bytes), and `HookParameterValue` is always the
literal placeholder byte `00` — the interface draft's own convention for
declaring that a parameter exists, at this index, with this type, without
installing any concrete value for it. Contrast an **invocation** entry,
attached to the `Invoke` transaction that actually triggers this hook, one
per submitter:

```json
{
  "TransactionType": "Invoke",
  "Account": "...",
  "Destination": "...",
  "HookParameters": [
    {
      "HookParameter": {
        "HookParameterName": "5F5053000008076163636F756E74",
        "HookParameterValue": "AABBCCDDEEFF00112233445566778899AABBCCDD"
      }
    },
    {
      "HookParameter": {
        "HookParameterName": "5F505300010105636F756E74",
        "HookParameterValue": "0007"
      }
    }
  ]
}
```

Same `HookParameterName`s (the declaration and the invocation always agree
on index/type/name — that's the whole point), but real, typed
`HookParameterValue`s this time: `account`'s 20 raw bytes, `count`'s `0007`
(`7` as 2 bytes big-endian).

## Expected behavior

- Either `account` or `count` missing, or the wrong length, on the `Invoke`
  → rollback (`"rshooks: bad sig param 'account'"`, code `0`, or
  `"rshooks: bad sig param 'count'"`, code `1` — whichever argument fails
  first, in declaration order; `increment`'s body never runs).
- Both present and well-formed → accept
  (`"param-signature: incremented"`), and the accept code is the account's
  new counter total (`current + count`, wrapping). The counter persists
  across invocations, keyed per `account`.

## Error codes

`ParamSignatureError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for the one failure `increment`'s own body can exit with —
a missing/malformed `account`/`count` signature parameter rolls back
earlier, from the generated prologue, with its own message/code (code = the
argument's own index, `0` or `1` — see "Expected behavior" above), not this
one. `StateSetFailed` is numbered `16`, not `1`: any hook-authored code has
to stay clear of every possible signature-parameter argument index
(`0x00..=0x0F`, i.e. `0..=15`), or the two rollback sources become
ambiguous by code alone — see `book/src/data/parameters.md`'s "the `>= 16`
convention" for the rule this follows:

| variant | code | meaning |
|---|---|---|
| `StateSetFailed` | 16 | the updated counter could not be persisted |
