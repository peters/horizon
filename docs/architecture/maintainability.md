# Maintainability Guardrails

This document describes the project boundaries that keep Horizon from drifting
back into large multi-purpose modules.

## Module Boundaries

Remote-development provisioning, workers, managed SSH views, repository transfer,
and the Remote Environments modal were removed in #693. Ordinary SSH terminals,
Remote Hosts, Sessions, and remote browser settings remain. Runtime v3 references
are retained only as inert compatibility data with command-replay guards until
versioned migration; no remote-development controller consumes them. Config v11
omits obsolete top-level provider profiles while preserving `browser.remote`.

### `horizon-browser-protocol`

- Owns the small serialized contract shared by browser engines and clients:
  backend identifiers/capabilities, input and command values, validated agent
  actions, semantic results, bounded network records, video-capture options,
  and redacted audit entries.
- Depends only on serialization support plus `url` for endpoint
  canonicalization. It must not acquire process, socket, async-runtime,
  image-decoder, filesystem-coordination, MCP, `horizon-core`, or UI
  dependencies.
- `remote` owns the `browser.remote` configuration contract: `provider`
  (validated control endpoint, tagged authentication, credential bindings as
  references only, bounded limits), `target` (normalized device requirement and
  namespaced capability extensions), `error` (identifier-only messages), and
  the orchestrating module for validation, portable export and import. It
  holds no credential values, transport, allocation or store access.
- Contains no host policy. Browser launch, persistence, authentication,
  retention, steering ownership, backend input serialization, and presentation
  remain in their owning crates.

### `horizon-browser`

- Owns browser processes, CDP/WebDriver/BiDi transports, frame delivery,
  optional WebM page-pixel recording (`video/`, enabled by `video-capture`), and
  deterministic shutdown. Default builds omit the AV1 encoder; Horizon and its
  CLI/MCP consumers explicitly enable recording. The standalone CI feature
  matrix guards both configurations against host dependencies.
  It consumes and re-exports the lightweight protocol
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
  `session/viewport.rs` owns explicit CSS viewport policy and bounded resize
  observation. Host sizes are remembered while pinned; reset resumes the latest
  host size. Firefox driver glue lives in `webdriver/session/viewport.rs`. Each
  backend observes from its driver loop, and `FrameSlot` exposes the explicit
  target so renderers can gate pointer input on matching frames.
- `webdriver/session/startup.rs` owns local and remote allocation and BiDi setup.
- `webdriver/session.rs` orchestrates Firefox and Safari. Host coordination
  belongs in `webdriver/session/coordination.rs`, synchronous navigation
  outcomes in `webdriver/session/navigation.rs`, session creation and
  capabilities in `webdriver/session/handshake.rs`, the BiDi link (calls,
  event draining, subscriptions, preload) in `webdriver/session/bidi.rs`,
  adaptive screenshot cadence and page-scroll sampling in
  `webdriver/session/frames.rs`, and HTTP, action translation, and
  service/process responsibilities stay in their existing WebDriver leaves.
- `webdriver/transport.rs` defines the classic command contract
  (`ClassicTransport`) and request-path rules. `webdriver/http.rs` remains the
  loopback-only client for local drivers; `webdriver/remote_http.rs` is the
  separate authenticated HTTPS client for hosted grids (one credential bound
  to one origin, no redirects, bounded bodies, `ureq` with rustls) and maps
  failures onto the same `HttpError` shapes. Neither client knows about
  sessions, panels or providers.

### `horizon-browser-control`

- Owns filesystem browser discovery, ownership leases, handoff, action/result
  queues, host-routed create/close/visibility requests, workspace authorization,
  and redacted audit storage. The `manifest/` leaves preserve the existing
  transaction and retention boundaries.
- `paths.rs` owns the shared default coordination root and canonical identifier
  encoding. Core keeps its app-specific paths while using that same resolver and
  encoder. Explicit path constructors do not override global coordination APIs.
- Depends on the browser engine and small filesystem/serialization helpers.
  It must not depend on core, UI, terminal state, MCP, or agent runtimes. Provider
  credentials, device quotas, Teach, and host panel state stay outside this crate.

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
  creation restores an inert transcript and restart refuses local task fallback.
