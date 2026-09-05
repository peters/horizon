# Maintainability Guardrails

This document describes the project boundaries that keep Horizon from drifting
back into large multi-purpose modules.

## Module Boundaries

### `horizon-browser-protocol`

- Owns the small serialized contract shared by browser engines and clients:
  backend identifiers/capabilities, input and command values, validated agent
  actions, semantic results, bounded network records, and redacted audit
  entries.
- Depends only on serialization support. It must not acquire process, socket,
  async-runtime, image-decoder, filesystem-coordination, MCP, `horizon-core`,
  or UI dependencies.
- Contains no host policy. Browser launch, persistence, authentication,
  retention, steering ownership, backend input serialization, and presentation
  remain in their owning crates.

### `horizon-browser`

- Owns browser processes, CDP/WebDriver/BiDi transports, frame delivery, and
  deterministic shutdown. It consumes and re-exports the lightweight protocol
  values, and must not depend on `horizon-core`, `horizon-ui`, a GUI toolkit,
  or an async runtime.
- Host-specific IPC, authentication, persistence, and retention stay outside
  the crate behind `BrowserCoordination`.
- `session.rs` orchestrates the Chromium driver; command dispatch, event
  transitions, host coordination, lifecycle, startup, shutdown, and agent
  navigation settlement (`session/navigation.rs`) belong in focused
  `session/` leaves. The backend-neutral pending-navigation state machine
  lives in `navigation.rs` so both drivers settle the same typed outcome.
  Selector waits follow the same shape: the backend-neutral `PendingWait`
  state machine lives in `wait.rs`, and each driver's observation loop glue
  in `session/wait.rs` and `webdriver/session/wait.rs`.
- `webdriver/session.rs` orchestrates Firefox and Safari. Host coordination
  belongs in `webdriver/session/coordination.rs`, synchronous navigation
  outcomes in `webdriver/session/navigation.rs`, and HTTP, action translation,
  and service/process responsibilities stay in their existing WebDriver leaves.

### `horizon-core`

- Owns board state, workspace metadata, panel lifecycle, persistence
  projections, and shared layout math.
- `board.rs` should stay orchestration-focused, with board-local submodules for
  attention flows, agent working-status detection, workspace and panel
  membership changes, arrangement/collision logic, geometry queries, and
  shutdown state. Preset slot collision and swapping lives in
  `board/arrangement/reordering.rs`.
- Large board test surfaces should live in `board/tests/` topic files so
  `board.rs` can stay focused on production orchestration.
- `terminal.rs` should keep the terminal types and shared imports; lifecycle,
  event handling, resize policy, selection logic, and content helpers belong in
  `terminal/` leaf modules.
- `browser/mod.rs` maps engine sessions/events into Horizon panel state and
  retry/teardown behavior. Locked live coordination stays in
  `browser/manifest.rs`, with agent-side lease/action helpers in
  `browser/manifest/agent.rs`, bounded host-routed panel creation in
  `browser/manifest/create.rs`, visibility requests in
  `browser/manifest/visibility.rs`, their shared private queue primitives in
  `browser/manifest/request_queue.rs`, host-stamped workspace membership that
  scopes MCP discovery and control in `browser/manifest/workspace.rs`, and
  append-only audit storage in `browser/manifest/audit.rs`.
- `runtime_state.rs` should stay focused on persisted board/window orchestration.
  Persisted workspace, panel, template, and session-binding models live in
  `runtime_state/models.rs`, with board/workspace and panel persistence tests in
  `runtime_state/tests/`. `runtime_state/versioning.rs` guards schema compatibility
  on read and write serialization boundaries; supported legacy snapshots still
  migrate in memory. Agent binding orchestration, discovery, and external-store
  parsing belong in `runtime_state/` helper modules.
  Binding validation and assignment live in
  `runtime_state/binding_bootstrap.rs`; provider-specific session-store parsing
  belongs in focused leaves such as `runtime_state/agent_sessions/codex.rs`.
- `local_store.rs` centralizes agent-store environment paths and read-only
  SQLite opening so discovery, validation, and usage reporting agree.
- `remote_workspace/` owns the versioned remote-workspace aggregate, desired
  panels, exact runtime generation, and repository checkpoint metadata.
  Its validation is pure: provider I/O, runtime-state migration, coordination,
  repository transfer, and UI integration belong in later focused modules.
- `cloud_run/worker_lifetime.rs` owns explicit execution lifetime and compatible
  target serialization. `interactive_worker.rs` validates observed lifetime
  against that policy; neither represents the client/creation ownership lease.
  Missing or malformed legacy metadata never selects persistent execution.
- `cloud_run/store.rs` owns workflow snapshots and creation-claim transactions.
  Its `cloud_run/store/database.rs` leaf owns private-path preparation, connection policy,
  schema initialization, and compatibility checks. Keep database opening separate
  from domain-specific record operations.
