#!/usr/bin/env bash
# Continuous same-commit wasm-parity probe (design doc §7's "Continuous
# CI" check, as opposed to the one-time A/B/C adoption probe whose results
# are recorded in .claude/design/TESTENV_DESIGN.md).
#
# At the current commit, builds every examples/* crate through the real
# `rshooks build` pipeline TWICE:
#   (i)  as-committed  — the repo tree, untouched.
#   (ii) stripped       — a temp copy with all off-chain-unit-test wiring
#        (tests/ dirs, the rshooks-testenv dev-dependency, the extra
#        "rlib" crate-type, the std-under-test cfg_attr) mechanically
#        removed by strip-testenv-wiring.py.
#
# and asserts every produced .wasm is byte-identical between the two arms,
# plus the WCE (hook/cbak instruction cost), file size and max block/loop/if
# nesting depth `rshooks check` reports for each. Any difference means test
# machinery leaked into the shipped wasm artifact and fails the build.
#
# Never touches the repo's own examples/*/out — both arms build into a
# temp root.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/rshooks-testenv-parity.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

AS_COMMITTED_OUT="$TMP/as-committed"
STRIPPED_SRC="$TMP/stripped-src"
STRIPPED_OUT="$TMP/stripped"

echo "== probe:testenv-parity =="
echo "workdir: $TMP"

# --- Build the rshooks-build binary once; both arms share it. ---------
echo "-- building rshooks-build (release) --"
cargo build -p rshooks-build --release --manifest-path "$ROOT/Cargo.toml" >/dev/null
BIN="$ROOT/target/release/rshooks"
if [[ ! -x "$BIN" ]]; then
  echo "FATAL: expected rshooks-build binary at $BIN" >&2
  exit 1
fi

