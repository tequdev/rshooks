# Building a Hook

The previous chapter ran `rshooks build` without explaining what it
actually does. This chapter walks through the pipeline stage by stage, so
the printed report and the `check` subcommand make sense on their own. A
crate can declare more than one Hook (see [Hook
Chains](../concepts/chains.md)); this chapter describes the pipeline for
one declared entry, and the next section covers how it repeats.

## The pipeline

`rshooks build` runs cargo, then a fixed sequence of post-processing
and validation steps:

1. **Discovery build** — `cargo build --release --target wasm32v1-none`
   once, with no entry selected, compiling every declared Hook and `cbak`
   into one artifact. This build is never deployed; its only purpose is to
   read back the crate's declarations (the `#[hooks]` struct's shared
   schema and every `#[hook]`/`#[cbak]` entry's metadata), extracted from
   dead, hex-encoded carrier exports the macros generate and this artifact
   alone carries.
2. **Per-index build, once per declared entry** — for each index the crate
   declares, `cargo rustc --release --target wasm32v1-none -- --cfg
   'rshooks_entry="<i>"'` recompiles the same crate with that one entry
   selected. The `--cfg` flag steers the same `#[hooks]`-generated code to
   export exactly `hook` (and `cbak`, if this index declares one) instead
   of the discovery build's suffixed names — this is the only artifact
   that's ever SetHook-valid for that index. The tool then re-extracts this
   build's own carriers and checks them **byte-for-byte** against
   discovery's — a mismatch (a build script or `cfg`-sensitive macro
   producing different declarations at a different `--cfg` value) is a
   build error naming exactly which entry and field diverged.
3. **Hook-cleaner** (per index) — strips the disallowed `memory` export,
   the now-redundant carrier exports, and any other dead export, and (for
   Guard-type, API version 0, modules) flattens and inlines the crate's
   call graph into the `hook`/`cbak` entry points for *this index only*,
   untangling the resulting block/loop/if nesting so it fits the host's
   structural limits. Because each index is compiled and cleaned
   separately, one index's unreachable code (another entry's logic, in a
   multi-Hook chain) never counts against this index's own size or nesting
   budget.
4. **Guard checker** (per index) — for API version 0, validates that every
   loop begins with the exact guard call sequence the host requires, and
   computes the static worst-case instruction count (WCE) for `hook` and,
   if present, `cbak`, from those guards. This step is skipped for API
   version 1 (Gas-type hooks meter instructions at runtime instead of
   requiring static guards).
5. **Validator** (per index) — checks the complete SetHook rule set:
   exactly one `hook` export (and at most one `cbak`, and only if this
   index declared one), no disallowed imports, no recursion, and a binary
   size at or under the 65,535-byte SetHook limit (unless
   `--allow-oversize` is passed, in which case the output is still written
   but clearly marked invalid).
6. **Sidecar and template generation** — once every index has been built
   and validated, writes one `<index>.<fn>.metadata.json` sidecar per
   entry, then a `sethook.template.json` covering every declared index in
   one `Hooks` array, plus its `sethook.template.meta.json` generation
   sidecar. Covered in [Hook Chains](../concepts/chains.md) and [Per-Hook
   Attributes](../build/metadata.md).
7. **Publish** — stages every artifact from this run, then atomically
   updates the output root's `current` entry (`out/current` below —
   wherever `--out` points, or `<target>/rshooks/<crate-name>` by default;
   see [Your First Hook](../getting-started/first-hook.md#what-lands-in-out))
   to point at it. A failed run never touches `current`; it always resolves
   to the most recent complete, validated build.

Every wasm-producing step runs against the exact bytes that will be
deployed — the WCE and `HookHash` recorded in that entry's metadata
sidecar describe the file actually written to `out/current/`, not an
intermediate artifact.

## Reading the printed report

`build` prints its progress as it goes: a discovery line, one
`building entry <index> (...)` line per declared entry, then a `wrote ...`
line for every published artifact once the whole chain has built
successfully:

```text
discovery build (accept-all)
building entry 0 (`main`)
wrote out/current/0.main.wasm (174 bytes, estimated SetHook fee 870000 drops)
wrote out/current/0.main.metadata.json
wrote out/current/sethook.template.json
wrote out/current/sethook.template.meta.json
```

`build` itself doesn't print the guard checker's or validator's numbers —
those land in that entry's `<index>.<fn>.metadata.json` sidecar (the `WCE`
object) instead. To see them on the terminal, run `rshooks check` against
the binary `build` just wrote:

```text
$ rshooks check out/current/0.main.wasm
worst-case instructions: hook=14 cbak=0
max nesting depth: 0
OK: out/current/0.main.wasm is a valid SetHook wasm binary
size: 174 bytes
estimated SetHook fee: 870000 drops (0.870000 XAH)
```

- **`worst-case instructions`** is the guard checker's static upper bound
  on instructions the host will ever execute for each entry point. It only
  appears for API version 0 (Guard-type) modules — a Gas-type module has no
  static bound of this kind.
- **`max nesting depth`** is the deepest block/loop/if nesting in the final
  module, checked against the host's structural limit — 32 for a
  Guard-type module. This is the number [Hook Chains](../concepts/chains.md)
  covers in more depth: dense use of the typed `#[state]`/`#[hook_param]`/
  `#[otxn_param]` accessors at one call site can push it close to that
  ceiling.
- **`size` / `estimated SetHook fee`** are computed directly from that
  entry's final binary byte count — SetHook's fee schedule is
  `bytes × 5000` drops, so this is the actual one-time deployment fee cost
  of the binary you just built, not an approximation. Because each index is
  its own independent wasm, each has its own size and its own fee — a
  multi-Hook crate's total deployment cost is the sum across every declared
  index.

## Validating a binary without building it

`rshooks check <file>` runs the same guard-checker and validator
steps against an existing wasm file, without invoking cargo or writing any
output. It works on any SetHook-shaped wasm, including one this toolchain
didn't build — see [The `rshooks` CLI](../build/cli.md) for its full
flag reference, and [`build`](../build/cli.md)'s and [`clean`](../build/cli.md)'s
as well.

## A note on `--auto-guard`

Guards are your responsibility by default: an unguarded loop is treated as
a hard build error, on the principle that a missing `guard!` in your own
source is a bug, not something the toolchain should paper over. The
`--auto-guard` flag exists mainly for loops the *compiler* generates that
never appear in your Rust source at all (certain array-equality and
buffer-zeroing patterns can lower to an unguarded loop at the WASM level).
It is deprecated and scheduled for removal. It's covered in full,
including why it's a footgun if used carelessly and the source-level
idioms that avoid needing it in the first place, in the
[Guards and Loops](../concepts/guards.md) chapter.
