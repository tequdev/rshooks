# `HookError` as a transparent code newtype — design

Status: implemented. Reshapes `rshooks::error::HookError` (`crates/rshooks/src/error.rs`,
`docs/DESIGN.md` §5.1) from a decoding enum into a `#[repr(transparent)]` newtype over the
raw Hook API return code, so that turning a host return into an error and back is the
identity. Tracking issue: tequdev/rshooks#152, item E4.

## 1. The cost being removed

Every Hook API call returns an `i64`; negative values are error codes. Every wrapper in
`rshooks` funnels that value through `error::res`, which builds a `HookError` for the
negative case. With `HookError` as a decoding enum (`Unknown(i64)` carrying a payload,
16 bytes, `#[repr(u8)]`), that construction is a decode: a `-10024` special case, a range
test, a code-to-tag table load from the data segment, and a tag test — roughly 18
instructions per `res` site, plus the table's data segment in every module. The decode runs
eagerly whether or not the caller ever inspects which error it got; in the common
`Err(_) => rollback!(..)` / `.map_err(|_| ..)` / `.is_err()` shapes the decoded value
is discarded, and LLVM cannot always eliminate the table load once the call is inlined into
`hook()`. The inverse, `HookError::code`, is a second table load.

The enum shape also forces two rules on every hook author and on this crate's internals:
at most one specific-variant `match` per function (the decode's control flow nests deeply
once inlined), and "compare the raw `i64` against `rshooks_core::DOESNT_EXIST` before
`res`" inside `rshooks` (`state::decode_read`, `api::state::value_or_absent`, every
generated view's optional-field read).

## 2. Design

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct HookError(i64);

impl HookError {
    pub const OutOfBounds: HookError = /* -1 */;
    pub const InternalError: HookError = /* -2 */;
    /* … one associated const per code in hook/error.h, PascalCase … */
    pub const TooManyNamespaces: HookError = /* -45 */;

    pub const fn code(self) -> i64;          // identity: the raw code
    pub fn kind(self) -> HookErrorKind;      // the one decode, on demand
}

impl From<i64> for HookError { /* the identity */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HookErrorKind {
    OutOfBounds, InternalError, /* … declaration order = table order … */,
    TooManyNamespaces,
    /// Any code without a named constant (positive, `-24`, below `-45`, …).
    Unknown,
}

pub type Result<T> = core::result::Result<T, HookError>;
```

Consequences:

- **`res` is one sign test.** `Err(HookError(code))` is the raw value; no table, no data
  segment. `HookError::code` is a field read. Comparing against a named error
  (`err == HookError::DoesntExist`) is one `i64.eq`, the same instruction the raw-code
  compare inside `rshooks` already used, so the "compare the raw code before `res`"
  convention is a plain style choice where the code is already in hand, not a
  nesting-budget rule for new code.
- **`Result<T, HookError>` is 16 bytes for 8-byte `T`**, down from 24: a scalar pair
  rather than a memory aggregate. The payload is a plain `i64`, not a `NonZeroI64`: the
  niche a `NonZeroI64` would buy `Result<(), HookError>` is not worth the
  `new_unchecked` assume, which blocks LLVM from folding the sign test into an adjacent
  value test (measured on `15_slot-objects`: +1306 WCE and +1 nesting level with
  `NonZeroI64`). `From<i64>` is the identity and never panics; a non-negative code is
  representable (`kind()` reports `Unknown`), and `res` never produces one.
- **Names are kept.** The associated consts are the enum's former variant names, in
  PascalCase (`#[allow(non_upper_case_globals)]`), so every value-position use —
  `Err(HookError::TooSmall)`, `== HookError::DoesntExist`, `Some(HookError::NotImplemented)`,
  `?`, `.map_err(|_| ..)` — compiles unchanged. SCREAMING_CASE names would have forced a
  rename at every one of those sites for no codegen difference.
- **`kind()` is the only decode**, for the rare exhaustive dispatch. It is a
  code-to-kind table lookup (nesting depth 1, not a `match` arm per code, which lowers to
  one nested block per arm), and is paid only at a call site that asks for it. The `-24` gap in
  `hook/error.h` and the out-of-sequence `INVALID_FLOAT = -10024` are handled exactly as
  before: `-10024` is tested before the table, `-24` maps to `Unknown`.
- **`Debug` prints the name**, not the number: `DoesntExist`, or `Unknown(-9999)` for a
  code without a constant — the same text the derived enum `Debug` produced, so test
  output is unchanged.

## 3. Source-visible break

`HookError` is no longer an enum, so its former variants are not patterns:

```rust
// before                                    // after
match r {                                    match r {
    Err(HookError::DoesntExist) => a(),          Err(e) if e == HookError::DoesntExist => a(),
    Err(_) => b(),                               Err(_) => b(),
}                                            }

match err {                                  match err.kind() {
    HookError::TooBig => x(),                    HookErrorKind::TooBig => x(),
    HookError::Unknown(c) => y(c),               HookErrorKind::Unknown => y(err.code()),
    _ => z(),                                    _ => z(),
}                                            }
```

`HookError::Unknown(code)` as a constructor becomes `HookError::from(code)`. Nothing else
changes: `code()`, `From<i64>`, `Result<T>`, `PartialEq`/`Eq`/`Copy`/`Debug` all keep their
signatures and observable behaviour for every code (`HookError::from(c).code() == c` for
every nonzero `c`).

No hook in this repository matched on a variant; the one doc example that did
(`XFL::is_strictly_positive`) uses the guard form above. This is a breaking change on the
0.x line and ships in the next minor release.

## 4. What this replaces

- The "at most one specific-`HookError`-variant match site per function" nesting rule in
  `docs/DESIGN.md` §5.1 applies only to `TxType::from(u16)` / `LedgerEntryType` decodes
  now; a `HookError` comparison has no decode to nest.
- The design-stage "quiet-error accessors" idea (`*_exact_ok -> Option<T>`, estimated
  −43 WCE on `17_sto-writer`) is subsumed: the error those accessors would have avoided
  constructing is free to construct.
- The earlier probe of a safe const value table for `HookError::from` (which kept the
  16-byte enum and measured +13 WCE / +750 B) is a different mechanism and its rejection
  does not apply here.

## 5. Interaction with `Rollback`

There is no `From<HookError> for Rollback`. The conversion is cheap, but a Hook
API error code (`-1..=-45`, `-10024`) is not a hook's own `HookReturnCode`; `?` from a raw
`HookError` into a typed entry would publish the host's code as the hook's verdict. Map at
the call site (`.map_err(|_| MyError::X)?`), as `examples/16_typed-results` does.

## 6. Verification

- `crates/rshooks/src/error.rs` unit tests: round trip for every known code, every
  constant's `kind()`, `-10024`/`-24` irregularity, out-of-range codes, `Debug` text,
  `size_of::<HookError>() == 8` and `size_of::<Result<i64, HookError>>() == 16`.
- `mise run test`, `mise run lint`, `mise run build-examples`,
  `mise run probe:testenv-parity`.
- `mise run record-example-metrics`: every example with a discarded `res` error moves;
  the expected direction is down in WCE and size (the decode table's data segment
  disappears from every module that carried it).
