#!/usr/bin/env python3
"""The release binary's dependencies (I80, decision 030).

akit's release binary links akio and the standard library only (decision 001): no crate from a
registry. This walks the resolved graph from the `akit` package along normal and build edges, for
every target (a dependency behind a target `cfg` counts, whatever the host; one behind a cfg only
test builds set does not), and fails if it reaches a registry crate that ALLOWED does not name. Development dependencies are not followed: they never
reach the binary, and cargo-deny and cargo-audit check them with the rest of the lock file.

Run from anywhere: python3 -I tools/release-deps.py
"""

import json
import os
import subprocess
import sys

# A registry crate allowed in the release binary, each with the decision note that accepted it
# (decision 001's exceptions). Empty: the binary links akio alone.
ALLOWED = {}

# cfgs that only test builds set (akio's loom models: RUSTFLAGS="--cfg loom"); an edge behind one
# never reaches a release build. Every other target cfg is followed, whatever the host.
TEST_ONLY_CFGS = {"cfg(loom)"}


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"],
        cwd=root,
        check=True,
        capture_output=True,
    ).stdout
    meta = json.loads(out)
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    start = [pid for pid, p in packages.items() if p["name"] == "akit" and p["source"] is None]
    if len(start) != 1:
        print("release-deps: no single local `akit` package in the workspace", file=sys.stderr)
        return 2
    seen = set()
    stack = list(start)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            kinds = {
                k["kind"] for k in dep["dep_kinds"] if k.get("target") not in TEST_ONLY_CFGS
            }
            if kinds & {None, "build"}:
                stack.append(dep["pkg"])
    git, registry = set(), []
    for pid in seen:
        p = packages[pid]
        source = p["source"] or ""
        if source.startswith("git+"):
            git.add(source.split("?")[0].removeprefix("git+"))
        elif source:
            registry.append((p["name"], p["version"]))
    bad = sorted((n, v) for n, v in registry if n not in ALLOWED)
    for name, version in bad:
        print(f"release-deps: {name} {version} from a registry reaches the release binary "
              "without a decision note (decision 001)")
    print(f"release-deps: {len(seen)} packages in the release binary's graph; "
          f"git sources: {', '.join(sorted(git)) or 'none'}; "
          f"registry crates: {len(registry)} ({len(registry) - len(bad)} allowed)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