- `terminal.rs` should keep the terminal types and shared imports; lifecycle,
  event handling, resize policy, selection logic, and content helpers belong in
  `terminal/` leaf modules.
- `browser/mod.rs` maps engine sessions/events into Horizon panel state and
  retry/teardown behavior. `browser/manifest.rs` reexports the shared
  `horizon-browser-control::manifest` implementation so existing host callers
  retain identical types, locks, and process identity.
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
  A copied reference cannot authorize execution or recreate the removed store.
  Retired managed SSH views remain inert for legacy compatibility and cannot
  restart saved commands locally. Ordinary SSH panels remain supported.
  Binding validation and assignment live in
  `runtime_state/binding_bootstrap.rs`; provider-specific session-store parsing
  belongs in focused leaves such as `runtime_state/agent_sessions/codex.rs`.
- `local_store.rs` centralizes agent-store environment paths and read-only
  SQLite opening so discovery, validation, and usage reporting agree.
- Shared domain helpers belong here when both core and UI need them.
- If a UI feature needs to reconstruct runtime state, sync template-backed
  workspace metadata, or format panel/workspace domain labels, prefer adding a
  core API instead of rebuilding that logic in `horizon-ui`.

- `remote_browser_credential` owns provider credentials for remote browser
  sessions: the value-free store trait and sink, the session-only in-memory
  store, the `keyring-core` adapter (`keyring_store`), the fake store seam, and
  the resolver that turns bindings into one origin-bound authorization header.
  It never serializes values, never touches the process environment, and holds
  no transport, allocation or UI code. `workbench` runs OS-store operations on
  a worker thread with a presence cache the UI reads, so a locked store or an
  unlock prompt never blocks the render loop.
- `browser/remote_session.rs` turns a configured remote target into the
  driver's `RemoteSessionRequest`: endpoint and limits from the provider,
  `alwaysMatch` capabilities from the target (device fields placed by the
  provider's adapter kind), and the authorization header resolved from the
  credential stores at create time. Nothing here allocates or renders; the
  panel keeps the request for Retry and never consults a store again.

### `horizon-browser-cli`

- Owns deterministic browser plans, bounded execution control, durable job
  lifecycle, explicit resume policy, and user-facing reports.
- `job/agent.rs` selects and invokes the optional local prompt-job adapter
  (Grok headless or Codex `exec`); MCP actions stay on the existing contract.
- `run_state.rs` coordinates lifecycle metadata and exclusive resume leases.
  Large verified step results live in immutable files managed by
  `run_state/checkpoint_artifacts.rs`; `state.json` retains only compact result
  references so intent updates never rewrite prior payloads.
- `standalone/lease.rs` records keep-alive host identity, process-start identity,
  and creation order so resume can reconnect to the newest live browser or fail
  closed after a crash or PID reuse.
- `variables.rs` validates bounded plan literals; `project.rs` writes optional
  JSON/CSV projections of prior structured results.
- Browser performance acceptance for #324 is the combined G1 WebSocket fixture
  plus G1b five-minute E24 observation, recorded in
  [`docs/architecture/browser-performance-acceptance.md`](browser-performance-acceptance.md).
- Browser packaging measurements, public-API/semver expectations, and
  publish-flag/package-content checks live in
  [`docs/architecture/browser-packaging.md`](browser-packaging.md) and
  `scripts/check-browser-packaging.sh`. No crate is published from that check.

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
  - `browser_close_requests`: the audited close queue, kept pending until the
    panel's teardown signal settles and the remote release is established
  - `browser_remote_create`: planning for a create that names a remote target:
    provider, capabilities and credentials resolved before any panel exists,
    typed refusals that carry no value, and the per-provider session limit
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

- `app::settings::remote_browsers` renders provider readiness and credential
  entry for remote browser targets. It reads the editing config and the
  app-held `CredentialWorkbench`; typed values go to the workbench only, never
  to the YAML buffer, and its input buffers are scrubbed when the settings
  editor closes.

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
