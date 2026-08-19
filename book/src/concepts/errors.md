# Accept, Rollback, and Errors

A Hook always terminates by calling into the host: either `accept`, which
keeps the originating transaction's effects, or `rollback`, which discards
them. This page covers `rshooks`'s `Result`/`HookError` type for Hook API
failures, the `accept!`/`rollback!` macros that actually end execution, and
`hook_errors!`/`exit_on_err!`, the idiom this crate provides for giving a
hook its own meaningful, stable error-code system instead of one
undifferentiated rollback code everywhere.

## `HookError` and `Result`

Every Hook API function returns an `i64`: a non-negative value is a success
payload (often a byte count, or a slot/field-pointer value), and a negative
value is one of 45 documented error codes from the Hook API. `rshooks`
decodes that negative range into a typed enum, `rshooks::error::HookError`:

```rust
use rshooks::error::HookError;

let err = HookError::from(-5);
assert_eq!(err, HookError::DoesntExist);
assert_eq!(err.code(), -5);
```

Every wrapper in `rshooks::api::*` and `rshooks::xfl` returns
`rshooks::error::Result<T>` — a plain type alias for
`core::result::Result<T, HookError>` — so a failed host call surfaces as an
ordinary `Err(HookError::SomeVariant)` you can match on, rather than a raw
negative integer. `HookError::Unknown(i64)` exists for forward
compatibility, carrying the raw code for any negative value this version of
the crate doesn't yet recognize by name.

It's worth being precise about what `HookError` represents: it's about *why
a host call failed* (out of bounds, doesn't exist, invalid argument, and so
on) — not about *why your hook rejected the transaction*. That second
concept is what the rest of this page covers.

## Ending execution: `accept!` and `rollback!`

A hook must always end by calling one of these two macros. Both accept the
same two grammars:

```rust,ignore
accept!();                 // no message, code 0
accept!(msg, code);        // message bytes + application-defined code

rollback!(msg, code);      // rollback always takes a message and a code
```

`msg` is a raw byte slice (`&[u8]`, typically a byte-string literal like
`b"done"`) — never a formatted string, since `core::fmt`/`format!` aren't
used in hook code (see [Anatomy of a Hook](anatomy.md)). `code` may be a
plain `i64` literal, or any value whose type implements `Into<i64>` — which
is exactly what `hook_errors!` gives you below, so `rollback!(msg, my_enum_variant)`
works directly, without an explicit `.code()`/`i64::from(..)` call at the
call site.

Both macros expand to a call that, on the real wasm host, never returns —
execution unwinds immediately. That's why hooks written against these
macros don't need a placeholder return value on the branches that call
them: `rollback!` has return type `!` (the never type), so it type-checks
against whatever the surrounding `match`/`if` arm needs to produce.

## Designing meaningful error codes with `hook_errors!`

The `code` argument to `rollback!`/`accept!` isn't discarded: xahaud
records it in the transaction's metadata as
`HookExecution.HookReturnCode`. If every rejection path in a hook calls
`rollback!(msg, -1)` with the same code, nothing inspecting the transaction
afterwards — an indexer, a wallet, a support script — can tell *why* it was
rejected without parsing the message text. `hook_errors!` is how `rshooks`
hooks avoid that: one variant per rejection reason, each with its own
explicit, stable discriminant.

Here's the worked example from `examples/04_errors`, a hook that rejects a
`Payment` for one of four distinct reasons:

```rust
use rshooks::hook_errors;

hook_errors! {
    /// Rejection reasons returned by this hook.
    pub enum RejectReason {
        /// The originating account could not be read.
        BadAccountField = -101,
        /// The source tag is blocked.
        BlockedSourceTag = -102,
        /// The amount is not native.
        NotNativeAmount = -103,
        /// The amount exceeds the policy limit.
        AmountTooLarge = -104,
    }
}
```

`hook_errors!` expands this into a `#[repr(i64)]`, `Debug + Clone + Copy +
PartialEq + Eq` enum with the given variants and discriminants, plus:

- `impl From<RejectReason> for i64`;
- an inherent `fn code(self) -> i64` — the same conversion as a method, for
  call sites that prefer `err.code()` over `i64::from(err)`.

Each variant requires an explicit `i64`-valued discriminant — the macro's
grammar enforces this — and negative discriminants work the same as
positive ones, as in the example above. The example crate then adds a
small hand-written `impl` for the parts the macro doesn't generate — a
message per variant, and a `rollback` convenience:

