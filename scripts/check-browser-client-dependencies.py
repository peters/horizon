#!/usr/bin/env python3
"""Keep standalone browser clients independent of Horizon core and UI."""

import pathlib
import subprocess


ROOT = pathlib.Path(__file__).resolve().parent.parent
CLIENTS = ("horizon-browser-mcp", "horizon-browser-cli")
FORBIDDEN = {"horizon-core", "horizon-ui", "horizon-repository", "alacritty_terminal", "eframe", "egui", "winit"}


def main():
    for package in CLIENTS:
        tree = subprocess.run(
            ["cargo", "tree", "--locked", "-p", package, "--edges", "normal,build",
             "--prefix", "none", "--target", "all"],
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
        dependencies = {line.split()[0] for line in tree.splitlines() if line.strip()}
        violations = dependencies & FORBIDDEN
        if violations:
            raise SystemExit(f"{package} depends on host packages: {sorted(violations)}")
        print(f"{package}: standalone dependency boundary passed")


if __name__ == "__main__":
    main()
