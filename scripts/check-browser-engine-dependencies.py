#!/usr/bin/env python3
"""Check the engine in isolation so workspace feature unification cannot mask it."""

import pathlib
import subprocess


ROOT = pathlib.Path(__file__).resolve().parent.parent
FORBIDDEN = {
    "horizon-core", "horizon-ui", "horizon-browser-cli", "horizon-browser-mcp",
    "horizon-browser-control", "horizon-browser-routines", "horizon-repository",
    "alacritty_terminal", "eframe", "egui", "winit", "tokio", "rmcp",
}


def dependencies(*features):
    result = subprocess.run(
        ["cargo", "tree", "--locked", "-p", "horizon-browser",
         "--no-default-features", "--edges", "normal,build", "--prefix", "none",
         "--target", "all", *features],
        cwd=ROOT, check=True, capture_output=True, text=True,
    )
    return {line.split()[0] for line in result.stdout.splitlines() if line.strip()}


def main():
    minimal = dependencies()
    full = dependencies("--features", "video-capture")
    violations = (minimal | full) & FORBIDDEN
    if violations:
        raise SystemExit(f"engine depends on host packages: {sorted(violations)}")
    if "rav1e" in minimal:
        raise SystemExit("minimal engine unexpectedly includes AV1 encoding")
    if "rav1e" not in full:
        raise SystemExit("video-capture feature does not include the AV1 encoder")
    print(f"Engine boundary passed: {len(minimal)} minimal / {len(full)} video packages")


if __name__ == "__main__":
    main()
