# End-to-end testing

`rshooks check` verifies that a Hook wasm binary is valid for SetHook.
The end-to-end suite also deploys the built examples to a standalone Xahau
node and verifies their runtime behavior.

## Prerequisites

- Docker-compatible container runtime
- [`xrpld-lab`](https://pypi.org/project/xrpld-lab/)
- pnpm

The node version is configured by `XAHAUD_VERSION` in `mise.toml`.

## Run the suite

Start a standalone node in one terminal:

```sh
mise run e2e:node-up
```

Run the suite in another terminal:

```sh
mise run e2e
```

Stop the node when finished:

```sh
mise run e2e:node-down
```

`mise run e2e` builds every example before executing the Vitest suite in
`e2e/`. Tests run serially because they share the standalone ledger.

## What the tests verify

The suite deploys example hooks with `@transia/hooks-toolkit`, triggers them,
and inspects transaction metadata and ledger state. Coverage includes:

- Hook acceptance and rollback results
- Hook parameters and Hook state reads and writes
- Slot API and keylet access
- Emitted transactions and callback execution
- Typed data and account-ID macros
- The reward and governance hooks

The test client connects to the standalone node's admin WebSocket endpoint,
submits signed transactions, and advances ledgers with `ledger_accept`.
