# Maintainability Guardrails

This document describes the project boundaries that keep Horizon from drifting
back into large multi-purpose modules.

## Module Boundaries

### Remote worker image

- `containers/remote-worker/panel-session.py` owns the worker-side one-shot task
  marker and verified tmux status/attachment contract. Its dedicated `tmux.conf`
  retains sessions independently of clients. Provider lifecycle, repository
  transfer, durable backup and local panel integration remain separate concerns.
- `containers/remote-worker/host-identity.py` publishes versioned startup readiness
  for the access-bound SSH identity and private retained panel-state root. Task markers live beside the
  identity on workspace storage; sockets remain runtime-only. Missing or legacy
  readiness fails closed, and lost processes are not restarted from their markers.
  Storage migration, process recovery and checkpointing are separate boundaries.

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
- `config.rs` owns configuration loading, validation and aggregate settings.
  Preset models, panel-option conversion and existing default/migration helpers
  live in `config/presets.rs`, with stable public re-exports from the parent.
- `board.rs` should stay orchestration-focused, with board-local submodules for
  attention flows, agent working-status detection, workspace and panel
  membership changes, arrangement/collision logic, geometry queries, and
  shutdown state. Preset slot collision and swapping lives in
  `board/arrangement/reordering.rs`.
- Large board test surfaces should live in `board/tests/` topic files so
  `board.rs` can stay focused on production orchestration.
- `panel.rs` owns panel models and content access; explicit restart logic lives
  in `panel/lifecycle.rs`. `panel/spawn.rs` keeps content selection and command
  resolution, while `panel/spawn/terminal.rs` owns terminal construction,
  transcript restoration and disconnected/failure snapshots. Remote client views
  retain an execution reference independently of visual workspace membership;
  both creation and restart defer remote attachment without local task fallback.
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
  Runtime v3 carries opaque owner-session/workspace references through board
  saves and session copies. Remote-bearing snapshots require complete stable IDs
  before migration, save or restore; they never repair missing remote identities.
  A copied reference does not authorize its claimed owner. Future reconciliation
  must validate the actual resolved session against the exact-owner store.
  Remote panels are SSH client views; task kinds and remote agent-native resumes
  remain in `remote_workspace/`, separate from local agent catalog/binding logic.
  Binding validation and assignment live in
  `runtime_state/binding_bootstrap.rs`; provider-specific session-store parsing
  belongs in focused leaves such as `runtime_state/agent_sessions/codex.rs`.
- `local_store.rs` centralizes agent-store environment paths and read-only
  SQLite opening so discovery, validation, and usage reporting agree.
- `remote_workspace/` owns the versioned remote-workspace aggregate, desired
  panels, exact runtime generation, and repository checkpoint metadata.
  Its validation is pure: provider I/O, runtime-state migration, coordination,
  repository transfer, and UI integration belong in later focused modules.
- `remote_provider_config.rs` owns explicit non-secret provider profiles, empty
  defaults, exact lookup and redacted validation. The main configuration delegates
  to it; local profile construction shares target-name and local-endpoint rules.
  Configuration loading never selects an ambient daemon or performs provider I/O.
- `remote_ssh_identity.rs` exposes retained local client-key preparation and strict
  recovery, separately from the pure remote aggregate. Linux filesystem privacy
  and durable publication live in `remote_ssh_identity/linux.rs`; bounded key-utility
  execution lives in `command.rs`. Neither owns provider or remote task lifetime.
- `remote_workspace_recovery.rs` joins owned allocations, retained client keys and
  non-creating provider inspection. Its result is not task/attachment authority.
  `cloud_run/store/remote_allocations/recovery.rs` atomically checks both snapshots
  before retaining exact worker/host identity and marking reconciliation; neither
  absence nor client drop grants creation, restart or remote cleanup.
- `remote_workspace_setup.rs` sequences explicit allocation, retained private
  identity, public request reservation and fenced provider ensure. Its store
  admission leaf checks exact snapshots without granting creation authority;
  claimed, observed or expired retries use non-creating recovery. Setup remains
  separate from attachment, task readiness and explicit remote management.
- `remote_environment_observation.rs` produces overview-safe, point-in-time
  provider observations with exact snapshot checks but no private-key access or
  saved-state writes. It shares allocation observation validation with recovery;
  neither absence nor observed readiness grants management or attachment authority.
