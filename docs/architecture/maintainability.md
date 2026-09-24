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

### `horizon-device`

- Independently packageable library; default builds need no Horizon or async
  runtime. `model` owns target/endpoint, geometry and bounded action types;
  the private `x11` adapter owns Linux capture/input implementation details.
- The optional `cli` binary owns target-file locking, CLI dispatch and stdio MCP.
  Device input and screenshots share one contract across both transports.
- Application/display lifecycle, live viewers, and remote management remain
  caller responsibilities. Future device adapters must advertise explicit
  capabilities; no viewer renderer or remote manager belongs in the library.

### Native Device panels

- `horizon-core::device` owns the validated VNC target and panel state;
  `panel::spawn::device` creates a panel without a PTY. Creator-supplied identity
  is normalized in core and persisted with panel state; VNC observations stay
  connection-local. Existing command metadata persists the target, and an
  optional SSH tunnel host is persisted through the panel's `ssh_connection`
  like an SSH panel, in which case the target is the endpoint as seen from that
  host. Restored panels require manual reconnect.
- Device panel requests are claimed on the UI thread by `app/device_request_pump.rs` when a request file appears. Wayland does not deliver `RedrawRequested` while a frame callback is outstanding and the compositor is not presenting the surface, and it reports neither visibility nor minimization, so the queue cannot wait for `App::ui`.
- `horizon-ui::device_widget` owns presentation and, only while a person has Interact on, event capture in `capture.rs` and the pointer and keyboard mapping in `input.rs`; agents never get that path. `details` renders
  labelled connection facts, while core selects and bounds the displayed name. `frame` validates
  and composites decoded rectangles; `session` owns a cancellable socket/decoder
  worker and a single latest-frame slot. `session/tunnel.rs` owns one `ssh -W`
  process per tunnelled connection, piped straight into the decoder and killed
  with it, and keeps ssh's last diagnostic lines for the failure message. The completed UI pass reconciles root
  and detached viewer visibility; hidden workers pause frame requests and resume
  with a full refresh. Desktop resizing also requests a full refresh. Panel,
  workspace and session cleanup drops the worker. Viewer input never reaches the target.
- The native decoder is a narrowly patched Git dependency pinned by full commit
  in `Cargo.toml`. Its fork retains licenses, provenance and qualification limits.
  It is excluded from the publishable `horizon-device` control package. The
  standalone crate's screenshot/action contract remains independent of viewing.
- Isolated Linux fixtures live in `scripts/device-smoke`; interactive tests use a native VNC Device panel and recording scoped to the isolated desktop.
  They are development prerequisites; no recorder or remote manager is added
  to the product.

### `horizon-wayland`

- Owns the Wayland protocol bridges winit 0.30 does not provide, currently
  trackpad pinch through `zwp_pointer_gestures_v1`. It adopts winit's own
  `wl_display` through libwayland's foreign-display entry point, because
  Wayland only delivers gestures over a client's own surfaces; a second
  connection would see none.
- This crate holds the only `unsafe` Wayland FFI. `PinchBridge::start` is an
  `unsafe fn` whose contract is that the adopted display outlives the bridge;
  its single caller, `horizon-ui::native_app::pinch`, discharges it by owning
  the bridge for the life of the event loop alongside an `OwnedDisplayHandle`
  that drops after it. The crate and that call site carry
  `#![deny(unsafe_code)]` with one scoped `#[allow]` and a `// SAFETY:`
  rationale each; the exception must not widen. The three crates permitted
  to use scoped unsafe code are `horizon-cursor`, `horizon-wayland`, and
  `horizon-ui`; every other crate keeps `forbid`.
- `wayland-client` is requested with its `system` feature explicitly, since
  only the libwayland backend can adopt a foreign display. The bridge is
  Linux-only and returns `None` off Wayland, so X11 sessions keep the XInput
  bridge and macOS keeps winit's native pinch.

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

