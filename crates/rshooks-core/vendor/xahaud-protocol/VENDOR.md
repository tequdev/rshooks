# Vendored upstream: xahaud protocol format definitions

Where [`../xahaud-hook/`](../xahaud-hook/VENDOR.md) vendors the Hook API C
headers a hook *calls*, this directory vendors the definitions of the data a
hook *reads*: xahaud's own declarations of every serialized field, every
transaction format and every ledger entry format. They are vendored
**verbatim, byte-identical from upstream** for the same reason the headers
are — so the Rust side can be generated from them and parity-tested against
them, rather than trusted to stay in sync by hand. These files are never
hand-edited.

## Provenance

- Upstream repository: `Xahau/xahaud`
- Branch: `release`
- Files, and where they come from on that branch:

  | vendored file | upstream path | what it declares |
  |---|---|---|
  | `sfields.macro` | `include/xrpl/protocol/detail/` | every `sfXxx` field: serialized type + field code |
  | `transactions.macro` | `include/xrpl/protocol/detail/` | one `TRANSACTION(tag, value, name, fields)` per transaction type |
  | `ledger_entries.macro` | `include/xrpl/protocol/detail/` | one `LEDGER_ENTRY[_DUPLICATE](tag, value, name, rpcName, fields)` per ledger entry type |
  | `TxFormats.cpp` | `src/libxrpl/protocol/` | the `commonFields` list every transaction format shares |
  | `LedgerFormats.cpp` | `src/libxrpl/protocol/` | the `commonFields` list every ledger entry format shares |
  | `InnerObjectFormats.cpp` | `src/libxrpl/protocol/` | the inner-object formats (`sfEmitDetails`, `sfSigner`, `sfHookExecution`, …) |

  They span two upstream directories but are vendored flat, under their
  basenames.
- Last synced: 2026-08-30
- Recorded hashes: [`SHA256SUMS`](SHA256SUMS) (single source of truth,
  regenerated only by the sync script)

## Rules

- **Never hand-edit these six files.** Re-sync only with
  `scripts/sync-vendor.sh` (run from the repo root), which downloads all six
  from the `release` branch, overwrites the vendored copies, and regenerates
  `SHA256SUMS`. If the sync changed anything, regenerate the artifact below
  with `cargo xtask gen-core`, review the resulting `git diff`, and re-run
  `cargo test --workspace` before committing.
- `scripts/sync-vendor.sh --check` verifies (without writing) that the
  vendored files are byte-identical to upstream `release` AND match
  `SHA256SUMS`. CI runs this on every push/PR and weekly on a schedule
  (`.github/workflows/vendor-sync.yml`), so upstream drift surfaces as a
  failing workflow instead of a silent divergence.
- **`../../protocol_formats.json` is generated from these six files**, not
  hand-edited: `cargo xtask gen-core` (see `crates/xtask`) parses them into
  a versioned intermediate representation and writes it there, exactly as it
  writes `hook_api.json` from the Hook API headers. `cargo xtask gen-core
  --check` (what CI runs) verifies the artifact is up to date without
  writing anything, and fails naming it when it has drifted.
- The parse is **cross-validated against `../xahaud-hook/sfcodes.h`**: every
  field `sfields.macro` declares must exist there with the identical
  `(type << 16) | field` code, or generation fails naming the field. The two
  vendor groups therefore cannot drift out of sync with each other silently:
  re-syncing one without the other is a build failure.

  **One exemption, by upstream's design:** the four fields whose serialized
  type names a whole container rather than a value — `sfTransaction`,
  `sfLedgerEntry`, `sfValidation`, `sfMetadata`, serialized type IDs
  10001–10004 — are declared in `sfields.macro` but deliberately absent
  from `sfcodes.h`, which is written from the Hook API's point of view and
  has no reason to name a field no hook can read. Those four skip the code
  comparison (`protocol_ir::cross_validate`, gated on
  `protocol_parse::PSEUDO_STI_MIN`); they are still carried in the artifact
  with the code the macro implies. Every other field, without exception, is
  checked.
- **Parity test** (`../../tests/protocol_formats_parity.rs`) re-parses the
  three `.macro` files at test time with a deliberately independent minimal
  parser — independent of `xtask`'s for the same reason the header parity
  tests are: a bug in a shared parser would be invisible to the test meant
  to catch it — and cross-checks its counts and a sample of known formats
  against the generated artifact.
- A drift-tripwire test (`../../tests/vendor_sha256.rs`) hashes these six
  files at test time and asserts them against `SHA256SUMS`, so an accidental
  local edit (or a partial/corrupted re-download) fails CI loudly instead of
  silently drifting from what a real xahaud node runs.

## License

`xahaud` is ISC-licensed, and these files carry the upstream copyright
headers verbatim. From upstream's `LICENSE.md`:

```
ISC License

Copyright (c) 2011, Arthur Britto, David Schwartz, Jed McCaleb, Vinnie Falco, Bob Way, Eric Lombrozo, Nikolaos D. Bougalis, Howard Hinnant.
Copyright (c) 2012-2020, the XRP Ledger developers.
Copyright (c) 2020-2024, XRPL Labs.

Permission to use, copy, modify, and distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

The ISC license permits redistribution verbatim (with the copyright and
permission notice intact, as reproduced above and in each file's header),
which this vendoring satisfies.
