<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-banner.svg" />
    <source media="(prefers-color-scheme: light)" srcset="assets/hero-banner.svg" />
    <img src="assets/hero-banner.svg" alt="Horizon — Your Terminals, One Canvas" width="100%" />
  </picture>
</p>

<p align="center">
  <a href="https://github.com/peters/horizon/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/peters/horizon?style=flat-square&color=74a2f7" /></a>
  <a href="https://github.com/peters/horizon/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/peters/horizon/ci.yml?branch=main&style=flat-square&label=CI" /></a>
  <img alt="License" src="https://img.shields.io/badge/license-MIT-a6e3a1?style=flat-square" />
  <img alt="Platform" src="https://img.shields.io/badge/Linux%20%C2%B7%20macOS%20%C2%B7%20Windows-cba6f7?style=flat-square" />
</p>

<p align="center">
  <b>Horizon</b> is a GPU-accelerated terminal board that puts all your sessions<br/>
  on an infinite canvas. Organize, pan, zoom, and never lose a terminal again.
</p>

<p align="center">
  <img src="assets/demo.gif" alt="Horizon demo — panning across AI Agents, Dev, and Monitoring workspaces" width="800" />
</p>

---

## Why Horizon?

Tabbed terminals hide your work. Tiled terminals box you in. **Horizon gives you a canvas** — an infinite 2D surface where every terminal lives as a panel you can place, resize, and group however you want.

Think of it as a whiteboard for your terminal sessions. Keep your frontend, backend, logs, and AI agents all visible at once — across multiple color-coded workspaces — and navigate between them with a minimap.

---

## Highlights

<table>
<tr>
<td width="50%">

### Infinite Canvas
Pan and zoom freely across a boundless workspace surface. Place terminals anywhere. A **minimap** in the corner keeps you oriented — click it to jump.

</td>
<td width="50%">

### Workspaces
Group related panels into **color-coded workspaces**. Auto-arrange with five layout modes — rows, columns, grid, stack, cascade — or drag panels freely.

</td>
</tr>
<tr>
<td>

### Full Terminal Emulation
24-bit color, mouse reporting, scrollback, alt-screen, and Kitty keyboard protocol. Powered by the **Alacritty terminal engine** — the same one behind the fastest terminal on the planet.

</td>
<td>

### AI Agent Panels
First-class **Claude Code** and **Codex** integration. Sessions persist and auto-resume. A live **usage dashboard** tracks token spend across agents.

</td>
</tr>
<tr>
<td>

### Git Integration
A built-in **git status panel** watches your repo in the background. See changed files, inline diffs, and hunk-level detail — no context switching.

</td>
<td>

### Smart Detection
**Ctrl+click** a URL to open it. Hover a file path and click to jump to it. Horizon sees what your terminal prints and makes it interactive.

</td>
</tr>
<tr>
<td>

### Live Settings Editor
Open the config with **Ctrl+,** — a side panel with **YAML syntax highlighting** and live preview. Every change applies instantly to the canvas behind it.

</td>
<td>

### Session Persistence
Close Horizon, come back tomorrow. Your workspaces, panel positions, scroll positions, and terminal history are **restored exactly as you left them**.

</td>
</tr>
</table>

---

## Install

### Download (fastest)

Grab the latest binary from [**Releases**](https://github.com/peters/horizon/releases/latest) — no dependencies needed.

| Platform | Asset | |
|:---------|:------|:-|
| **Linux** x64 | `horizon-linux-x64.tar.gz` | Extract, `chmod +x`, run |
| **macOS** arm64 | `horizon-osx-arm64.tar.gz` | Extract, `chmod +x`, run |
| **macOS** x64 | `horizon-osx-x64.tar.gz` | Extract, `chmod +x`, run |
| **Windows** x64 | `horizon-windows-x64.exe` | Download and run |

### Build from source

```bash
git clone https://github.com/peters/horizon.git
cd horizon
git lfs install
git lfs pull
cargo run --release
```

> Requires **Git LFS** for bundled assets and **Rust 1.85+**. Linux needs system headers for GPU rendering — see [AGENTS.md](AGENTS.md#prerequisites) for per-distro install commands.

---

## Quick Tour

| Shortcut | What it does |
|:---------|:-------------|
| **Ctrl+N** | New terminal panel |
| **Ctrl+K** | Quick-navigate to any workspace |
| **Ctrl+,** | Open settings editor |
| **Ctrl+Plus / Ctrl+Minus** | Zoom canvas in or out |
| **Ctrl+Scroll** | Zoom around the cursor |
| **Ctrl+B** | Toggle sidebar |
| **Ctrl+M** | Toggle minimap |
| **Ctrl+0** | Reset canvas view |
| **F11** | Fullscreen the active panel |
| **Ctrl+Click** | Open URL or file path under cursor |
| **Ctrl+double-click** canvas | Create a new workspace |

<sub>On macOS, substitute Cmd for Ctrl.</sub>

---

## Configuration

Horizon reads `~/.horizon/config.yaml`. Define workspaces, panel presets, and feature flags:

```yaml
workspaces:
  - name: Backend
    cwd: ~/projects/api
    panels:
      - kind: shell
      - kind: claude
      - kind: git_changes

  - name: Frontend
    cwd: ~/projects/web
    panels:
      - kind: shell
      - kind: shell

presets:
  - name: Shell
    alias: sh
    kind: shell
  - name: Claude Code
    alias: cc
    kind: claude
  - name: Git Changes
    alias: gc
    kind: git_changes

features:
  attention_feed: true
```

---

## Built With

| | |
|:--|:--|
| [**Rust**](https://www.rust-lang.org) | Edition 2024, safe and fast |
| [**eframe / egui**](https://github.com/emilk/egui) | Immediate-mode UI framework |
| [**wgpu**](https://wgpu.rs) | GPU rendering — Vulkan, Metal, DX12, OpenGL |
| [**alacritty_terminal**](https://github.com/alacritty/alacritty) | Battle-tested terminal emulation |
| [**Catppuccin Mocha**](https://catppuccin.com) | Dark color palette |

---

## Contributing

See [**AGENTS.md**](AGENTS.md) for development setup, architecture, coding standards, and CI requirements.
Manual smoke-test plans live under [**docs/testing**](docs/testing), including the
[**workspace close smoke test**](docs/testing/workspace-close-smoketest-plan.md).

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
```

---

<p align="center">
  <sub>MIT License</sub>
</p>
