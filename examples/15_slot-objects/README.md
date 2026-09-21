# slot-objects

## What you'll learn

What the **typed slot layer** (`rshooks::slot_obj`) actually does on a real
node — and, more to the point, how the claims it is built on were checked.
See [Slots and Ledger Objects](../../book/src/data/slots.md) for the API
itself, including the `take_*` recycling contract this example proves live.

This example is a live acceptance harness, not a tutorial. Read
`examples/08_slot-ledger` first for the everyday shape of the API; come here
for the things a host build cannot prove.

## The checks

Each contributes one bit to the accept code, so the e2e test can see exactly
which passed (see the `/// Bit N: ...` doc comments on the check constants
in `src/lib.rs` for the short form).

| bit | check | why it needs a live node |
|---|---|---|
| 0 | **Account-root walk** — `from_keylet` on an account keylet, then typed reads of `sfSequence`/`sfAccount`/`sfBalance` | host stubs return `NotImplemented` for every call; nothing about a real object is observable |
| 1 | **Drops round-trip** — `as_xfl()` on a native amount, scaled back with `to_int(6, false)`, equals the raw wire drops | `as_xfl` on a native amount yields **XAH units**, not drops (mantissa = drops, exponent −6, normalized) — an easy factor-of-10⁶ mistake this pins down |
| 2 | **Parent-clear then child-read** — derive a child, clear the parent, *then* read the child | `slot_path!`'s first hop auto-assigns a fresh slot for the child. That is only sound if the host **copies** the parent's storage into it rather than aliasing it |
| 3 | **`take_*` past the slot budget** — repeated derive-read-release, well past the 255-slot budget | the same loop with a plain `value()` runs out with `NO_FREE_SLOTS`; this proves `take_value()` really frees |
| 4 | **Failing mid-hop leaks nothing** — repeated `slot_path!` walks whose second hop always fails | a hop after the first that fails clears the ladder's one slot before the macro returns, so repeated failures leak nothing |
| 5 | **Repeated successful navigation** — three-hop walks, each leaf read with `take_value()` | the success path has to recycle too, well past the slot budget |
| 6 | **Failure-path `take_*` cleanup** — repeated *failing* `take_value()` reads | the other half of the `take_*` contract: it clears on failure as well as success |
| 7 | **Failed `try_cast` cleans up** — repeated casts that cannot hold | any `try_cast` failure consumes the handle and best-effort clears the slot |
| 8 | **A root slot casts to `STObject`** | a root slot reports a high-level object code (serialized type ID 10001–10004), not the ordinary 14 — the predicate has to accept those, and still reject a wrong target |
| 9 | **`u64` reads agree** between `value()` and raw bytes | `u64::value()` decodes wire bytes rather than using as-int64 mode, which rejects bit-63 values (`sfExchangeRate` sets one). An account root has no such field, so this pins the two paths agreeing on a real value |
| 10 | **IOU `as_xfl`** — the sender's trust-line balance, `is_native() == false`, round-tripped to the amount paid in | the account root's balance is always native, so the IOU branch of `slot_float` needs a `RippleState` object |

## Why every check is its own `#[inline(never)]` function

The Hook API's guard checker rejects a module whose block nesting exceeds 32
levels. All the checks' `if let` ladders inlined into one entry point blow
past that. Splitting each into its own frame brings the hook comfortably
back under the limit — the same `#[inline(never)]` escape hatch
`examples/80_governance` uses against the same ceiling.

For the record, `slot_path!` itself is not the problem: measured on its
own, its nesting after `rshooks-build`'s unnest pass stays flat regardless
of hop count, with worst-case instructions growing linearly instead.

## Cost

Each recycling loop runs well past the 255-slot budget, which is the only
property that matters, so the worst-case instruction count is large by
design (see [`metrics.json`](./metrics.json)). This hook exists to exhaust
things, not to be cheap; `examples/08_slot-ledger` is where the layer's
zero-cost claim is measured.

Two constraints shape the structure, both worth knowing before adding a
check here:

- **The guard checker sums every loop in the module.** A `match` over check
  groups does not make them alternatives to it, so every loop is paid for
  in one worst-case figure against the Hook API's ceiling — which is why
  the simple `take_*` loop was folded into the successful walk (whose leaf
  uses `take_value()`, so one loop proves both recycling contracts).
- **One check group per invocation.** Running every loop in a single
  execution is prohibitively expensive. The originating transaction
  carries a `CHK` parameter naming one group; the e2e submits one `Invoke`
  per group and ORs the accept codes.

Every loop is `guard!`-bounded by its own iteration count, so the hook is
guard-clean with no extra `rshooks` flags.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/15_slot-objects/Cargo.toml
cargo run -p rshooks-build -- check examples/15_slot-objects/out/current/0.main.wasm
```

## Error codes

`SlotObjectsError` (`rshooks::hook_errors!`, see `src/lib.rs`) covers the
setup failures (no sender, keylet, account-root load); each variant's doc
comment states its meaning. A check that *fails* does not roll back — it
simply leaves its bit clear, so the e2e test can report precisely which
invariant broke.