- `file_chooser` owns manual upload requests and answers shared with the host.
  The CDP and BiDi session leaf modules bind each request to its original input
  and retire stale requests. `horizon-core::browser::file_chooser` performs bounded
  directory reads off the render thread; `horizon-ui::browser_widget::file_chooser`
  presents the host dialog. Public panel metadata reports support and pending
  selection without exposing selected paths.
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
  `board/arrangement/reordering.rs`; the panel resize collision cascade, which
  pushes sibling panels and whole cloud frames, lives in
  `board/arrangement/panel_collisions.rs`.
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
  `terminal/` leaf modules. `terminal/logical_line.rs` assembles the text under
  a click from soft-wrapped rows and from URL rows a program hard-wrapped; its
  row-shape heuristics are tested under `terminal/logical_line/tests/`.
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
- `agent_work/` keeps restart-work evidence separate from conversation binding.
  `command.rs` verifies external executable ownership with a bounded shell probe; functions and aliases keep their ordinary launch.
  `ledger.rs` correlates lifecycle events by prompt; `transcript.rs` reads bounded
  provider tails without retaining their content; `policy.rs` makes conservative
  restart decisions from explicit evidence. `store.rs` owns bounded, private,
  atomic metadata writes and one-shot handoff claims. The UI's internal
  `--agent-work-hook` command records events before initializing plugins or the
  GUI; the dedicated opt-in plugin is leased with the owning host. These
  primitives do not launch turns. `lifecycle.rs` attaches opted-in launch
  identities and prepares/seals handoffs; `repository.rs` fingerprints repository
  contents. Terminal shutdown workers own cancellation and PTY exit, keeping
  filesystem work outside the UI shutdown path.
- `local_store.rs` centralizes agent-store environment paths and read-only
  SQLite opening so discovery, validation, and usage reporting agree.
- Shared domain helpers belong here when both core and UI need them.
- If a UI feature needs to reconstruct runtime state, sync template-backed
  workspace metadata, or format panel/workspace domain labels, prefer adding a
  core API instead of rebuilding that logic in `horizon-ui`.

- `remote_browser_credential` owns provider credentials for remote browser
  sessions: the value-free store trait and sink, the session-only in-memory
  store, the launch-time environment snapshot (`environment`), the `keyring-core`
  adapter (`keyring_store`), the fake store seam,
  and the resolver that turns bindings into one origin-bound authorization
  header. It never serializes values, never writes a secret back into the
  process environment, and holds no transport, allocation or UI code. `workbench`
  runs OS-store operations on a worker thread with a presence cache the UI
  reads, so a locked store or an unlock prompt never blocks the render loop.