```rust,ignore
impl RejectReason {
    fn message(self) -> &'static [u8] {
        match self {
            RejectReason::BadAccountField => b"errors: could not read otxn Account",
            RejectReason::BlockedSourceTag => b"errors: blocked SourceTag",
            RejectReason::NotNativeAmount => b"errors: unsupported (non-native) Amount",
            RejectReason::AmountTooLarge => b"errors: amount exceeds policy limit",
        }
    }

    fn rollback(self) -> ! {
        rollback!(self.message(), self)
    }
}
```

`rollback!(self.message(), self)` relies on the `Into<i64>` impl
`hook_errors!` generated: `self` (a `RejectReason`) converts through
`i64::from` on its way into the host call, with no `.code()` needed at the
call site. The hook body then runs a short chain of checks, calling
`RejectReason::rollback()` the moment one fails:

```rust,ignore
#[hooks]
impl Errors {
    #[hook(0, on = [Payment])]
    fn main(&self) -> i64 {
        if otxn_field_typed(sfAccount).is_err() {
            RejectReason::BadAccountField.rollback();
        }

        match otxn_field_u64(sfSourceTag) {
            Ok(tag) if tag == u64::from(BLOCKED_SOURCE_TAG) => {
                RejectReason::BlockedSourceTag.rollback()
            }
            _ => {}
        }

        let drops = match otxn_field_typed(sfAmount) {
            Ok(AmountBytes::Native(n)) => u64::from_be_bytes(n.0) & !NATIVE_AMOUNT_FLAG_BITS,
            Ok(AmountBytes::Iou(_)) | Err(_) => RejectReason::NotNativeAmount.rollback(),
        };

        if drops > MAX_DROPS {
            RejectReason::AmountTooLarge.rollback();
        }

        accept!()
    }
}
```

Because `RejectReason::rollback` returns `!`, each `match`/`if` arm that
calls it type-checks against whatever the other arms return — no
placeholder value needed anywhere in the chain.

### How the codes surface on-ledger

| Code | Reason | Message |
|-----:|--------|---------|
| `0` | (via `accept!()`) | every check passed |
| `-101` | `BadAccountField` | `errors: could not read otxn Account` |
| `-102` | `BlockedSourceTag` | `errors: blocked SourceTag` |
| `-103` | `NotNativeAmount` | `errors: unsupported (non-native) Amount` |
| `-104` | `AmountTooLarge` | `errors: amount exceeds policy limit` |

This example deliberately chose codes in `-101..=-104` — well outside the
Hook API's own `-1..=-45`/`-10024` range — so an application-defined
`HookReturnCode` is unambiguous at a glance against a `HookError` that
leaked through instead. That's not a hard requirement, just good hygiene:
pick a range for your own codes and stay out of the Hook API's.

## `exit_on_err!`: converting a `Result` at the boundary

Real hook logic is often broken into small helper functions returning
`Result<T, YourErrorEnum>`, with the conversion to `rollback!` happening
only once, at the point the hook actually needs to exit. `exit_on_err!`
is that conversion point:

```rust
use rshooks::{exit_on_err, hook_errors};

hook_errors! {
    /// Firewall error codes.
    pub enum FirewallError {
        /// The sender is on the blacklist.
        BlockedAccount = 1,
    }
}

fn check(blocked: bool) -> Result<u32, FirewallError> {
    if blocked {
        Err(FirewallError::BlockedAccount)
    } else {
        Ok(42)
    }
}

let value = exit_on_err!(b"firewall: blocked", check(false));
assert_eq!(value, 42);
```

`exit_on_err!(msg, result)` expands to a `match`: `Ok(value)` evaluates to
`value`, and `Err(err)` calls `rollback!(msg, err)` — which, on the real
wasm host, never returns. `E` needs only `Into<i64>`, which every
`hook_errors!` enum provides automatically (a plain `i64` error works too,
via the reflexive `From<i64> for i64`). This is the same "convert at the
boundary" shape `accept!`/`rollback!` already use for their own `code`
argument — ordinary helper functions stay in `Result`-land, and only the
call site that actually needs to end the hook touches `rollback!` directly.

## Typed entry returns: `HookResult`

Everything above uses the entry signature every earlier chapter has shown:
`fn main(&self) -> i64`, ending every path with `accept!`/`rollback!`. A
`#[hook]`/`#[cbak]` entry may instead return `rshooks::exit::HookResult` — a
`Result<Accept, Rollback>` alias — and use ordinary `?` to propagate
failures out of helper functions, in place of a hand-written `accept!`/
`rollback!` call at every failure point:

