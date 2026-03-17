# Maintainability Guardrails

This document describes the project boundaries that keep Horizon from drifting
back into large multi-purpose modules.

## Module Boundaries

### `horizon-core`

- Owns board state, workspace metadata, panel lifecycle, persistence
  projections, and shared layout math.
- `board.rs` should stay orchestration-focused, with board-local submodules for
  attention flows, workspace and panel membership changes, arrangement/collision
  logic, geometry queries, and shutdown state.
- `runtime_state.rs` should stay focused on persisted board/window state; agent
  session discovery and external-store parsing belong in `runtime_state/`
  helper modules.
- Shared domain helpers belong here when both core and UI need them.
- If a UI feature needs to reconstruct runtime state, sync template-backed
  workspace metadata, or format panel/workspace domain labels, prefer adding a
  core API instead of rebuilding that logic in `horizon-ui`.

### `horizon-ui`

- Owns rendering, egui interaction, transient view state, and deferred UI
  actions.
- `app/mod.rs` orchestrates frame flow only.
- `app/` leaf modules stay focused:
  - `canvas`: canvas rendering and HUD
  - `lifecycle`: frame orchestration, shutdown flow, and repaint pacing
  - `panel_chrome`: panel titlebar chrome, badges, context menus, and rename UI
  - `panels`: panel-area orchestration and body rendering
  - `sidebar`: sidebar rendering and deferred sidebar actions
  - `settings`: settings editor state and save/apply flows
  - `session`: startup bootstrap and session catalog/rebind flows
  - `persistence`: runtime/config save glue
  - `workspace`: workspace frame rendering and rename/drag UI
- `input/` and `terminal_widget/` follow the same rule: split event
  translation, layout, rendering, and behavior helpers into dedicated modules
  instead of extending a single file.

## File Size Policy

- Start splitting a Rust source file before it reaches roughly 600 lines.
- CI fails non-test Rust source files above 1000 lines in:
  - `crates/horizon-core/src`
  - `crates/horizon-ui/src`
- Inline `#[cfg(test)]` modules should stay at the end of the file; the line
  limit is measured on the production-code portion before that block.
- `#[allow(clippy::too_many_lines)]` is not an acceptable substitute for
  decomposition in those source trees.

## Review Heuristics

Use these checks during implementation and review:

- Does this file have one reason to change?
- Is any shared logic duplicated across UI and core?
- Is render code mutating domain state directly when it could emit a deferred
  action instead?
- Is a module tree clearer than one more helper stuffed into the current file?

If the answer to any of those is "yes", split or move the code in the same
change.
