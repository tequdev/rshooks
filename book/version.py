#!/usr/bin/env python3
"""mdBook preprocessor that replaces `{{version}}` with the workspace version
read from the repo-root Cargo.toml's `[workspace.package]` section."""
import json
import os
import re
import sys

if len(sys.argv) > 1 and sys.argv[1] == "supports":
    sys.exit(0)

context, book = json.load(sys.stdin)
cargo_toml = os.path.join(context["root"], "..", "Cargo.toml")
with open(cargo_toml, encoding="utf-8") as f:
    manifest = f.read()

section = re.search(r"^\[workspace\.package\]\n(.*?)(?=^\[|\Z)", manifest, re.S | re.M)
match = section and re.search(r'^version\s*=\s*"([^"]+)"', section.group(1), re.M)
if not match:
    sys.exit("version.py: could not find version in [workspace.package]")
version = match.group(1)


def replace(item):
    chapter = item.get("Chapter")
    if not chapter:
        return
    chapter["content"] = chapter["content"].replace("{{version}}", version)
    for sub in chapter["sub_items"]:
        replace(sub)


for item in book["items"]:
    replace(item)

json.dump(book, sys.stdout)
