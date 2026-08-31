# Vendored upstream: xahaud protocol format definitions

This directory contains the upstream definitions used to generate serialized
fields, transaction formats, and ledger entry formats. The files are vendored
verbatim so generated Rust can be parity-tested against xahaud.

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
  | `features.macro` | `include/xrpl/protocol/detail/` | the amendment table, with each amendment's `Supported::yes\|no` — evidence only, see below |

  They span two upstream directories but are vendored flat under their
  basenames.
- Recorded hashes: [`SHA256SUMS`](SHA256SUMS) (single source of truth,
  regenerated only by the sync script)

## Rules

- **Never hand-edit these seven files.** Re-sync only with
  `scripts/sync-vendor.sh` from the repository root. After a change, run
  `cargo xtask gen-core`, review the diff, and run `cargo test --workspace`.
- `scripts/sync-vendor.sh --check` verifies that the vendored files match both
  upstream `release` and `SHA256SUMS` without writing.
- **`features.macro` generates nothing.** It is vendored as *evidence* for
  the curated `../../format_availability.json`. Formats gated by an amendment
  marked `Supported::no` are `dormant` until node support changes. The
  generator does not parse this file.
- **`../../protocol_formats.json` is generated, not hand-edited.**
  `cargo xtask gen-core --check` verifies that it is current without writing.
- Generation cross-validates `sfields.macro` against
  `../xahaud-hook/sfcodes.h`; each ordinary field must have the same
  `(type << 16) | field` code in both vendor groups.

  The four container pseudo-fields (`sfTransaction`, `sfLedgerEntry`,
  `sfValidation`, and `sfMetadata`, type IDs 10001–10004) are intentionally
  absent from `sfcodes.h` and exempt from this comparison. They remain in the
  generated artifact with the codes declared by `sfields.macro`.
- **Parity test** (`../../tests/protocol_formats_parity.rs`) re-parses the
  three `.macro` files with an independent parser and compares every parsed
  format with the generated artifact.
- A drift-tripwire test (`../../tests/vendor_sha256.rs`) hashes these seven
  files and checks them against `SHA256SUMS`.

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