```rust,ignore
use rshooks::exit::{Accept, HookResult};

hook_errors! {
    pub enum DepositError {
        BadAmount = 1 => b"deposit: bad amount",
        StateSetFailed = 2 => b"deposit: state_set failed",
    }
}

#[inline(always)]
fn read_amount(t: &Vault) -> Result<u64, DepositError> {
    let bytes = t.amount.get_required().map_err(|_| DepositError::BadAmount)?;
    Ok(u64::from_be_bytes(bytes))
}

#[hook(0, on = [Invoke])]
fn deposit(&self) -> HookResult {
    let amount = read_amount(self)?;
    // ... more `?`-propagated steps ...
    Ok(Accept::new(b"deposit: ok", amount as i64))
}
```

`#[hooks]` accepts either return shape on any entry or callback, and a
chain can freely mix them — the `i64` form and the `HookResult` form
coexist in the same `#[hooks] impl` block (`examples/16_typed-results` does
exactly this: a typed `deposit` entry alongside a legacy `reset` entry).
Internally, the generated wrapper calls a sealed
`::rshooks::exit::EntryReturn::finish(..)` on whatever the entry returns:
the identity function for `i64`, or a two-arm `match` calling `accept`/
`rollback` for `HookResult`. Returning any other type is a compile error
naming that trait, not a bespoke macro diagnostic.

### `Accept`, `Rollback`, and the `hook_errors!` message clause

`Accept::new(msg, code)`/`Rollback::new(msg, code)` mirror `accept!`/
`rollback!`'s own arguments; `Accept::code(code)`/`Rollback::code(code)` are
the empty-message shorthand. `?` converts into `Rollback` from two sources:

- **A raw `i64`** — `From<i64> for Rollback` (empty message).
- **Any `hook_errors!` enum** — every enum gets `impl From<Enum> for
  Rollback` unconditionally, and `hook_errors!` now accepts an *optional*
  per-variant message clause that feeds it:

  ```rust
  use rshooks::hook_errors;

  hook_errors! {
      pub enum DepositError {
          /// Message clause: this variant's `Rollback` carries it.
          BadAmount = 1 => b"deposit: bad amount",
          /// No clause: this variant's `Rollback` gets an empty message.
          StateSetFailed = 2,
      }
  }
  ```

  The clause is per-variant — some variants may carry one and others not,
  in the same enum — and an enum with no clause anywhere still gets the
  `From<Enum> for Rollback` impl, with every message empty, so `?` works
  uniformly whether or not any variant bothers with a message.

### The `#[inline(always)]` helper convention

Every helper function called on a `?` path inside a typed entry should be
`#[inline(always)]`, as `read_amount` is above. Measured
(`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` §5's `p2fix` probe): the
same logic through a plain (not force-inlined) `Result`-returning helper
costs a handful of extra worst-case instructions — an un-inlined
call-boundary cost, not a `?`/`Result` cost — while force-inlined, the
identical typed code measured *below* its hand-written `accept!`/
`rollback!` twin. `examples/16_typed-results` follows this convention
throughout.

### The one hard rule: never `?` a raw `HookError` into `Rollback`

There is **no** `From<HookError> for Rollback` impl, and none is planned.
[`HookError::code`](../../crates/rshooks/src/error.rs) is a 45-arm
re-encode match — decoding the negative Hook API return code back into an
enum variant, then re-encoding that variant back into the same `i64` it
came from — and measurement (design doc §5, probe P5) showed a
`?`-propagated two-hop `HookError` → `Rollback` conversion costs **3.1x**
the worst-case instruction count and **+67%** the size of the equivalent
raw-code-check twin. That is exactly the class of regression
`docs/TODO.md`'s item 2 flagged as this feature's biggest risk, and it is
why this crate does not offer the convenient-looking blanket conversion at
all.

The supported pattern for a fallible Hook API call inside a typed entry is
`.map_err(..)`, discarding the decoded `HookError` and keeping only "some
call failed":

```rust,ignore
let value = some_hook_api_call().map_err(|_| MyError::SomeCallFailed)?;
```

— exactly what `read_amount`/`bump_counter` do above. Fall back to
`accept!`/`rollback!` directly (the raw escape hatch, unchanged and always
available) when a computed, non-`'static` message is needed, or when a
match on the specific `HookError` variant is genuinely required (see this
page's `HookError` section above for why matching a specific variant has
its own, larger measured cost).

## Where to go next

- [Guards and Loops](guards.md) covers the other hard constraint every hook
  must satisfy: the loop-guard system.
- [Reading the Originating Transaction](../data/otxn.md) covers the
  `otxn_field_*` calls used in the worked example above.
- [Macro Reference](../reference/macros.md) is the full grammar listing for
  every macro this page uses.
- `examples/16_typed-results` is the full worked example for
  ["Typed entry returns: `HookResult`"](#typed-entry-returns-hookresult)
  above, with `rshooks build`/`check` numbers in its `README.md` and
  off-chain unit tests covering both the accept and `?`-rollback paths.
