# Vendored upstream: xahaud guard checker

Per `docs/DESIGN.md` §6.5, the authoritative accept/reject verdict for
API-version-0 (Guard-type) hooks comes from xahaud's own guard checker,
compiled here from **verbatim, byte-identical upstream source** — not a
Rust reimplementation. These files are never hand-edited.

## Provenance

- Upstream repository: `Xahau/xahaud`
- Commit: `1d5cdfbb43152ba88e5f50fe776ac47bc966ca1a` (branch `HookFeeV2`) —
  the commit the `tequdev/guard-checker` release `v0.1.1-HookFeeV2-1`
  binaries are built from, pinned as `GUARD_CHECKER_REF` in
  `scripts/sync-vendor.sh`
- Files: `Guard.h`, `Enum.h`, `hook_api.macro`, all from
  `include/xrpl/hook/` at that commit
- Last synced: 2026-09-08
- Recorded hashes: [`SHA256SUMS`](SHA256SUMS) (single source of truth;
  regenerated only by the sync script)

## Rules

- **Never hand-edit these three files.** Re-sync only with
  `scripts/sync-vendor.sh` (run from the repo root), which downloads all
  three from the pinned commit, overwrites the vendored copies, and
  regenerates `SHA256SUMS`. To move to another guard-checker release,
  change `GUARD_CHECKER_REF` to the xahaud commit its `xahaud` submodule
  points at. Review the resulting `git diff` and re-run
  `cargo test -p rshooks-build` before committing — an upstream change can
  change checker behavior.
- `scripts/sync-vendor.sh --check` verifies (without writing) that the
  vendored files are byte-identical to the pinned upstream commit AND match
  `SHA256SUMS`. CI runs this on every push/PR and weekly on a schedule
  (`.github/workflows/vendor-sync.yml`), so upstream drift surfaces as a
  failing workflow instead of a silent divergence.
- The only code we author against them is `crates/rshooks-build/cpp/guard_shim.cpp`,
  which compiles `Guard.h` with `-DGUARD_CHECKER_BUILD` (upstream's own
  supported standalone-build mode — see `Enum.h`'s
  `#ifndef GUARD_CHECKER_BUILD` / `#else` split, which stubs `uint256` as
  `std::string` and provides an always-`enabled()` `hook_api::Rules`).
- A drift-tripwire test (`tests/guard_native.rs`,
  `vendored_files_match_recorded_sha256`) hashes these three files at test
  time and asserts them against `SHA256SUMS`, so an accidental local edit
  (or a partial/corrupted re-download) fails CI loudly instead of silently
  drifting from what the real node runs.

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
