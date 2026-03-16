# Horizon — Agent Guidelines

> **Source of truth** for all contributors and AI agents working on this project.

## Project Overview

**Horizon** is a GPU-accelerated terminal board — a visual workspace for managing
multiple terminal sessions as freely positioned, resizable panels on a canvas.

**Stack:** Rust (edition 2024) · eframe/egui (wgpu backend) · vt100 · portable-pty

## Workspace Layout

```
crates/
  horizon-core/       Core: terminal emulation, PTY, board & panel management
  horizon-ui/         Binary: eframe application, UI rendering, input handling
```

### horizon-core

- `error.rs` — Typed error enum via thiserror
- `terminal.rs` — vt100 parser wrapper (screen buffer, resize)
- `panel.rs` — Panel = terminal + PTY session + identity
- `board.rs` — Board = collection of panels + focus management

### horizon-ui

- `main.rs` — Entry point, tracing init, eframe launch
- `app/` — `eframe::App` orchestration split by canvas, panels, sidebar, settings, session, persistence
- `terminal_widget/` — Terminal widget split by layout, input, render, scrollbar logic
- `input/` — Keyboard translation, mouse reporting, escape-sequence building
- `theme.rs` — Color palette (Catppuccin Mocha), styling constants

## Development Workflow

### Pre-push validation (all must pass)

```bash
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --workspace --lib --bins -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic
```

### Code Quality Bar

- Self-documenting code preferred over comments
- Typed error enums (thiserror) — no `Box<dyn Error>` or `.unwrap()` in library code
- `#![forbid(unsafe_code)]` on all crates
- Consolidate repeated helpers into shared modules in horizon-core
- Keep new or edited modules single-purpose; avoid mixing rendering, persistence, session bootstrap, and filesystem logic in one file
- If UI code needs shared layout math, state conversion, or template-sync logic, move it into `horizon-core` instead of duplicating it in `horizon-ui`
- Treat roughly 600 lines as the point to split a Rust source file; the CI guardrail fails non-test files above 1000 lines under `crates/horizon-core/src` and `crates/horizon-ui/src`
- Do not use `#[allow(clippy::too_many_lines)]` in core or UI source files; decompose the code instead
- Keep inline `#[cfg(test)]` modules at the end of the file so maintainability checks can measure production code cleanly
- Minimize allocations in the render hot path (per-frame code)
- Every `unsafe` block (if ever needed) must have a `// SAFETY:` rationale

### Maintainability Rules

- Prefer small module trees over large flat files: `mod.rs` should orchestrate, leaf modules should do one job
- UI modules render or collect UI actions; domain state mutation belongs in `horizon-core` unless it is purely presentational state
- When editing a file that is already large, split it as part of the change instead of adding another responsibility
- Keep architecture notes current in [`docs/architecture/maintainability.md`](docs/architecture/maintainability.md) when module boundaries or guardrails change

### CI Tiers (`.github/workflows/ci.yml`)

| Tier | Command | Status |
|------|---------|--------|
| Blocking | `cargo clippy --all-targets --all-features -- -D warnings` | Must pass |
| Strict | `cargo clippy --workspace --lib --bins -- -D warnings -D clippy::unwrap_used -D clippy::expect_used` | Must pass |
| Pedantic | `cargo clippy ... -W clippy::pedantic` | Advisory (will promote) |

### Commit Guidelines

- Concise imperative messages, optionally scoped: `feat(board):`, `fix(render):`, `ci:`
- One logical change per commit
- PRs include: purpose, behavior impact, test evidence

### Dependencies

- Always check crates.io for the latest stable version before adding
- Prefer workspace-level dependencies (root `Cargo.toml`)
- New dependencies require justification

### Testing

- Unit tests close to code (`#[cfg(test)]`)
- Integration tests under `crates/*/tests/`
- Test panel creation, PTY lifecycle, resize, input routing
- For UI/layout changes, verify with a live screenshot after launch and after resize/fit interactions; build success alone is not sufficient

### UI Launch Troubleshooting

- If Horizon "doesn't launch", first distinguish a crash from an unmapped window: `ps -C horizon` then `xwininfo -root -tree | rg Horizon`
- When `xwininfo -id <window-id> -stats` reports `Map State: IsUnMapped`, the process created a root window but the desktop never surfaced it; inspect first-frame UI/input code before blaming PTY startup
- When the map state is `IsViewable`, treat it as a focus, placement, or window-manager issue instead of a launch failure

## Architecture Notes

### Threading Model

- **Main thread:** egui event loop + rendering
- **Per-panel reader thread:** reads PTY output, sends via `mpsc::channel`
- **Input:** main thread writes directly to PTY stdin

### Data Flow

```
Shell → PTY slave → PTY master reader → [thread] → channel → main thread → vt100 → egui
Keyboard → main thread → PTY master writer → PTY slave → Shell
```

### Panel Lifecycle

1. `Board::create_panel()` opens a PTY, spawns `$SHELL`
2. Reader thread continuously sends output chunks to main thread
3. Each frame: drain channel → feed vt100 parser → render grid
4. On resize: recalculate rows/cols → resize vt100 + PTY
5. On close: drop Panel (PTY handles cleaned up automatically)