# --- Discover examples the same way build-examples.yml does: one entry
# per examples/*/Cargo.toml, so a newly added example needs no probe edit.
mapfile -t EXAMPLES < <(for m in "$ROOT"/examples/*/Cargo.toml; do basename "$(dirname "$m")"; done | sort)
echo "examples: ${EXAMPLES[*]}"

# --- Prepare the stripped source tree. ---------------------------------
# Copy only examples/ (excluding its own build cache and prior output),
# and symlink crates/ back to the real directory so examples/*/Cargo.toml's
# `path = "../../crates/..."` dependencies keep resolving. Cargo does NOT
# resolve the symlink when walking up from crates/rshooks/Cargo.toml to
# find the enclosing workspace (it walks the given path textually, not
# canonicalized) — so the root Cargo.toml (declaring `members = ["crates/*"]`,
# which is what crates/rshooks's `.workspace = true` fields resolve
# through) must also be present at $STRIPPED_SRC/Cargo.toml, one level
# above the crates/ symlink, exactly mirroring the real repo layout.
echo "-- preparing stripped tree --"
mkdir -p "$STRIPPED_SRC"
rsync -a \
  --exclude='target' \
  --exclude='out' \
  "$ROOT/examples/" "$STRIPPED_SRC/examples/"
ln -s "$ROOT/crates" "$STRIPPED_SRC/crates"
ln -s "$ROOT/Cargo.toml" "$STRIPPED_SRC/Cargo.toml"
ln -s "$ROOT/Cargo.lock" "$STRIPPED_SRC/Cargo.lock"

python3 "$ROOT/scripts/strip-testenv-wiring.py" "$STRIPPED_SRC/examples"

# --- Build both arms. ----------------------------------------------------
build_arm() {
  local manifest_root="$1" out_root="$2"
  local dir
  for dir in "${EXAMPLES[@]}"; do
    "$BIN" build \
      --manifest-path "$manifest_root/examples/$dir/Cargo.toml" \
      --out "$out_root/$dir/out" \
      >/dev/null
  done
}

echo "-- building as-committed arm --"
build_arm "$ROOT" "$AS_COMMITTED_OUT"

echo "-- building stripped arm --"
build_arm "$STRIPPED_SRC" "$STRIPPED_OUT"

# --- Compare. --------------------------------------------------------------
sha256_of() {
  # macOS has no coreutils sha256sum by default in all environments; shasum
  # -a 256 is the portable choice, sha256sum used first if present.
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

wce_of() {
  # Extracts "hook cbak" WCE from the metadata sidecar's real field names
  # (WCE.hook / WCE.cbak) — see examples/*/out/current/*.metadata.json.
  python3 -c '
import json, sys
with open(sys.argv[1]) as f:
    d = json.load(f)
print(d["WCE"]["hook"], d["WCE"]["cbak"])
' "$1"
}

nesting_of() {
  # max_nesting_depth is not in the metadata sidecar (it has no such
  # field); it is computed by `rshooks check` at validation time, so this
  # runs check and parses its "max nesting depth: N" line.
  "$BIN" check "$1" | sed -n 's/^max nesting depth: //p'
}

FAILED=0
REPORT=()

for dir in "${EXAMPLES[@]}"; do
  a_current="$AS_COMMITTED_OUT/$dir/out/current"
  b_current="$STRIPPED_OUT/$dir/out/current"

  if [[ ! -d "$a_current" || ! -d "$b_current" ]]; then
    echo "FATAL: missing output dir for $dir (as-committed: $a_current, stripped: $b_current)" >&2
    FAILED=1
    continue
  fi

  # `-L`: $a_current/$b_current are themselves the `current` symlink (see
  # mise.toml's build-examples task) — without it, `find` does not descend
  # into a symlinked starting path and silently returns nothing.
  mapfile -t a_wasms < <(find -L "$a_current" -maxdepth 1 -name '*.wasm' -exec basename {} \; | sort)
  mapfile -t b_wasms < <(find -L "$b_current" -maxdepth 1 -name '*.wasm' -exec basename {} \; | sort)

  if [[ "${a_wasms[*]}" != "${b_wasms[*]}" ]]; then
    echo "MISMATCH ($dir): different artifact sets"
    echo "  as-committed: ${a_wasms[*]}"
    echo "  stripped:     ${b_wasms[*]}"
    FAILED=1
    continue
  fi

  for wasm in "${a_wasms[@]}"; do
    a_wasm="$a_current/$wasm"
    b_wasm="$b_current/$wasm"
    a_sha="$(sha256_of "$a_wasm")"
    b_sha="$(sha256_of "$b_wasm")"

    a_meta="${a_wasm%.wasm}.metadata.json"
    b_meta="${b_wasm%.wasm}.metadata.json"
    a_wce="$(wce_of "$a_meta")"
    b_wce="$(wce_of "$b_meta")"

    a_size="$(wc -c <"$a_wasm" | tr -d ' ')"
    b_size="$(wc -c <"$b_wasm" | tr -d ' ')"

    a_nesting="$(nesting_of "$a_wasm")"
    b_nesting="$(nesting_of "$b_wasm")"

    if [[ "$a_sha" != "$b_sha" || "$a_wce" != "$b_wce" || "$a_size" != "$b_size" || "$a_nesting" != "$b_nesting" ]]; then
      echo "MISMATCH ($dir/$wasm):"
      echo "  sha256:  as-committed=$a_sha stripped=$b_sha"
      echo "  WCE:     as-committed=($a_wce) stripped=($b_wce)"
      echo "  size:    as-committed=$a_size stripped=$b_size"
      echo "  nesting: as-committed=$a_nesting stripped=$b_nesting"
      FAILED=1
    else
      REPORT+=("OK   $dir/$wasm  sha256=${a_sha:0:12}  WCE=($a_wce)  size=$a_size  nesting=$a_nesting")
    fi
  done
done

echo
echo "-- results --"
printf '%s\n' "${REPORT[@]}"

if [[ "$FAILED" -ne 0 ]]; then
  echo
  echo "FAIL: probe:testenv-parity found differences between the as-committed and stripped builds" >&2
  exit 1
fi

echo
echo "PASS: probe:testenv-parity — ${#EXAMPLES[@]} examples, all wasm artifacts byte-identical (as-committed == stripped)"
