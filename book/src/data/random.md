# Consensus Randomness

`dice` and `util_random` draw from the ledger's consensus RNG, which the
`RNG` amendment adds. SetHook rejects a wasm that imports `dice` or `util_random`
until the amendment is enabled. Validators contribute entropy to a `Random`
ledger object at the start of each ledger, and every draw from any Hook
advances that object's state. No Hook can read the state: `slot_set` and
`ledger_keylet` refuse the `Random` object, and it is kept out of
`meta_slot`. A draw is therefore unpredictable even to a Hook that ran
earlier in the same transaction.

```rust,ignore
// A uniform number in 0..6.
let roll = dice(6)?;

// Fill a buffer with random bytes.
let mut buf = [0u8; 8];
let n = util_random(&mut buf)?;
```

`rshooks::api::util::dice(sides: u32) -> Result<u64>` returns a number in
`[0, sides)` with every side equally likely. `util_random(out)` fills `out`
and returns the number of bytes written.

## Failures

- `dice(0)` is `InvalidArgument`.
- `util_random` with an empty buffer is `TooSmall`, and with more than 512
  bytes is `TooBig`.
- `TooLittleEntropy` means the ledger has no usable entropy: no `Random`
  object exists, it was seeded for another ledger, or fewer than 80% of the
  validators contributed to it. Treat it as "no randomness available right
  now" and take a defined path rather than substituting a predictable value.
- `TooManyNonces` means the execution already made 256 draws.

Argument errors are reported before any entropy check and cost no draw.

## Budget

An execution (a `hook` or `cbak` call) may make 256 successful draws,
shared by `dice` and `util_random`. A `util_random` call costs a single draw
whatever its length. Failed calls cost nothing, and the next execution
starts with a fresh budget.

## Rollback does not undo draws

Draws are never rewound. A transaction whose Hook rolls back has still
advanced the ledger's RNG state, so a rollback cannot be used to retry for a
different result: the next execution draws from the advanced state.

## Testing

`rshooks-testenv` returns `TooLittleEntropy` until the test seeds a `Random`
object with `TestEnv::rng_seed`; see
[Off-Chain Unit Tests](../testing/unit-tests.md).

The standalone node the end-to-end suite uses has no validator keys
configured, so it builds no entropy transaction and its ledger never holds a
`Random` object. An end-to-end test against it exercises the
`TooLittleEntropy` path only. The `rng` example (`examples/23_rng`) covers
the seeded and the unseeded path.
