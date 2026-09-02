# slot-objects

## What you'll learn

What the **typed slot layer** (`rshooks::slot_obj`) actually does on a real
node — and, more to the point, how the claims it is built on were checked.

This example is a live acceptance harness, not a tutorial. Read
`examples/08_slot-ledger` first for the everyday shape of the API; come here
for the five things a host build cannot prove.

## The checks

Each contributes one bit to the accept code, so the e2e test can see exactly
which passed. A full pass is `2047` (`0b111_1111_1111`).

| bit | value | check | why it needs a live node |
|---|---|---|---|
| 0 | 1 | **Account-root walk** — `from_keylet` on an account keylet, then typed reads of `sfSequence`/`sfAccount`/`sfBalance` | host stubs return `NotImplemented` for every call; nothing about a real object is observable |
| 1 | 2 | **Drops round-trip** — `as_xfl()` on a native amount, scaled back with `to_int(6, false)`, equals the raw wire drops | `as_xfl` on a native amount yields **XAH units**, not drops (mantissa = drops, exponent −6, normalized). Round 1 of the design review corrected this by a factor of 10⁶; this is the pin |
| 2 | 4 | **Parent-clear then child-read** — derive a child, clear the parent, *then* read the child | `slot_path!` clears each intermediate as soon as its child exists. That is only sound if the host **copies** the parent's storage into the child slot rather than aliasing it |
| 3 | 8 | **`take_*` past the 255-slot budget** — 300 iterations of derive-read-release | the budget is 255 slots per execution. The same loop with a plain `value()` stops at iteration 256 with `NO_FREE_SLOTS`; this proves `take_value()` really frees |
| 4 | 16 | **Failing mid-hop leaks nothing** — 300 `slot_path!` walks whose second hop always fails | the ladder clears the current handle *unconditionally*, before inspecting the result, so a later failing hop cannot leak the parent that produced it. Repeating past the budget is what makes a leak visible |
| 5 | 32 | **Repeated successful navigation** — 260 three-hop walks, each leaf read with `take_value()` | the success path has to recycle too; 260 iterations move 780 slots through a 255-slot budget |
| 6 | 64 | **Failure-path `take_*` cleanup** — 260 *failing* `take_value()` reads | the other half of the `take_*` contract: it clears on failure as well as success, and only that keeps this inside the budget |
| 7 | 128 | **Failed `try_cast` cleans up** — 260 casts that cannot hold | any `try_cast` failure consumes the handle and best-effort clears the slot; repeating past the budget is what proves the clear happened |
| 8 | 256 | **A root slot casts to `STObject`** | a root slot reports a high-level object code (serialized type ID 10001–10004), not the ordinary 14 — the predicate has to accept those, and still reject a wrong target |
| 9 | 512 | **`u64` reads agree** between `value()` and raw bytes | `u64::value()` decodes wire bytes rather than using as-int64 mode, which rejects bit-63 values (`sfExchangeRate` sets one). An account root has no such field, so this pins the two paths agreeing on a real value |
| 10 | 1024 | **IOU `as_xfl`** — the sender's trust-line balance, `is_native() == false`, round-tripped to the amount paid in | the account root's balance is always native, so the IOU branch of `slot_float` needs a `RippleState` object. With bit 1 this covers both branches live |

## Why every check is its own `#[inline(never)]` function

The Hook API's guard checker rejects a module whose block nesting exceeds 32
levels. Five checks' worth of `if let` ladders inlined into one entry point
measured **53**. Splitting each into its own frame brings the hook
comfortably back under the limit (the current value lives in
[`metrics.json`](./metrics.json)) — the same `#[inline(never)]` escape hatch `examples/80_governance` uses against the
same ceiling, and the reason `docs/DESIGN.md` §5.8 recommends keeping
`slot_path!` chains short.

For the record, `slot_path!` itself is not the problem: measured on its own,
its nesting after `rshooks-build`'s unnest pass is **1** at 1, 3 *and* 10 hops,
with worst-case instructions growing linearly with hop count.

## Cost

Each recycling loop runs 260 iterations of real host calls — just past the
255-slot budget, which is the only property that matters — so the worst-case
instruction count is large by design (see [`metrics.json`](./metrics.json)).
This hook exists to exhaust
things, not to be cheap; `examples/08_slot-ledger` is where the layer's
zero-cost claim is measured.

Two constraints shape the structure, both worth knowing before adding a
check here:

- **The guard checker sums every loop in the module.** A `match` over check
  groups does not make them alternatives to it, so all four loops are paid
  for in one worst-case figure against the Hook API's 65,535 ceiling. That
  is why the iteration count is 260 rather than a rounder 300, and why the
  simple `take_*` loop was folded into the successful walk (whose leaf uses
  `take_value()`, so one loop proves both recycling contracts).
- **One check group per invocation.** Running every loop in a single
  execution needed ~130k instructions. The originating transaction carries a
  `CHK` parameter naming one group; the e2e submits one `Invoke` per group
  and ORs the accept codes.

Every loop is `guard!`-bounded by its own iteration count, so the hook is
guard-clean with no extra `rshooks` flags.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/15_slot-objects/Cargo.toml
cargo run -p rshooks-build -- check examples/15_slot-objects/out/current/0.main.wasm
```

## Error codes

`SlotObjectsError` (`rshooks::hook_errors!`, see `src/lib.rs`):

| variant | code | meaning |
|---|---|---|
| `NoSender` | 1 | the originating transaction has no `sfAccount` |
| `KeyletFailed` | 2 | building the account keylet failed |
| `AccountRootFailed` | 3 | loading the sender's account root into a slot failed |

A check that *fails* does not roll back — it simply leaves its bit clear, so
the e2e test can report precisely which invariant broke.
