#!/usr/bin/env python3
"""Keep Surge Git-tag updates compatible with Dependabot and Cargo --locked."""

from pathlib import Path
import sys
import tomllib
from urllib.parse import parse_qs, urlsplit


def check(root: dict, ui: dict, lock: dict) -> list[str]:
    errors = []
    if "surge-core" in root.get("workspace", {}).get("dependencies", {}):
        errors.append(
            "Declare surge-core directly in crates/horizon-ui/Cargo.toml: "
            "Dependabot's workspace updater does not update Git tags."
        )
    dependency = ui.get("dependencies", {}).get("surge-core", {})
    if not isinstance(dependency, dict) or not dependency.get("git") or not dependency.get("tag"):
        errors.append("surge-core must have a direct Git repository and release tag.")
        return errors
    packages = [p for p in lock.get("package", []) if p.get("name") == "surge-core"]
    if len(packages) != 1:
        errors.append("Cargo.lock must contain exactly one surge-core package.")
        return errors
    source = urlsplit(packages[0].get("source", ""))
    repository = source._replace(query="", fragment="").geturl()
    if (
        repository != "git+" + dependency["git"]
        or parse_qs(source.query).get("tag") != [dependency["tag"]]
        or not source.fragment
    ):
        errors.append("surge-core Git repository/tag differs between Cargo.toml and Cargo.lock.")
    return errors


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    documents = []
    for filename in ("Cargo.toml", "crates/horizon-ui/Cargo.toml", "Cargo.lock"):
        with (root / filename).open("rb") as handle:
            documents.append(tomllib.load(handle))
    errors = check(*documents)
    for error in errors:
        print(error, file=sys.stderr)
    if not errors:
        print("Surge manifest and lockfile pins agree; Git tag is directly updateable.")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