- `browser/remote_session.rs` turns a configured remote target into the
  driver's `RemoteSessionRequest`: endpoint and limits from the provider,
  `alwaysMatch` capabilities from the target (device fields placed by the
  provider's adapter kind), and the authorization header resolved from the
  credential stores at create time. Nothing here allocates or renders; the
  panel keeps the request for Retry and never consults a store again.

- `browser/remote_identity.rs` caches friendly remote-session labels and full
  hover details from provider evidence, keeping requested settings separate.
  Remote lifecycle transitions clear the evidence; the UI only sizes and
  truncates the cached label.

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
- `native_app/pinch.rs` bridges Linux trackpad pinch into the existing zoom
  path: window-scoped XInput 2.4 pinch and focus events on X11, and the
  `horizon-wayland` bridge on Wayland. Older X11 servers and compositors
  without pointer gestures retain keyboard/scroll zoom.
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
  - `browser_cleanup`: closes ended browser panels on the host polling cadence,
    after pending creates report their failure; uses the core board's ended-session
    query and existing close path so remote holds and teardown remain tracked
  - `browser_remote_create`: planning for a create that names a remote target:
    provider, capabilities and credentials resolved before any panel exists,
    typed refusals that carry no value, and the per-provider session limit
  - `canvas`: canvas rendering and HUD
  - `canvas_scroll`: viewport-local scroll gesture ownership and canvas event consumption
  - `canvas_drag`: viewport-local ownership for primary drags starting on empty canvas
  - `lifecycle`: frame orchestration and repaint pacing, with application-exit
    ownership and persistence sequencing in `lifecycle/shutdown.rs`
  - `panel_chrome`: panel titlebar chrome, badges, and rename UI
  - `panels`: panel-area orchestration and body rendering, with gesture and
    context-menu handling and outcome application in `panels/interaction.rs`
  - `remote_hosts_overlay`: overlay state/input shell with query/filter,
    layout, row/header paint helpers and the SSH/VNC mode plus destination
    workspace controls split into `remote_hosts_overlay/`, plus the per-host
    context menu; the overlay only reports an `Open`, `SetDefaultWorkspace`
    or `SaveShortcut` action
  - `remote_hosts`: overlay lifecycle and catalog refresh, with workspace
    resolution, VNC port resolution (`RemoteLaunch`) and panel creation in
    `remote_hosts/launch.rs`, config-backed
    preferences (the default workspace) in `remote_hosts/preferences.rs`, and
    host shortcuts saved as presets in `remote_hosts/shortcuts.rs`
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


### Shared Chromium lifecycle

`horizon-browser::session::SharedBrowserSession` provides an opt-in process and
profile group. Each driver owns its own page target and connection. Reservations
cover queued startup, and page lifecycle controls cannot terminate a live sibling.
Final release retires the process before profile cleanup; failed startup retains
the exact child control, and failed target close remains pending. Existing
`start_session` callers retain exclusive sessions.

### Shared Firefox lifecycle

`session::SharedSessionGroup` selects a Chromium or Firefox profile group.
`webdriver/shared.rs` owns Firefox context reservations, page cleanup and exact
process retirement. Each page filters BiDi events to its context tree. Classic
commands select their context and execute under one shared lock and one deadline;
session-global routes are rejected. Failed page cleanup retains its context and
registration IDs for bounded retries while preserving active siblings.

An ambiguous Firefox context-creation reply retains ownership until exact-process
reap and blocks further creation in that generation. Runtime capture registrations
join the same retry ledger as startup registrations.

### Browser profile membership

`horizon-core::browser::shared_session` owns local profile-group membership,
shared-page launch options, and duplicate eligibility. Runtime snapshots persist
the group ID independently of panel identity. Session copies rekey groups while
preserving shared membership; cleanup retains the registered group until exact
process retirement and final profile deletion.

Once a profile group has been shared, its backend remains fixed for that group's
lifetime, including after a sibling panel closes. This prevents switching away
while a closed sibling still owns the profile during asynchronous teardown.
Standalone profiles retain ordinary backend switching.

### Duplicate requests

The control manifest stores duplicate requests in a nested queue that older hosts
cannot misinterpret as independent creation. `app::browser_duplicate` revalidates
the actor, workspace, source ownership, readiness and handoff state before invoking
the core operation and exposing the result.

The panel titlebar menu and public `browser_duplicate` tool expose shared-page
creation for ready local Chromium and Firefox panels. UI duplication reveals the
new panel; tool requests pass through host authorization. The public tool contract
and smoke gate include duplication without exposing browser transport endpoints.

### Remote allocation recovery

Remote allocation recovery is split between the engine's
`webdriver/remote/recovery` (private identity and authorization) and its `probe`
module (bounded exact-session WebDriver and provider reporting evidence), core's
`browser/remote_recovery` (scope and exact lease ownership), the browser control
`manifest/recovery` queue, and the UI's `app/browser_recovery` dispatch and
`settings/remote_recovery` presentation. Network work does not run on the UI thread.

Remote Android Chrome single taps use `webdriver/session/remote_click` for
bounded geometry settling and visual-viewport conversion, plus native touch
input. Its JavaScript preflight performs geometry checks and scrolling only;
Safari, desktop and multi-click paths retain their existing native dispatch.
The gate requires negotiated `platformName=Android` and `browserName=chrome`;
other Chromium-branded names retain their existing dispatch until their native
coordinate semantics are validated. The geometry fixtures run in the browser-engine CI tier.

The preflight requires visual-viewport evidence. It retains native hit-testing
for transparent interactive controls and the first client-rectangle convention
of Element Click; searching later fragments is a separate behavior change.

Device view limits and viewport layout live in `crates/horizon-core/src/device/view.rs`;
`crates/horizon-ui/src/device_widget/controls.rs` collects session-local presentation
settings while its worker applies the bounded image layout.

Native Device panel lifecycle uses `manifest/device.rs` for bounded private host
requests, `horizon-browser-mcp/src/controller/device.rs` for public MCP dispatch,
and `app/device_requests.rs` for live board scope and ownership checks. Requests
are host-bound; inspection is workspace-scoped and mutations require ownership.
`device_widget` reports connection and actual clipped image presentation
separately. Native input remains in the standalone device crate.
`app/device_presentation.rs` captures the renderer's root/detached visibility
context; `device_widget/host.rs` owns transient view and Reveal observations.
Their optional shared manifest data keeps render-time and later navigation
geometry distinct without changing view, focus, ownership or transport.
`app/device_reveal_wait.rs` holds a Reveal result until a frame completed after
the reveal draws the viewer, or a bounded wait returns the blocking observation.

### Shared remote-provider capacity

`browser/remote_usage` owns the provider-independent usage snapshot, refresh
state, credential snapshot and provider API adapters. It performs no session
admission or mutation. `settings/remote_usage` only renders snapshots and collects
refresh actions. Adding a provider API requires an adapter and normalization,
without changing the shared display. `RemoteProviderProfile::local_session_limit`
expresses whether an adapter uses a local grid limit or provider-managed capacity.
Allocation ownership and cleanup remain in the existing recovery modules.

The `manifest/provider_usage` queue carries only safe provider summaries between
MCP and its owning host. `controller/provider_usage` is the MCP transport boundary;
`app/browser_provider_usage` performs host authorization and dispatch into the
shared core model. The CLI plan runner invokes the same public tool. Provider API
and credential logic must not be copied into either transport or UI rendering.

## Cloud workspaces

`horizon-cloud::companions` owns passive repository declarations and pure,
scope-checked target selection. It has no provider, filesystem, SSH or UI side
effects. Callers supply the owning host's trusted inventory; the serialized
selection itself is not authentication. Runtime grants and readiness belong in
the host coordinator, not the portable selection contract.

`horizon-cloud` owns portable repository configuration, typed worker identities,
RunPod REST lifecycle and the durable allocation-state protocol. Credentials are
caller supplied. `runpod::volumes` owns CPU workspace-volume placement, allocation
fencing, attachment verification and deletion; `runpod::stock` answers per-size CPU
stock for placement. `runpod::replacement` switches a verified, running worker to a
new image digest through the pod update and observes which image of the pair the
provider reports; it keeps no journal, so callers record intent first.
`runpod::billing` reads one worker's validated billing buckets. The crate must
not depend on core/UI, terminal, browser, device, Git, settings storage or a provider CLI.
`startup::StartupMetadata` is bounded opaque, non-secret creation data saved in
`WorkerSpec`. The RunPod request passes it through one environment value, and the
shared worker-identity check requires its exact echo before adoption or lifecycle
operations. The provider crate does not interpret application bootstrap or
membership policy. Existing runtime callers leave this optional field absent.

`horizon-core::cloud_runtime` coordinates local image preparation, committed source
transfer, durable deployment/session references and existing OpenSSH transport.
Its `image`, `repository`, `state`, `lifecycle` and `ssh` modules keep those duties
separate. `image::agents` resolves the newest npm release of every agent CLI for
image builds, using only bounded, cancellable registry reads. `allocation`
re-exports the typed allocation/project/controller identities and machine-local
placement bindings from `horizon-cloud-protocol`. That small
contract crate can be used by the host and worker without importing core/UI; it
contains no runtime or ownership authority. Its `signed` module authenticates bounded
management envelopes against a previously pinned controller key, binding project
membership, operation, action, revision and exact payload bytes. It does not load
signing credentials or replace worker membership checks. Host registration and
worker dispatch are separate consumers and are not connected yet.
`cloud_runtime::owner` supplies an opt-in owning-host journal API: native machine
identity, a cloud-specific OS-store registration, a signing key and a canonical
lock outside transferable journal state, pinned by native file identity and a nonce.
Lock roots/files are created with private modes and must retain effective-user
ownership and no group/other access on acquisition and every ownership check.
Artifact reads, publication and directory synchronization use a retained directory
handle and reject replacement of its registered path. Root creation retains a
private staging-directory handle through exclusive publication, refusing an
existing or competing destination. Its journal
generation/hash is anchored in the registration; candidate, pending, published and committed boundaries keep
crash recovery exact and copied or rolled-back journals fenced. Every native write
rechecks ownership and the exact preceding registration; deleted or changed native
state cannot be recreated from cached credentials. Pending and final commits also
validate the journal artifacts they anchor before changing native state. This API is not
called by runtime entry points yet. Backends are implemented for Linux Secret Service
and macOS Keychain; other hosts remain blocked by the existing Unix directory
durability requirement. Synthetic tests never access the user's credential store.
On Linux, `scripts/cloud-smoke/controller-keyring.sh` qualifies registration and
signing against a disposable Secret Service backend across crash/restart, using a
private D-Bus session, private data directories and the exact foreground daemon PID.
The macOS backend remains unqualified: native create/save/reopen/sign and
locked/unavailable-Keychain checks on an isolated target must pass before macOS
runtime activation or a support claim. Synthetic macOS filesystem tests do not
qualify native Keychain behavior. Secret serialization writes directly into a
fixed zeroizing buffer, including partial output on an encoding failure.
The host `allocation::legacy` module converts the complete v1 deployment payload into a validated
allocation/project pair and reconstructs the old runtime view without dropping
cleanup fences. The module performs no I/O and grants no provider or membership
authority. `state::migration` provides opt-in local publication with an old-reader
barrier and preserved storage/trust companions; `state::transaction` commits and
recovers the paired projections. Migration and updates hold a parent registry lock
before allocation and project locks, with exclusive mutable handle access. These
local APIs perform no provider I/O and are not called by runtime entry points yet;
credential binding, runtime activation and sharing remain integration work. `deployment::storage` persists a separate volume journal under the same
per-cloud lock; explicit cleanup and local removal account for both resources.
`worker_contract` shares capability transport and contract validation
between local image checks and SSH readiness, including legacy full-image support.
`cost` estimates a worker's current run from the provider's effective hourly rate
and latest start time without I/O, so any surface can reuse it; the UI only formats
it and schedules the refresh. `cost::total` combines billing buckets with that run
so the latest, possibly partial, bucket is never counted twice, and labels a worker
billed before the one-year read window with that window instead of its lifetime.
`billing` owns the per-cloud background refresh: settings, credential and provider
reads run on a short-lived thread every few minutes, bounded by the provider timeout
and cancelled when the cloud is unbound or its runtime is dropped; the UI schedules
a frame for the next refresh even while the cloud is idle.
`repository::launch` discovers the selected checkout, parses its default profile
and resolves the committed revision; the UI launch coordinator captures workspace
identity and performs preparation while the user enters a title. Credential
preflight runs off-thread and checks only profile-enabled agents.
`registry` owns repository-scoped machine bindings, credential separation, private
Docker authentication, issuer scope checks and pre-allocation image validation.
Its `draft` shares setup with the UI/CLI; `store` persists generation/account fences;
`management` shares validation, status, reconciliation and revocation with the
standalone registry MCP adapter. `horizon-cloud::runpod::registry` owns only provider
binding transitions and never decides credential policy. Publishing credentials
stay in local temporary Docker configs; only verified pull material is transferred.
Disconnecting presentation never terminates compute or remote processes.
Cloud grouping and immutable membership live in `cloud_panel`, sharing workspace
layout calculations. Corner resizing of a cloud frame lives in `cloud_panel/resize.rs`.
UI modules render controls, consume progress and attach the
ordinary panel types; worker/provider operations run outside the render thread.

`horizon-cloud-worker` hosts the existing browser runtime and public MCP queues
inside one container. The device CLI owns serialized native input and attribution.
SSH carries presentation; agent tools and tmux remain on the worker. Image build
scripts and runtime contract examples live in `examples/cloud-worker`.

The default `cloud-workspaces` feature enables operational RunPod clouds.
`cloud-panel-mock` additionally enables labelled design fixtures; simulated
providers never authorize allocation. Original fixture details remain in
[the prototype guide](../prototypes/cloud-panels.md).

Cloud remote-browser deployment uses repository-scoped machine-local grants in
`cloud_runtime::browser_auth`; portable profiles contain target names and selected
worker-local ports only. `horizon-browser::remote_config` and `provider_usage`
share provider adaptation and capacity policy across desktop and worker hosts.
The worker retains remote allocation recovery and teardown ownership; disconnecting
its presentation client never releases a hosted device or ends the private tunnel.
