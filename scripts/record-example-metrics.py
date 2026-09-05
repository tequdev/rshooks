#!/usr/bin/env python3
"""Record or check each example's WCE / size / nesting snapshot.

Each `examples/<dir>/metrics.json` is the source of truth for that crate's
current `rshooks build`/`check` numbers. Refresh after a library or
example change instead of editing README prose:

    python3 scripts/record-example-metrics.py
    mise run record-example-metrics

`--check` rebuilds (unless `--skip-build`) and fails if any snapshot
would change — that is the GitHub Actions gate against a forgotten
refresh.

By default the script always runs `cargo build --release -p rshooks-build`
first and uses the resulting `target/release/rshooks`, so Cargo's own
freshness check governs whether anything recompiles. Pass `--rshooks` (or
set `RSHOOKS`) to use an existing binary as-is and skip that build.

Usage:
  scripts/record-example-metrics.py
  scripts/record-example-metrics.py --check
  scripts/record-example-metrics.py --example 02_state-counter
  scripts/record-example-metrics.py --skip-build --check --example 02_state-counter
  scripts/record-example-metrics.py --rshooks /path/to/rshooks
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
EXAMPLES_DIR = ROOT / "examples"
SCHEMA = "rshooks-example-metrics-v1"
METRICS_NAME = "metrics.json"

NESTING_RE = re.compile(r"^max nesting depth: (\d+)\s*$", re.MULTILINE)
WCE_LINE_RE = re.compile(
    r"^worst-case instructions: hook=(\d+) cbak=(\d+)\s*$", re.MULTILINE
)


def discover_examples() -> list[str]:
    names = []
    for manifest in sorted(EXAMPLES_DIR.glob("*/Cargo.toml")):
        names.append(manifest.parent.name)
    if not names:
        raise SystemExit(f"FATAL: no examples/*/Cargo.toml under {EXAMPLES_DIR}")
    return names


def metrics_path(example: str) -> Path:
    return EXAMPLES_DIR / example / METRICS_NAME


def current_dir(example: str) -> Path:
    return EXAMPLES_DIR / example / "out" / "current"


def dump_metrics(document: dict[str, Any]) -> str:
    # Insertion order is part of the snapshot contract so git diffs stay
    # stable: schema, then entries sorted by (index, hook_fn), each entry
    # in artifact/index/hook_fn/bytes/wce/max_nesting order.
    return json.dumps(document, indent=2) + "\n"


def canonical_entry(
    *,
    artifact: str,
    index: int,
    hook_fn: str,
    size: int,
    wce_hook: int | None,
    wce_cbak: int | None,
    max_nesting: int,
) -> dict[str, Any]:
    return {
        "artifact": artifact,
        "index": index,
        "hook_fn": hook_fn,
        "bytes": size,
        "wce": {"hook": wce_hook, "cbak": wce_cbak},
        "max_nesting": max_nesting,
    }


def canonical_document(entries: list[dict[str, Any]]) -> dict[str, Any]:
    ordered = sorted(entries, key=lambda e: (e["index"], e["hook_fn"]))
    return {"schema": SCHEMA, "entries": ordered}


def ensure_rshooks(explicit: Path | None) -> Path:
    if explicit is not None:
        path = explicit.expanduser().resolve()
        if not path.is_file():
            raise SystemExit(f"FATAL: --rshooks {path} is not a file")
        return path
    env = os.environ.get("RSHOOKS")
    if env:
        path = Path(env).expanduser().resolve()
        if not path.is_file():
            raise SystemExit(f"FATAL: RSHOOKS={path} is not a file")
        return path
    candidate = ROOT / "target" / "release" / "rshooks"
    # Always run the release build so Cargo's freshness check, not a stale
    # binary sitting at `candidate`, decides whether a rebuild is needed.
    print("-- building rshooks-build (release) --", flush=True)
    subprocess.run(
        [
            "cargo",
            "build",
            "-p",
            "rshooks-build",
            "--release",
            "--manifest-path",
            str(ROOT / "Cargo.toml"),
        ],
        cwd=ROOT,
        check=True,
    )
    if not candidate.is_file():
        raise SystemExit(f"FATAL: expected rshooks-build binary at {candidate}")
    return candidate


def run_rshooks(bin_path: Path, args: list[str], *, capture: bool) -> subprocess.CompletedProcess[str]:
    cmd = [str(bin_path), *args]
    completed = subprocess.run(
        cmd,
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )
    if completed.returncode != 0:
        detail = completed.stderr or completed.stdout or ""
        raise SystemExit(
            f"FATAL: {' '.join(cmd)} exited {completed.returncode}\n{detail}"
        )
    return completed


def build_example(bin_path: Path, example: str, extra_args: list[str]) -> None:
    manifest = EXAMPLES_DIR / example / "Cargo.toml"
    out = EXAMPLES_DIR / example / "out"
    print(f"-- building {example} --", flush=True)
    run_rshooks(
        bin_path,
        [
            "build",
            "--manifest-path",
            str(manifest),
            "--out",
            str(out),
            *extra_args,
        ],
        capture=False,
    )


def list_wasms(example: str) -> list[Path]:
    current = current_dir(example)
    if not current.is_dir():
        raise SystemExit(
            f"FATAL: {current} is missing — build the example first "
            f"or omit --skip-build"
        )
    # `current` is a symlink to the generation directory; Path.glob
    # follows it.
    wasms = sorted(p for p in current.glob("*.wasm") if p.is_file())
    if not wasms:
        raise SystemExit(f"FATAL: no .wasm artifacts under {current}")
    return wasms


def sidecar_for(wasm: Path) -> Path:
    return wasm.with_name(wasm.name[: -len(".wasm")] + ".metadata.json")


def parse_check_output(stdout: str, wasm: Path) -> tuple[int, int, int]:
    nesting_match = NESTING_RE.search(stdout)
    if nesting_match is None:
        raise SystemExit(
            f"FATAL: {wasm}: `rshooks check` did not print max nesting depth:\n{stdout}"
        )
    wce_match = WCE_LINE_RE.search(stdout)
    if wce_match is None:
        raise SystemExit(
            f"FATAL: {wasm}: `rshooks check` did not print worst-case instructions:\n{stdout}"
        )
    return int(wce_match.group(1)), int(wce_match.group(2)), int(nesting_match.group(1))


def collect_entry(bin_path: Path, wasm: Path) -> dict[str, Any]:
    sidecar = sidecar_for(wasm)
    if not sidecar.is_file():
        raise SystemExit(f"FATAL: missing sidecar {sidecar}")
    meta = json.loads(sidecar.read_text())
    try:
        index = int(meta["index"])
        hook_fn = str(meta["hook_fn"])
        wce = meta["WCE"]
        meta_hook = wce["hook"]
        meta_cbak = wce["cbak"]
    except (KeyError, TypeError, ValueError) as exc:
        raise SystemExit(f"FATAL: {sidecar}: expected index/hook_fn/WCE: {exc}") from exc

    completed = run_rshooks(bin_path, ["check", str(wasm)], capture=True)
    check_hook, check_cbak, max_nesting = parse_check_output(completed.stdout, wasm)

    if meta_hook != check_hook or meta_cbak != check_cbak:
        raise SystemExit(
            f"FATAL: {wasm}: sidecar WCE (hook={meta_hook} cbak={meta_cbak}) "
            f"!= check WCE (hook={check_hook} cbak={check_cbak})"
        )

    return canonical_entry(
        artifact=wasm.name,
        index=index,
        hook_fn=hook_fn,
        size=wasm.stat().st_size,
        wce_hook=meta_hook,
        wce_cbak=meta_cbak,
        max_nesting=max_nesting,
    )


def collect_example(bin_path: Path, example: str) -> dict[str, Any]:
    entries = [collect_entry(bin_path, wasm) for wasm in list_wasms(example)]
    return canonical_document(entries)


def unified_diff(path: Path, expected: str, actual: str) -> str:
    import difflib

    return "".join(
        difflib.unified_diff(
            expected.splitlines(keepends=True),
            actual.splitlines(keepends=True),
            fromfile=f"a/{path.relative_to(ROOT)}",
            tofile=f"b/{path.relative_to(ROOT)}",
        )
    )


def process_example(
    *,
    bin_path: Path,
    example: str,
    skip_build: bool,
    extra_args: list[str],
    check: bool,
) -> bool:
    """Return True if the snapshot matches (check) or was written (record)."""
    if not skip_build:
        build_example(bin_path, example, extra_args)
    document = collect_example(bin_path, example)
    actual = dump_metrics(document)
    path = metrics_path(example)
    if check:
        if not path.is_file():
            print(
                f"MISSING {path.relative_to(ROOT)}\n"
                f"  run: python3 scripts/record-example-metrics.py --example {example}",
                file=sys.stderr,
            )
            return False
        expected = dump_metrics(json.loads(path.read_text()))
        if expected != actual:
            print(f"STALE {path.relative_to(ROOT)}", file=sys.stderr)
            diff = unified_diff(path, expected, actual)
            if diff:
                print(diff, end="", file=sys.stderr)
            print(
                f"  re-run: python3 scripts/record-example-metrics.py --example {example}",
                file=sys.stderr,
            )
            return False
        print(f"OK    {example}")
        return True
    path.write_text(actual)
    print(f"WROTE {path.relative_to(ROOT)}")
    for entry in document["entries"]:
        print(
            f"      {entry['artifact']}: {entry['bytes']} bytes  "
            f"WCE hook={entry['wce']['hook']} cbak={entry['wce']['cbak']}  "
            f"nesting={entry['max_nesting']}"
        )
    return True


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Record or check examples/*/metrics.json (WCE / bytes / nesting)."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if any metrics.json would change (do not write)",
    )
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="use existing examples/<dir>/out/current instead of rebuilding",
    )
    parser.add_argument(
        "--example",
        action="append",
        dest="examples",
        metavar="DIR",
        help="only this examples/ directory (repeatable)",
    )
    parser.add_argument(
        "--rshooks",
        type=Path,
        default=None,
        help=(
            "path to the rshooks CLI to use as-is, skipping the release build "
            "(default: always run `cargo build --release -p rshooks-build` "
            "and use target/release/rshooks)"
        ),
    )
    parser.add_argument(
        "build_args",
        nargs="*",
        help="extra arguments forwarded to `rshooks build` (after --)",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    known = discover_examples()
    if args.examples:
        unknown = [name for name in args.examples if name not in known]
        if unknown:
            raise SystemExit(
                f"FATAL: unknown example(s) {unknown}; known: {', '.join(known)}"
            )
        selected = args.examples
    else:
        selected = known

    bin_path = ensure_rshooks(args.rshooks)
    failed = 0
    for example in selected:
        ok = process_example(
            bin_path=bin_path,
            example=example,
            skip_build=args.skip_build,
            extra_args=args.build_args,
            check=args.check,
        )
        if not ok:
            failed += 1

    if args.check:
        if failed:
            print(
                f"\nFAIL: {failed} example metric snapshot(s) stale or missing",
                file=sys.stderr,
            )
            return 1
        print(f"\nPASS: {len(selected)} example metric snapshot(s) match")
        return 0
    print(f"\nrecorded {len(selected)} example(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
