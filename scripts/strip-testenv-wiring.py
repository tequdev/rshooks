#!/usr/bin/env python3
"""Strip off-chain-unit-test wiring from a COPY of examples/.

Used only by scripts/probe-testenv-parity.sh, which builds the same commit
twice — once as-committed, once through this stripper — and diffs the wasm
output. Removes exactly what test wiring added to test-wired example crates
(tests/ dirs, the rshooks-testenv dev-dependency, the extra "rlib"
crate-type, and the std-under-test cfg_attr) so the "stripped" tree matches
what those crates looked like before test wiring existed.

Deliberately per-example, not generic: an unrecognized crate that still
references rshooks-testenv means the stripper is out of date, which must
fail loudly (see the final scan below) rather than silently blind the
parity probe.

Usage: strip-testenv-wiring.py <path-to-copied-examples-dir>
"""

from __future__ import annotations

import re
import shutil
import sys
from pathlib import Path

# Examples with known test wiring (Stage 5) and their transform. Extend
# this set, in lockstep with whichever stage wires a new example, whenever
# tests/ + a rshooks-testenv dev-dependency are added to it.
WIRED_EXAMPLES = {
    "02_state-counter",
    "10_emit-txn",
    "07_xfl-math",
    "13_keylets",
    "08_slot-ledger",
    "15_slot-objects",
    "16_typed-results",
    "17_sto-writer",
    "18_typed-views",
}


def strip_cargo_toml(path: Path) -> None:
    text = path.read_text()

    # crate-type = ["cdylib", "rlib"] -> ["cdylib"] (rlib only existed so
    # tests/ integration tests could link the crate as a dependency).
    new_text = re.sub(
        r'crate-type\s*=\s*\[\s*"cdylib"\s*,\s*"rlib"\s*\]',
        'crate-type = ["cdylib"]',
        text,
    )
    if new_text == text:
        raise SystemExit(f"FATAL: {path}: expected a [\"cdylib\", \"rlib\"] crate-type to strip, found none")
    text = new_text

    # `test = true` (added so `cargo test` runs the crate's own unit-test
    # harness) reverts to the implicit default by dropping the line.
    text = re.sub(r"(?m)^test\s*=\s*true\n", "", text)

    # The whole [dev-dependencies] section (rshooks{testenv} +
    # rshooks-testenv) is test-only; drop it entirely. Section runs from
    # its header to the next `[...]` header or EOF.
    new_text = re.sub(r"(?ms)^\[dev-dependencies\]\n(?:(?!^\[).*\n?)*", "", text)
    if new_text == text:
        raise SystemExit(f"FATAL: {path}: expected a [dev-dependencies] section to strip, found none")
    text = new_text

    if "rshooks-testenv" in text:
        raise SystemExit(f"FATAL: {path}: still references rshooks-testenv after stripping")

    path.write_text(text)


def strip_lib_rs(path: Path) -> None:
    text = path.read_text()

    new_text = text.replace(
        "#![cfg_attr(not(test), no_std)]",
        "#![no_std]",
    )
    if new_text == text:
        # Not every wired example uses the std-under-test escape hatch
        # (e.g. 02_state-counter keeps its tests/ entirely out-of-crate),
        # so this is not itself an error.
        pass
    text = new_text

    # Drop a trailing in-crate `#[cfg(test)] mod tests { .. }` block. It
    # never compiles into the shipped wasm (cfg(test) is off outside
    # `cargo test`), so leaving it in would not change build output — it's
    # removed anyway so the stripped source has no residual reference to
    # rshooks_testenv for the sanity scan below to trip over.
    text = re.sub(r"(?ms)\n#\[cfg\(test\)\]\nmod tests \{.*\Z", "\n", text)

    path.write_text(text)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} <path-to-copied-examples-dir>")
    examples_root = Path(sys.argv[1])
    if not examples_root.is_dir():
        raise SystemExit(f"FATAL: {examples_root} is not a directory")

    handled = set()
    for example_dir in sorted(p for p in examples_root.iterdir() if p.is_dir()):
        cargo_toml = example_dir / "Cargo.toml"
        if not cargo_toml.exists():
            continue
        name = example_dir.name

        if name in WIRED_EXAMPLES:
            handled.add(name)
            tests_dir = example_dir / "tests"
            if tests_dir.exists():
                shutil.rmtree(tests_dir)
            strip_cargo_toml(cargo_toml)
            lib_rs = example_dir / "src" / "lib.rs"
            if lib_rs.exists():
                strip_lib_rs(lib_rs)

    missing = WIRED_EXAMPLES - handled
    if missing:
        raise SystemExit(
            f"FATAL: expected wired examples not found under {examples_root}: {sorted(missing)}"
        )

    # Final sanity scan: any examples/*/Cargo.toml, tests/ dir, or lib.rs
    # std-under-test escape hatch this script did not strip is either a
    # new example that gained test wiring the stripper doesn't know about,
    # or a bug in the transform above — either way, fail loudly rather
    # than let the probe silently compare two identical builds.
    problems = []
    for example_dir in sorted(p for p in examples_root.iterdir() if p.is_dir()):
        cargo_toml = example_dir / "Cargo.toml"
        if not cargo_toml.exists():
            continue
        name = example_dir.name
        cargo_text = cargo_toml.read_text()
        if "rshooks-testenv" in cargo_text:
            problems.append(f"{name}: Cargo.toml still references rshooks-testenv")
        if (example_dir / "tests").exists():
            problems.append(f"{name}: tests/ directory still present")
        lib_rs = example_dir / "src" / "lib.rs"
        if lib_rs.exists() and "cfg_attr(not(test), no_std)" in lib_rs.read_text():
            problems.append(f"{name}: src/lib.rs still has the std-under-test escape hatch")

    if problems:
        details = "\n".join(f"  - {p}" for p in problems)
        raise SystemExit(
            "FATAL: test wiring survived the strip — the stripper doesn't recognize an "
            "example's wiring (add it to WIRED_EXAMPLES / extend the transform):\n" + details
        )


if __name__ == "__main__":
    main()
