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
  on an infinite canvas. Organize by workspace, launch from presets, quick-nav fast, and never lose a terminal again.
</p>

<p align="center">
  <img src="assets/demo.gif" alt="Horizon demo — panning across AI Agents, Dev, and Monitoring workspaces" width="800" />
</p>

---

## Why Horizon?

Tabbed terminals hide your work. Tiled terminals box you in. **Horizon gives you a canvas** — an infinite 2D surface where every terminal lives as a panel you can place, resize, and group however you want.

Think of it as a whiteboard for your terminal sessions with a structured workflow on top. Start with color-coded workspaces, launch preset panels, jump with Quick Nav, and fit the active workspace whenever you want a clean overview.

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
Group related panels into **color-coded workspaces**. Auto-arrange with three layout modes — rows, columns, grid — or drag panels freely.

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

### Remote Hosts
**Ctrl+Shift+H** opens a fast overlay that discovers hosts from SSH config and Tailscale. Search, filter, and connect instantly. Type **user@filter** to override the SSH user. Connected sessions are grouped into a **Remote Sessions** grid workspace.

</td>
<td>

### Live Settings Editor
Open the config with **Ctrl+Shift+,** — a side panel with **YAML syntax highlighting** and live preview. Every change applies instantly to the canvas behind it.

</td>
</tr>
<tr>
<td>

### Session Persistence
Close Horizon, come back tomorrow. Your workspaces, panel positions, scroll positions, and terminal history are **restored exactly as you left them**.

</td>
<td>

### Markdown Editor
Drop a `.md` file onto the canvas or create one from the command palette. **Split view** with syntax highlighting and live preview, saved with **Ctrl+Shift+S**.

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

> Requires **Git LFS** for bundled assets and **Rust 1.88+**. Linux needs system headers for GPU rendering — see [AGENTS.md](AGENTS.md#prerequisites) for per-distro install commands.

---

## Quick Tour

### Keyboard Shortcuts

All app shortcuts use **Ctrl+Shift** to avoid conflicts with shell programs (Ctrl+C, Ctrl+K, Ctrl+B, etc.) and OS-level bindings. They are configurable through the `shortcuts:` block in your config file and editable from the built-in settings panel.
Duplicate or overlapping bindings are rejected, including near-conflicts such as `Ctrl+B` and `Ctrl+Shift+B`.

| Shortcut | What it does |
|:---------|:-------------|
| **Ctrl+Shift+K** | Quick-navigate to any workspace |
| **Ctrl+Shift+N** | New terminal panel |
| **Ctrl+Shift+W** | Focus the active workspace at the current zoom |
| **Ctrl+Shift+9** | Fit the active workspace into view |
| **Ctrl+Shift+H** | Open Remote Hosts overlay |
| **Ctrl+Shift+B** | Toggle sidebar |
| **Ctrl+Shift+U** | Toggle HUD |
| **Ctrl+Shift+M** | Toggle minimap |
| **Ctrl+Shift+A** | Align visible attached workspaces into a horizontal row |
| **Ctrl+Shift+,** | Open settings editor |
| **Ctrl+Shift+0** | Reset canvas view |
| **Ctrl+Shift+Plus** | Zoom canvas in |
| **Ctrl+Shift+Minus** | Zoom canvas out |
| **F11** | Fullscreen the active panel |
| **Escape** | Exit active panel fullscreen |
| **Ctrl+Shift+F11** | Toggle window fullscreen |
| **Ctrl+Shift+S** | Save the active Markdown editor |

### Structured Workflow

If you do not want to start by dragging panels around the canvas, use Horizon like this:

1. Create a workspace from the toolbar or with **Ctrl+double-click** on the canvas.
2. Add a terminal from your first preset with **Ctrl+Shift+N**.
3. Jump between workspaces with **Quick Nav** using **Ctrl+Shift+K**.
4. Use **Ctrl+Shift+W** to refocus the current workspace or **Ctrl+Shift+9** to fit it into view.
5. Use the workspace header controls for **Rows**, **Cols**, or **Grid** when you want a structured layout without leaving the canvas.

### Mouse Actions

| Interaction | What it does |
|:------------|:-------------|
| **Middle-mouse drag** | Pan the canvas |
| **Space + Left-click drag** | Pan the canvas |
| **Minimap click-and-drag** | Jump to that area of the canvas |
| **Ctrl+Scroll** | Zoom around the cursor |
| **Ctrl+Click** | Open URL or file path under cursor |
| **Ctrl+double-click** canvas | Create a new workspace |
| **Ctrl+double-click** inside a workspace | Add a new terminal |

<sub>On macOS, substitute Cmd for Ctrl.</sub>

---

## Configuration

The settings editor writes back to the same config file Horizon loaded. By default that is `~/.horizon/config.yaml`, and `config.yml` is also supported when discovered or passed explicitly. You can define workspaces, panel presets, feature flags, and keyboard shortcuts:

```yaml
shortcuts:
  command_palette: Ctrl+Shift+K
  new_terminal: Ctrl+Shift+N
  focus_active_workspace: Ctrl+Shift+W
  fit_active_workspace: Ctrl+Shift+9
  open_remote_hosts: Ctrl+Shift+H
  toggle_sidebar: Ctrl+Shift+B
  toggle_hud: Ctrl+Shift+U
  toggle_minimap: Ctrl+Shift+M
  align_workspaces_horizontally: Ctrl+Shift+A
  toggle_settings: Ctrl+Shift+Comma
  reset_view: Ctrl+Shift+0
  zoom_in: Ctrl+Shift+Plus
  zoom_out: Ctrl+Shift+Minus
  fullscreen_panel: F11
  exit_fullscreen_panel: Escape
  fullscreen_window: Ctrl+Shift+F11
  save_editor: Ctrl+Shift+S
  search: Ctrl+Shift+F

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

# Optional: disable the default attention feed
features:
  attention_feed: false
```

Use key names like `Plus`, `Minus`, `Comma`, `Escape`, and `F11` in YAML instead of punctuation-only shortcut components such as `Ctrl++`.

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
Release instructions live in [**docs/release-flow.md**](docs/release-flow.md).
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
