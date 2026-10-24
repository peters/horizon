#!/usr/bin/env python3
"""Select the release Artifacts matrix for .github/workflows/release-artifacts.yml.

Usage: select-release-artifact-platforms.py "<platform> [<platform> ...]"

Prints `include=<json>`, the `strategy.matrix.include` list for the named
platforms, in the form a workflow step appends to $GITHUB_OUTPUT. Platforms
are separated by whitespace, so a caller can join optional groups with an
expression and leave empty gaps.

An unknown name or an empty selection exits with status 2: a caller that
asks for nothing or for a typo must fail, not build less than it expects.

Each `cache_key` is part of a rust-cache key. Renaming one starts that
platform's cache cold.
"""

import json
import sys

PLATFORMS = {
    "linux-x64": {
        "os": "ubuntu-latest",
        "name": "Linux x64",
        "cache_key": "linux-artifacts",
        "release_asset": "horizon-linux-x64.tar.gz",
        "binary_path": "target/release/horizon",
        "asset_kind": "tar",
    },
    "macos-arm64": {
        "os": "macos-latest",
        "name": "macOS arm64",
        "cache_key": "macos-artifacts",
        "release_asset": "horizon-osx-arm64.tar.gz",
        "binary_path": "target/release/horizon",
        "asset_kind": "tar",
    },
    "macos-x64": {
        "os": "macos-15-intel",
        "name": "macOS x64",
        "cache_key": "macos-intel-artifacts",
        "release_asset": "horizon-osx-x64.tar.gz",
        "binary_path": "target/release/horizon",
        "asset_kind": "tar",
    },
}


def select(selection):
    wanted = selection.split()
    if not wanted:
        raise ValueError("no platform selected")
    unknown = sorted(set(wanted) - PLATFORMS.keys())
    if unknown:
        raise ValueError(f"unknown platform: {' '.join(unknown)} (known: {' '.join(PLATFORMS)})")
    return [{"id": key, **entry} for key, entry in PLATFORMS.items() if key in wanted]


def main(arguments):
    if len(arguments) != 1:
        print(f"usage: {sys.argv[0]} \"<platform> [<platform> ...]\"", file=sys.stderr)
        return 2
    try:
        include = select(arguments[0])
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(f"include={json.dumps(include, separators=(',', ':'))}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