- `cloud_run/store/remote_workspaces.rs` owns validated, session-owned remote
  snapshot storage with exact revisions and bounded recovery. Replacement
  invariants live in its `validation.rs` leaf. These records are independent of
  board snapshots and session-file deletion. Its prepared replacement can
  participate in a caller-owned store transaction without committing it. The
  workflow store's `workflow_writes.rs` binds validated workflow metadata to its
  encoded snapshot before staging insertion, separately from transaction commit;
  provider/workflow coordination and runtime-state references remain separate
  integration responsibilities.
- `cloud_run/store/remote_allocations.rs` atomically binds a runtime generation,
  its single-worker setup workflow and their ownership record. Its `binding.rs`
  leaf performs bounded cross-record recovery; `guards.rs` prevents generic
  record/workflow writes and creation claims from bypassing that binding.
  Recovery preserves expired setup identities without granting new creation.
  These store operations run off the render thread and neither own remote
  execution lifetime nor perform provider actions.
- The remote record store's `creation_fences.rs` leaf owns the schema-four
  migration of legacy runtime identities into append-only creation denials,
  transactional publication alongside every prepared runtime write, exact schema
  validation, and indexed claim checks. Migration reuses the aggregate decoder;
  no denial confers allocation ownership or provider authority. Migration and
  runtime-write regressions live separately under its colocated test tree.
- Shared domain helpers belong here when both core and UI need them.
- If a UI feature needs to reconstruct runtime state, sync template-backed
  workspace metadata, or format panel/workspace domain labels, prefer adding a
  core API instead of rebuilding that logic in `horizon-ui`.

### `horizon-browser-cli`

- Owns deterministic browser plans, bounded execution control, durable job
  lifecycle, explicit resume policy, and user-facing reports.
- `run_state.rs` coordinates lifecycle metadata and exclusive resume leases.
  Large verified step results live in immutable files managed by
  `run_state/checkpoint_artifacts.rs`; `state.json` retains only compact result
  references so intent updates never rewrite prior payloads.

### `horizon-ui`

- Owns rendering, egui interaction, transient view state, and deferred UI
  actions.
- `app/mod.rs` orchestrates frame flow only.
- `app/` leaf modules stay focused:
  - `actions/`: overlay/layout math, panel lifecycle helpers, palette/shortcut
    dispatch, picker flows, and canvas interaction helpers
  - `browser_requests`: transient host polling, panel creation, visibility
    changes for authenticated requests routed from a live agent panel, and the
    host-owned workspace stamp that keeps MCP authorization current
  - `canvas`: canvas rendering and HUD
  - `lifecycle`: frame orchestration and repaint pacing, with application-exit
    ownership and persistence sequencing in `lifecycle/shutdown.rs`
  - `panel_chrome`: panel titlebar chrome, badges, and rename UI
  - `panels`: panel-area orchestration and body rendering, with gesture and
    context-menu handling and outcome application in `panels/interaction.rs`
  - `remote_hosts_overlay`: overlay state/input shell with query/filter,
    layout, and row/header paint helpers split into `remote_hosts_overlay/`
  - `sidebar`: sidebar rendering and deferred sidebar actions
  - `settings`: settings editor state and save/apply flows
  - `session`: startup bootstrap and session catalog/rebind flows, with startup
    result types in `session/types.rs` and loading/recovery rendering in
    `session/loading.rs`
  - `persistence`: runtime/config save glue
  - `view`: canvas pan/zoom state, coordinate transforms, and focus-to-bounds helpers
  - `workspace`: workspace frame orchestration and rename/drag UI, with
    paint/render/toolbar helpers split into `workspace/`
- `input/` and `terminal_widget/` follow the same rule: split event
  translation, layout, rendering, and behavior helpers into dedicated modules
  instead of extending a single file. Browser-widget input keeps frame-level
  coordination in `browser_widget/input.rs`, with independent keyboard/IME and
  pointer-capture state machines in `browser_widget/input/keyboard.rs` and
  `browser_widget/input/pointer.rs`.

## File Size Policy

The automated line-limit and `too_many_lines` suppression checks cover
`horizon-browser-protocol`, `horizon-browser`, `horizon-core`, and
`horizon-ui`; extracting a new crate is not an escape hatch for oversized
modules.

- Start splitting a Rust source file before it reaches roughly 600 lines.
- CI fails non-test Rust source files above 1000 lines in:
  - `crates/horizon-browser-protocol/src`
  - `crates/horizon-browser/src`
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

If the answer to any of those is "yes", follow the
[pull-request scope rules](../../AGENTS.md#pull-request-scope): land purely
mechanical moves in a focused prerequisite PR and keep each semantic change to
one independently testable outcome.