- `remote_worker_status.rs` gates non-creating panel inspection on exact owned
  recovery and current lifetime. Its `protocol.rs` leaf owns bounded status-only
  wire types; `ssh.rs` isolates host pins and client options; `command.rs` owns
  bounded nonblocking local-child I/O. It neither grants attachment/task startup
  nor changes the general user-configured SSH API or remote execution lifetime.
  Explicit saved shell/command verification reuses those gates and compares
  literal argv plus the effective directory with the worker's retained intent;
  unresolved agent launch/handoff/resume semantics stay fail-closed.
- `repository_overlay/` owns bounded exact-base metadata for separate index and
  working-tree changes. Its `paths.rs` applies the lexical transfer exclusion policy.
  Planning performs no filesystem, Git, provider or transfer I/O; actual capture/apply
  must independently validate approval, real node/link topology and content hashes.
  Its separate `reader/` boundary reads one explicitly selected node under a pinned
  Linux directory using kernel confinement, byte limits and change checks. It does
  not enumerate repositories, follow link targets, hash, authorize or transfer data;
  unsupported platforms fail closed without a weaker filesystem fallback.
  Its `bundle/` boundary owns a complete bounded set of SHA-256-verified file
  payloads and fingerprints exact two-layer metadata. Missing, extra, duplicate
  or length-inconsistent payloads fail before a bundle is returned. It performs
  no I/O and grants no capture, export or recovery authority; streaming large
  overlays, coherent capture and safe materialization remain separate concerns.
  `bundle/codec/` frames portable versioned bytes with bounded, constructor-checked
  metadata decoding, canonical ordering and recomputed payload hashes. It does not
  perform I/O or authorize export, extraction or repository writes.
  `bundle/store/` explicitly persists immutable digest-named records in a nominated
  private Linux directory, reusing the reader's pinned inode checks. Anonymous
  writes, no-replace publication and file/directory synchronization precede success;
  retrieval revalidates the complete codec and digest. This is not scheduled backup.
  `capture/` reads only nominated paths from a pinned Linux Git worktree into a
  verified bundle, preserving literal index and working bytes separately. It checks
  exact HEAD, selected index state and root association without filters or writes;
  coherent snapshots, selection/export approval and materialization remain separate.
- `containers/remote-worker/host-identity.py` owns workspace-retained server-key
  initialization, validation and runtime materialization before SSH starts.
  Its real-key regressions are separate from the retained-volume SSH smoke in
  `scripts/run-remote-worker-host-identity-smoke.sh`. Neither owns volume allocation,
  backup, provider restart or task supervision.
- `cloud_run/worker_lifetime.rs` owns explicit execution lifetime and compatible
  target serialization. `interactive_worker.rs` validates observed lifetime
  against that policy; neither represents the client/creation ownership lease.
  Missing or malformed legacy metadata never selects persistent execution.
- The local worker adapter's `local_docker/creation.rs` uses the existing durable
  workflow store to grant at most one creation attempt. A consumed grant permits
  only non-creating reconciliation; store failure, client drop or resource loss
  never renews it. Explicit ensure also consumes the grant when accepting an
  exact existing worker, before observing its connection. Provider construction
  requires that store explicitly; non-creating inspection never claims a grant.
- `cloud_run/interactive_worker_stop.rs` is an opt-in Stop contract, separate from
  deletion and client lifetime. The local adapter's `local_docker/stop.rs` verifies
  exact ownership and disabled automatic removal before a bounded stop, then
  verifies the same resource is retained and inactive. Lost responses need that
  independent proof; absence is not retained storage. Durable management intent,
  checkpointing and user-facing actions remain separate caller responsibilities.
- `cloud_run/runpod.rs` coordinates provider operations and exact ownership
  reconciliation. Its `models.rs` leaf owns profiles, persisted worker identity,
  lifecycle results and typed errors; `create_request.rs` owns serialized
  creation-request construction. Public type re-exports remain stable. HTTP
  transport and common interactive-worker adaptation stay in their existing
  `http.rs` and `interactive.rs` leaves.
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
- `app/bootstrap.rs` constructs the initial application state and configures
  startup-only fonts and install discovery. It does not own per-frame polling,
  provider actions, or remote execution lifetime.
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
  - `remote_environments`: single-flight saved-inventory loading and modal input
    ownership, with cached labels and rendering in `remote_environments/paint`.
    Compact record projection belongs to `horizon-core::remote_workspace::summary`;
    the overview does not own provider actions or remote execution lifetime.
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
