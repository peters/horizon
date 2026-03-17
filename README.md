<p align="center">
  <img src="assets/icons/icon-256.png" alt="Horizon" width="128" />
</p>

<h1 align="center">Horizon</h1>

<p align="center">
  <strong>A GPU-accelerated terminal board for managing multiple sessions on an infinite canvas.</strong>
</p>

<p align="center">
  <a href="https://github.com/peters/horizon/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/peters/horizon?style=flat-square&color=74a2f7" /></a>
  <a href="https://github.com/peters/horizon/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/peters/horizon/ci.yml?branch=main&style=flat-square&label=CI" /></a>
  <img alt="License" src="https://img.shields.io/badge/license-MIT-a6e3a1?style=flat-square" />
  <img alt="Platform" src="https://img.shields.io/badge/platform-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows-cba6f7?style=flat-square" />
</p>

---

Horizon gives you an infinite 2D canvas where terminal sessions live as freely positioned, resizable panels. Organize them into workspaces, arrange them however you like, and see everything at once with the minimap. Built with Rust and GPU-rendered via wgpu.

## Features

**Infinite canvas** -- Pan, zoom, and arrange terminals anywhere on a boundless surface. A minimap keeps you oriented.

**Workspaces** -- Group related panels into color-coded workspaces. Five auto-layout modes (rows, columns, grid, stack, cascade) or go freeform.

**Full terminal emulation** -- 24-bit color, mouse reporting, scrollback history, Kitty keyboard protocol, and alt-screen support powered by the Alacritty terminal engine.

**Smart detection** -- Ctrl+click URLs to open them in a browser. Hover file paths to open them in your editor.

**Built-in agent panels** -- First-class Claude Code and Codex integration with session persistence, auto-resume, and a live usage dashboard.

**Git integration** -- Real-time git status panel with inline diffs, file grouping, and background watching.

**Markdown editor** -- Drag-and-drop `.md` files onto the canvas to edit with live preview.

**Settings with live preview** -- Edit your YAML config in a side panel with syntax highlighting. Changes apply instantly.

**Session persistence** -- Close and reopen Horizon -- your workspaces, panel positions, and terminal history are restored.

**Attention feed** -- Get notified when panels need your attention without constantly watching every terminal.

## Install

### Download a release binary

Grab the latest binary from [Releases](https://github.com/peters/horizon/releases/latest). No dependencies required.

| Platform | Asset |
|----------|-------|
| Linux x64 | `horizon-linux-x64.tar.gz` |
| macOS x64 | `horizon-osx-x64.tar.gz` |
| Windows x64 | `horizon-windows-x64.exe` |

### Build from source

Requires [Rust](https://rustup.rs) stable 1.85+. On Linux, install system headers first (see [AGENTS.md](AGENTS.md#option-b--build-from-source) for per-distro commands).

```bash
git clone https://github.com/peters/horizon.git
cd horizon
cargo run --release
```

## Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+N` | New terminal panel |
| `Ctrl+K` | Quick workspace navigator |
| `Ctrl+B` | Toggle sidebar |
| `Ctrl+M` | Toggle minimap |
| `Ctrl+,` | Open settings |
| `Ctrl+0` | Reset canvas view |
| `F11` | Fullscreen active panel |
| `Ctrl+F11` | Toggle window fullscreen |
| `Ctrl+Click` | Open URL or file path |

<sub>On macOS, use Cmd instead of Ctrl.</sub>

## Configuration

Horizon stores its config at `~/.config/horizon/config.yaml`. Open the built-in editor with **Ctrl+,** to edit with syntax highlighting and live preview.

```yaml
workspaces:
  - name: Dev
    cwd: ~/projects/myapp
    panels:
      - kind: shell
      - kind: claude
      - kind: git_changes

presets:
  - name: Shell
    alias: sh
    kind: shell
  - name: Claude Code
    alias: cc
    kind: claude
```

## Tech stack

| Component | Role |
|-----------|------|
| [Rust](https://www.rust-lang.org) (edition 2024) | Language |
| [eframe](https://github.com/emilk/egui/tree/master/crates/eframe) / [egui](https://github.com/emilk/egui) | UI framework |
| [wgpu](https://wgpu.rs) | GPU rendering (Vulkan, Metal, DX12, OpenGL) |
| [alacritty_terminal](https://github.com/alacritty/alacritty) | Terminal emulation |
| [Catppuccin Mocha](https://catppuccin.com) | Color palette |

## Contributing

See [AGENTS.md](AGENTS.md) for development setup, coding standards, and CI requirements.

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
```

## License

[MIT](Cargo.toml)
