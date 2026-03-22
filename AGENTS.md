# Horizon — Agent Guidelines

> **Source of truth** for all contributors and AI agents working on this project.

## Quick Start

An AI agent given these instructions should be able to go from zero to a running Horizon binary.

**Repository:** `https://github.com/peters/horizon.git`

### Option A — Download a release binary (fastest)

Pre-built binaries are attached to GitHub releases. Prefer the latest **non-prerelease** tag. Use `gh release download --repo peters/horizon` or download from `https://github.com/peters/horizon/releases/latest`.

| Platform | Asset | Contents |
|----------|-------|----------|
| Linux x64 | `horizon-linux-x64.tar.gz` | Single `horizon` binary — extract and make executable |
| macOS x64 | `horizon-osx-x64.tar.gz` | Single `horizon` binary — extract and make executable |
| Windows x64 | `horizon-windows-x64.exe` | Ready-to-run executable |

No Rust toolchain or system headers are needed for this path.

### Option B — Build from source

#### Prerequisites

- **Rust stable ≥ 1.88** (edition 2024). Install via [rustup](https://rustup.rs) if not present.
- **Linux only:** the eframe/wgpu rendering stack needs system headers. Install them before `cargo build`:
  - Debian/Ubuntu: `sudo apt install -y build-essential pkg-config libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libvulkan-dev libgl-dev cmake`
  - Fedora: `sudo dnf install -y gcc pkg-config wayland-devel libxkbcommon-devel vulkan-loader-devel mesa-libGL-devel cmake`
  - Arch: `sudo pacman -S --needed base-devel wayland libxkbcommon vulkan-icd-loader cmake`
- **macOS:** Xcode Command Line Tools (`xcode-select --install`). Metal ships with the OS.
- **Windows:** MSVC build tools (installed automatically by `rustup` on the `msvc` target). DX12/Vulkan drivers ship with the GPU driver.

#### Build & Run

```bash
git clone https://github.com/peters/horizon.git
cd horizon
cargo run --release
```

#### Verify

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
```

If any step fails, read the error, fix the prerequisite, and retry. On Linux, missing system headers are the most common issue — look for `pkg-config` or linker errors and install the corresponding `-dev` package.

## Project Overview

**Horizon** is a GPU-accelerated terminal board — a visual workspace for managing
multiple terminal sessions as freely positioned, resizable panels on a canvas.

**Stack:** Rust (edition 2024) · eframe/egui (wgpu backend) · alacritty_terminal (VT parsing, PTY, event loop)

## Workspace Layout

```
crates/
  horizon-core/       Core: terminal emulation, PTY, board & panel management
  horizon-ui/         Binary: eframe application, UI rendering, input handling
```

### horizon-core

- `error.rs` — Typed error enum via thiserror
- `terminal.rs` + `terminal/` — alacritty_terminal wrapper split by lifecycle, event handling, resize policy, selection, replay, and support helpers
- `panel.rs` — Panel = terminal + PTY session + identity
- `board.rs` + `board/tests/` — Board orchestration and colocated behavior-focused tests

### horizon-ui

- `main.rs` — Entry point, tracing init, eframe launch
- `app/` — `eframe::App` orchestration split by canvas, panels, sidebar, settings, session, persistence, workspace helpers, and focused action submodules
- `remote_hosts_overlay.rs` + `remote_hosts_overlay/` — remote SSH chooser state/input shell with dedicated query, layout, and paint helpers
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
cargo clippy --workspace --lib --bins --examples -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic
```

- Run the validation commands in the exact checkout you will push. If you split work across branches or `git worktree`s, rerun the blocking and strict clippy tiers in each final branch/worktree after applying the split, not only in the original combined checkout.

### Configuration Changes

- When changing default presets, CLI flags, or any config-related code in `horizon-core/src/config.rs`, always sync the user's local config file (`~/.horizon/config.yaml`) to match

### Code Quality Bar

- Self-documenting code preferred over comments; add comments only for invariants, non-obvious tradeoffs, or safety contracts
- Use idiomatic Rust naming: `snake_case` (functions/modules), `CamelCase` (types), `SCREAMING_SNAKE_CASE` (consts)
- Typed error enums (thiserror) — no `Box<dyn Error>` or `.unwrap()` in library code
- Keep APIs explicit: prefer `Result<T, E>` and typed structs/enums over ad-hoc tuples
- Prefer `tracing` for structured logging
- `#![forbid(unsafe_code)]` on all crates
- Consolidate repeated helpers into shared modules in horizon-core
- Keep new or edited modules single-purpose; avoid mixing rendering, persistence, session bootstrap, and filesystem logic in one file
- If UI code needs shared layout math, state conversion, or template-sync logic, move it into `horizon-core` instead of duplicating it in `horizon-ui`
- Treat roughly 600 lines as the point to split a Rust source file; the CI guardrail fails non-test files above 1000 lines under `crates/horizon-core/src` and `crates/horizon-ui/src`
- Do not use `#[allow(clippy::too_many_lines)]` in core or UI source files; decompose the code instead
- Keep inline `#[cfg(test)]` modules at the end of the file so maintainability checks can measure production code cleanly
- Minimize allocations in the render hot path (per-frame code)
- Every `unsafe` block (if ever needed) must have a `// SAFETY:` rationale
- Avoid unnecessary `crate::` path prefixes in module-local code/tests when imports already provide the item
- Prefer `unwrap_or_else(std::sync::PoisonError::into_inner)` over manual `match` for poisoned mutex recovery
- `expect_used` is treated the same as `unwrap_used`: both can hide panic paths in runtime code
- Fix clippy/rustc warnings instead of suppressing them

### Maintainability Rules

- Prefer small module trees over large flat files: `mod.rs` should orchestrate, leaf modules should do one job
- UI modules render or collect UI actions; domain state mutation belongs in `horizon-core` unless it is purely presentational state
- When editing a file that is already large, split it as part of the change instead of adding another responsibility
- When an inline test block starts dominating a source file, move it into a colocated `module/tests/` tree split by behavior instead of letting the parent file keep growing
- Keep architecture notes current in [`docs/architecture/maintainability.md`](docs/architecture/maintainability.md) when module boundaries or guardrails change

### CI Tiers (`.github/workflows/ci.yml`)

| Tier | Command | Status |
|------|---------|--------|
| Blocking | `cargo clippy --all-targets --all-features -- -D warnings` | Must pass |
| Strict | `cargo clippy --workspace --lib --bins --examples -- -D warnings -D clippy::unwrap_used -D clippy::expect_used` | Must pass |
| Pedantic | `cargo clippy ... -W clippy::pedantic` | Advisory (will promote) |

### Commit Guidelines

- Concise imperative messages, optionally scoped: `feat(board):`, `fix(render):`, `ci:`
- One logical change per commit
- PRs include: purpose, behavior impact, test evidence
- Always fix pre-existing clippy warnings in touched files before committing; a commit must leave the blocking and strict CI tiers green in the exact branch/worktree that will be pushed for review

### Versioning

- The authoritative base version lives in `Cargo.toml` under `[workspace.package].version`
- The `horizon-core` workspace dependency version in `Cargo.toml` must match the workspace package version
- CI validates this with `scripts/check-version-sync.sh`

### Release Flow

Releases are tag-driven and documented in [`docs/release-flow.md`](docs/release-flow.md).

- Use `./scripts/next-version.sh alpha`, `./scripts/next-version.sh beta`, or `./scripts/next-version.sh stable` to suggest the next tag for the current release line
- Publish from **GitHub → Releases** using tags like `vX.Y.Z-alpha.N`, `vX.Y.Z-beta.N`, or `vX.Y.Z`
- Mark alpha and beta tags as prereleases in GitHub; leave stable tags as normal releases
- Publishing the GitHub Release triggers CI to publish the crates and upload the release binaries
- After a stable release, bump `Cargo.toml` to the next release line in a normal PR before cutting more prereleases

### Dependencies

- Always check crates.io for the latest stable version before adding
- Prefer workspace-level dependencies (root `Cargo.toml`)
- New dependencies require justification

### Testing

- Unit tests close to code (`#[cfg(test)]`)
- Integration tests under `crates/*/tests/`
- Test panel creation, PTY lifecycle, resize, input routing
- For UI/layout changes, verify with a live screenshot after launch and after resize/fit interactions; build success alone is not sufficient
- For any UI-related change, always create an extensive temporary smoke-test plan under `docs/testing/` that another agent or machine can execute without extra context. Cover baseline behavior, primary flows, edge cases, persistence/migration, and visual regressions.
- Temporary smoke-test plans are validation artifacts, not permanent docs. Delete them after the UI validation pass is complete unless the user explicitly asks to keep them.

### Smoke Test Reliability

- Treat motion-sensitive UI bugs as motion-sensitive validation problems: detached-window drag, resize feedback, hover jitter, and oscillation bugs are not reliably caught by still screenshots alone
- When a bug depends on live movement, capture either a short video (`screencapture -V <seconds>` on macOS) or a high-frequency native window position trace during an automated drag; use screenshots only as supporting evidence
- If multiple Horizon processes may be running, scope automation and inspection to the exact PID under test. Do not target windows by application name alone; mixed-process sampling can produce false passes and false failures
- For detached-window smoke tests, verify the exact path being fixed. If the bug is in restore/relaunch behavior, seed or inspect the persisted runtime state and relaunch into that state instead of only testing fresh detach flows
- Prefer repeatable native-window assertions over visual guesswork: sample outer window position before, during, and after drag/resize to confirm the window moves monotonically and does not snap back, alternate between coordinates, or keep replaying a saved restore position
- Validate the exact branch or commit that will be pushed. If you use a merge-test worktree or disposable checkout for diagnosis, rerun the decisive smoke pass in the final branch/worktree before concluding the PR is fixed
- Window titles may lag behind state changes or differ across restore paths. When automating detached-window tests, identify the root and detached windows by PID plus non-root window membership rather than by title string alone when possible

### Performance Profiling

- Prefer repeatable workloads over ad-hoc observation: profile idle, panning, mouse-move, resize, and scroll as separate cases instead of treating "high CPU" as one bucket
- Start with `cargo build --profile profiling --features trace-profiling`, then use `HORIZON_TRACE_SPANS=1 RUST_LOG=info target/profiling/horizon` or `scripts/profile.sh <seconds>`; the script already falls back to traced spans when `perf` is blocked
- Keep before/after comparisons workload-matched: same board layout, same interaction script, same binary profile, same tracing mode, and the same isolated runtime state (`HOME`, config, and session inputs)
- Compare normalized span costs, not just total wall time: use per-call `avg_us` for key spans such as `horizon::app::update`, `horizon::app::lifecycle::render_active_view`, `horizon::app::panels::render_panels`, and `egui::context::pass` when frame counts differ between runs
- When profiling pointer-driven regressions, automate the interaction with native tooling on the host OS instead of relying on manual movement: `xdotool` on X11, AppleScript/Quartz or equivalent on macOS, and Win32/UIA tooling on Windows
- Scope automation and inspection to the exact PID under test when multiple Horizon processes may exist; avoid targeting windows by application name alone
- Keep interaction workloads separate from terminal-output workloads so PTY redraw noise does not mask pointer or layout regressions
- Treat short motion traces as provisional. If a resize or pointer regression only appears in a small sample, rerun it with a denser or longer scripted interaction before changing code
- For UI perf changes, capture a live screenshot after the profiled interaction as well as at launch; some "wins" are just incorrect culling
- Keep temporary perf harnesses, generated configs, and trace logs outside the repo unless the user explicitly asks to preserve them

### Typical Hotspots

- `crates/horizon-ui/src/app/panels.rs` — panel composition, titlebar chrome, history meter, hover work, and context-menu setup
- `crates/horizon-ui/src/terminal_widget/render.rs` — per-cell iteration, text batching, default-color conversion, decorations, and redundant fills
- `crates/horizon-ui/src/terminal_widget/input.rs` — per-panel event cloning/scanning and pointer-move fan-out
- `crates/horizon-ui/src/app/workspace.rs` and `crates/horizon-ui/src/app/canvas.rs` — workspace hover effects, cursor changes, and canvas redraws while moving the mouse
- `crates/horizon-ui/src/app/canvas.rs` and the minimap path — repeated static geometry such as dot grids, glows, and overview rects can become tessellation-heavy under pointer-only redraws
- Mouse-move spikes are often hover/redraw problems, not PTY/output problems; compare against an idle trace before touching terminal I/O
- Moving the pointer anywhere inside the Horizon window can trigger full-scene redraws; "empty canvas" is not a free case if visible panels still repaint

### Performance Guardrails

- Safe optimizations usually reduce repeated work on the common path: batch default terminal text, fast-path default colors, gate per-panel pointer processing behind actual interaction, and remove redundant paint passes
- Off-screen culling is generally safe; active-workspace culling is not. Do not skip panel body rendering just because a panel belongs to a non-active workspace if it can still be visible on screen
- If a perf change affects visibility, focus, hover, or selection behavior, treat it as correctness-sensitive and verify it live before keeping it
- Record the exact workload used for any reported perf gain in the commit message or PR notes so later agents can reproduce it

### UI Feature Perf Checklist

- Any new UI feature must identify its redraw surface up front: what pointer movement, hover state, animation, terminal output, or config changes will cause it to update
- Treat pointer-only frames as a first-class perf budget; if a feature adds hover behavior, compare idle vs scripted mouse-move before and after, including a pass over empty canvas outside any workspace and a pass over visible panel chrome
- Do not add unconditional per-frame work across every panel or workspace for convenience. Compute lazily on interaction, gate by on-screen visibility, or cache by stable keys
- Prefer cached meshes or cached shapes for repeated static decoration such as grids, minimaps, badges, panel chrome, and other geometry that does not semantically change every frame
- Avoid broad `request_repaint` or animation loops for passive UI polish. Repaint continuously only when there is active interaction, active animation, or real terminal/output change
- New menus, tooltips, badges, and summary labels should avoid eager text layout, string building, or list construction for every visible panel each frame
- If a feature needs per-panel pointer inspection, keep the hot path narrow: no cloning/scanning full input state for inactive panels unless the pointer is actually relevant to that panel
- When a feature adds a new always-visible overlay, include the overlay in the perf trace and live screenshot verification; overlays can dominate pointer redraw cost even when the cursor is elsewhere
- If a feature cannot be made lazy or cache-friendly, document why in the commit message or PR notes and include the measured cost on the target workload

### UI Launch Troubleshooting

- If Horizon "doesn't launch", first distinguish a crash from an unmapped window: `ps -C horizon` then `xwininfo -root -tree | rg Horizon`
- When `xwininfo -id <window-id> -stats` reports `Map State: IsUnMapped`, the process created a root window but the desktop never surfaced it; inspect first-frame UI/input code before blaming PTY startup
- When the map state is `IsViewable`, treat it as a focus, placement, or window-manager issue instead of a launch failure

## Architecture Notes

### Threading Model

- **Main thread:** egui event loop + rendering
- **Per-panel event loop thread:** alacritty_terminal `EventLoop` reads PTY output, parses VT sequences, and sends events via `mpsc::channel`
- **Input:** main thread writes to PTY via `EventLoopSender`

### Data Flow

```
Shell → PTY slave → PTY master → [alacritty EventLoop thread] → Term (VT parse) → channel → main thread → egui
Keyboard → main thread → EventLoopSender → PTY master → PTY slave → Shell
```

### Panel Lifecycle

1. `Board::create_panel()` opens a PTY via `alacritty_terminal::tty`, spawns `$SHELL`
2. alacritty `EventLoop` thread continuously reads PTY output and updates the `Term` grid
3. Each frame: drain event channel → render from `Term` state → egui
4. On resize: recalculate rows/cols → resize `Term` + PTY
5. On close: drop Panel (PTY handles cleaned up automatically)
