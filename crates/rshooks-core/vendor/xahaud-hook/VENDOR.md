# Vendored upstream: xahaud Hook API headers

Per `docs/DESIGN.md` §4, `rshooks-core` is a faithful, mechanical translation
of xahaud's Hook API C headers into Rust. The source headers themselves are
vendored here **verbatim, byte-identical from upstream** so the translation
can be parity-tested against them at build time, rather than trusted to stay
in sync by hand. These files are never hand-edited.

## Provenance

- Upstream repository: `Xahau/xahaud`
- Branch: `release`
- Files: `error.h`, `extern.h`, `hookapi.h`, `ls_flags.h`, `macro.h`,
  `sfcodes.h`, `tts.h`, `tx_flags.h`, all from `hook/` on that branch
- Last synced: 2026-07-24
- Recorded hashes: [`SHA256SUMS`](SHA256SUMS) (single source of truth,
  regenerated only by the sync script)

## Rules

- **Never hand-edit these eight files.** Re-sync only with
  `scripts/sync-vendor.sh` (run from the repo root), which downloads all
  eight from the `release` branch, overwrites the vendored copies, and
  regenerates `SHA256SUMS`. If the sync changed anything, regenerate
  `rshooks-core`'s translated sources with `cargo xtask gen-core` (see below),
  review the resulting `git diff`, and re-run `cargo test -p rshooks-core`
  before committing — an upstream header change can change what the Rust
  translation needs to cover.
- `scripts/sync-vendor.sh --check` verifies (without writing) that the
  vendored files are byte-identical to upstream `release` AND match
  `SHA256SUMS`. CI runs this on every push/PR and weekly on a schedule
  (`.github/workflows/vendor-sync.yml`), so upstream drift surfaces as a
  failing workflow instead of a silent divergence.
- **`src/*.rs` (except `lib.rs`) are themselves generated, not hand-edited,
  from these headers**: `cargo xtask gen-core` (see `crates/xtask`) parses
  the eight files above and regenerates `error.rs`, `tts.rs`, `ls_flags.rs`,
  `tx_flags.rs`, `sfcodes.rs`, `consts.rs`, and `api.rs`. Run it after every
  vendor sync that touched this directory, before the parity tests below.
  `cargo xtask gen-core --check` (what CI runs) verifies those files are
  up to date without writing anything, and fails naming whichever files
  have drifted.
- **Parity tests** (`tests/`) parse these headers at test time — C
  `#define`/enum extraction with a tiny shift-add expression evaluator, and
  `extern.h` prototype parsing — and compare complete name/value/signature
  sets against the hand-authored Rust translation in `src/`. An upstream
  header change first fails the drift workflow above; after re-syncing, the
  parity tests fail until the Rust translation is updated to match. The
  translation cannot silently rot.

## License

`xahaud` is ISC-licensed. From upstream's `LICENSE.md`:

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
