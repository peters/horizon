# Maintainability Guardrails

This document describes the project boundaries that keep Horizon from drifting
back into large multi-purpose modules.

## Module Boundaries

### Agent skills

`horizon-ui::plugin_install::mcp_skills` owns the additional MCP skill assets,
complete-directory ownership checks, and installation targets. `owned_skills`
publishes complete new trees and records exact installed bytes before an update.
The record permits recovery of interrupted file replacement. Host leases remain
in `user_skills`. Skill references ship with their entry points. The repository
check compares shipped MCP tools and qualified operation lists with the linked guidance.
Private integration caches use exact file synchronization and remove retired
references. They accept prior unrecorded bundles and refuse symlinks before writes.


Remote-development provisioning, workers, managed SSH views, repository transfer,
and the Remote Environments modal were removed in #693. Ordinary SSH terminals,
Remote Hosts, Sessions, and remote browser settings remain. Runtime v3 references
are retained only as inert compatibility data with command-replay guards until
versioned migration; no remote-development controller consumes them. Config v11
omits obsolete top-level provider profiles while preserving `browser.remote`. Config v12 drops
the presets of the removed Usage and Git Changes panels, and a saved board keeps its other panels
when it names one of those kinds.

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
- `app/undrawn.rs` is the pass of a window that draws no frame. eframe calls `App::logic` in place of a frame, and the pass does there the work that needs no drawing: the events of the terminals, the start of a panel that a request opened, the window part of the shared terminals and the states of the agent panels. [The UI thread guide](ui-thread.md#the-pass-of-a-window-that-draws-no-frame) gives its rules and the list of the work that still waits for a frame.
- `horizon-ui::device_widget` owns presentation and, only while a person has Interact on, event capture in `capture.rs` and the egui key, button and position mapping in `keys.rs`; agents never get that path. The creator of a viewer sets `DevicePanelState::input`. RFB cannot report a server that ignores input, so a `ViewOnly` viewer disables Interact and shows the reason. `details` renders
  labelled connection facts, while core selects and bounds the displayed name.
  `device_widget/session.rs` connects the viewer to a `horizon-vnc` session: it
  sends the session wakes to the egui viewport of the viewer, and it keeps only
  the clipboard texts that are native session metadata. `picture.rs` gives the
  frames to egui without a copy.
- The `horizon-terminal` crate owns the click targets and the search of a
  terminal grid, with no PTY and no UI. It must not depend on egui, eframe,
  wgpu, winit or other Horizon crates, because other applications use it as a
  Git dependency. `logical_line.rs` assembles the text under a grid point from
  soft-wrapped rows and from URL rows a program hard-wrapped; its row-shape
  heuristics are tested under `logical_line/tests/`. `links.rs` finds the OSC 8
  hyperlink, URL or path at a column, and `search.rs` matches a literal or regex
  query in lines of text for `search_board` of horizon-core.
- The `horizon-vnc` crate owns the live session without a UI. It must not
  depend on egui, eframe, wgpu, winit or other Horizon crates, because other
  applications use it as a Git dependency. `framebuffer` validates and
  composites decoded rectangles and records the changed region; `input` turns
  pointer and key state into VNC input events; `session` owns a cancellable
  socket/decoder worker and a single latest-frame slot. `session/tunnel.rs` owns one `ssh -W`
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
  rationale each; the exception must not widen. The crates permitted
  to use scoped unsafe code are `horizon-wayland` and its single call site
  in `horizon-ui`; every other crate keeps `forbid`, except the one function
  that `horizon-pty-tap` describes below.
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

- `semantic.rs` owns document references and their invalidation.
  `session/semantic/frames.rs` and `webdriver/session/semantic/frames.rs` own
  child-frame scans and native input. Frame routes stay private to the engine.
  These modules share the node limit across documents and bound each scan.
- `file_chooser` owns manual upload requests and answers shared with the host.
  The CDP and BiDi session leaf modules bind each request to its original input
  and retire stale requests. `horizon-core::browser::file_chooser` performs bounded
  directory reads off the render thread. Its `picker` leaf owns file filtering,
  selection, and validation. `horizon-ui::browser_widget::file_chooser` uses the
  shared `dir_picker::modal` for the host dialog. Public panel metadata reports support and pending
  selection without exposing selected paths.
- `webauthn` owns the passkey bridge: `origin` (origin and `rp_id` rules,
  `clientDataJSON`), `request` (the command contract), `command` (the
  supervised process) and `page.js`. The CDP and BiDi leaf modules
  `session::webauthn` and `webdriver::session::webauthn` resolve the caller's
  origin from the browser and deliver replies. The host UI reads only
  `FrameSlot::passkey_pending`.
- `session::file_drop` sends native Chromium file drops. The WebDriver
  `session::file_drop` leaf delivers local Firefox file drops. The UI
  `app::file_drop::browser` leaf routes desktop drops to page coordinates.
  CLI and MCP attachment actions share authorization and private file staging
  in `horizon-browser-control`.
- Owns browser processes, CDP/WebDriver/BiDi transports, frame delivery,
  optional WebM page-pixel recording (`video/`, enabled by `video-capture`), and
  deterministic shutdown. `process/watch.rs` owns the watch that ends one browser
  when the Horizon that started it is gone: one child of Horizon holds the read end
  of a pipe whose write end belongs to Horizon, so a `SIGTERM`, a `SIGINT`, a crash
  or a `SIGKILL` of Horizon closes that end and the watch signals the process group
  of the browser. A browser of the Chromium engine has one, and so has the driver
  of the Firefox and Safari engines, because that driver starts their browser. The
  browser that Horizon ends itself stops its watch first, and a browser that
  Horizon did not start has no watch. Default builds omit the AV1 encoder; Horizon and its
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
  credential stores, Teach, and host panel state stay outside this crate.
- `remote_authorization` forms the provider authorization header from bound values
  that a host's own stores supply, and `remote_slots` owns the cross-instance
  provider slot leases and their quota key. Every host uses these two modules, so
  a host without core forms the same header and counts the same slots.
- `ssh_forward.rs` owns the one loopback forward to the `DevTools` port of
  another machine, and `ssh_forward/` holds its leaves: `transport.rs` is the
  bounded `ssh` call and the quoting rule, `ssh.rs` carries the program and the
  bounds of a connect, and `remote_browser.rs` holds the two `sh` snippets that
  start a Chromium and read its endpoint. The crate holds no Horizon program on
  the far side and no record file.

### `horizon-pty-tap`

- Owns the reader of a panel's pseudo terminal that reports the program status
  protocol (OSC 7501) of the program in it. The VT parser of this workspace drops
  a private OSC number, so the crate wraps the PTY of a panel, or the socket of its
  holder, in `PanelPty`: every call of the terminal event loop reaches the source
  that the wrapper owns, and its reader hands every byte on unchanged while a
  bounded scanner looks at it.
- The interface it implements, `alacritty_terminal::tty::EventedReadWrite`, cannot
  be implemented without one `unsafe fn`, because the trait declares `register` as
  an unsafe function and the wrapper must forward it. That forward is the only
  `unsafe` in the crate: the wrapper registers exactly the sources of the variant it
  owns and adds no source of its own. It is why this crate carries
  `#![deny(unsafe_code)]` with one scoped `#[allow(unsafe_code)]` and a
  `// SAFETY:` rationale, and why every other crate keeps `forbid`.
- The scanner keeps at most 256 bytes of one payload, drops a longer report whole,
  allocates nothing and cannot block or panic on any byte of a stream. A complete
  payload reaches a frame through a queue of eight reports that drops its oldest
  entry, so the event loop thread never waits for the UI thread, and a report
  that a newer one replaced before a frame ran is not replayed.
- Almost every byte of a panel's output is text with no sequence in it, so the
  bytes between two sequences are skipped by one `memchr` search of the next
  escape byte and the state machine looks at nothing there. `memchr` is a direct
  dependency for that search; the version is the one of the tree that `regex`
  already asks for, so the workspace gains no package.
- The crate depends on `alacritty_terminal` and on `polling` for the types that
  the forwarded method names. It holds no state of the product: the panel parses
  the payload and owns the claim, so this crate has no Horizon dependency.
- `share.rs` owns the outlet of a shared terminal. The reader copies each read
  into one bounded buffer while a computer follows a screen of the terminal. A
  read costs one atomic load while no computer follows. The crate holds no
  frame and no link of a shared terminal. The design page is
  [`shared-terminals.md`](shared-terminals.md).

### `horizon-core`

- Owns board state, workspace metadata, panel lifecycle, persistence
  projections, and shared layout math.
- `config.rs` owns configuration loading, validation and aggregate settings.
  Preset models, panel-option conversion and existing default/migration helpers
  live in `config/presets.rs`, with stable public re-exports from the parent.
- `board.rs` should stay orchestration-focused, with board-local submodules for
  agent working-status detection, workspace and panel
  membership changes, arrangement/collision logic, geometry queries, and
  shutdown state. Preset slot collision and swapping lives in
  `board/arrangement/reordering.rs`; the panel resize collision cascade, which
  pushes sibling panels and colliding workspace frames, lives in
  `board/arrangement/panel_collisions.rs`.
- Large board test surfaces should live in `board/tests/` topic files so
  `board.rs` can stay focused on production orchestration.
- `panel.rs` owns panel models and content access; explicit restart logic lives
  in `panel/lifecycle.rs`, and the sizes and scrollback limits of a new panel
  live in `panel/metrics.rs`. `panel/spawn.rs` keeps the selection of the content
  of a new panel, its saved session binding and its seed of the common fields,
  while `panel/spawn/terminal.rs` owns terminal construction, transcript
  restoration and disconnected/failure snapshots. The spawn answers one job per
  module beside it: `panel/spawn/launch_arguments.rs` builds the program and the
  arguments of a launch, `panel/spawn/environment.rs` builds the environment of
  the process and the one line of a launch whose named secrets were late,
  `panel/spawn/editor.rs` opens a file panel, and `panel/spawn/browser.rs` owns
  the browser panels beside the rule that chooses their machine. Remote client views
  retain an execution reference independently of visual workspace membership;
  creation restores an inert transcript and restart refuses local task fallback.
- `terminal.rs` should keep the terminal types and shared imports; lifecycle,
  event handling, resize policy, selection logic, and content helpers belong in
  `terminal/` leaf modules. `terminal/content.rs` maps a viewport cell to a grid
  point and asks `horizon-terminal` what the cell opens.
  Terminal destruction sends shutdown, then a helper thread joins and destroys
  the PTY. A finished thread handle can still own a PTY that waits for its child.
  The bounded regression for this case lives in `terminal/lifecycle/tests.rs`.
- `terminal/share.rs` writes the screen of a terminal for a follower. Its
  `ShareSource` gives a thread that is not the UI thread the outlet, the screen
  and the way to the program of one terminal.
  `terminal/follow.rs` owns the terminal of a follow panel and its feed.
  `panel/follow.rs` and `board/follow.rs` open the follow panel. A follow panel
  has no program of this computer. The places that act on the program of a
  panel ask `Panel::is_follower` and do nothing for a follow panel. The design
  page is [`shared-terminals.md`](shared-terminals.md).
- `config/server.rs` owns the three settings of the shared terminals of a
  computer, beside the entry that names the server. It also holds the sentence
  that the window shows for them (`SHARED_TERMINALS_NOTICE`).
- `browser/mod.rs` maps engine sessions/events into Horizon panel state and
  retry/teardown behavior. `browser/manifest.rs` reexports the shared
  `horizon-browser-control::manifest` implementation so existing host callers
  retain identical types, locks, and process identity.
- `browser/attached_host.rs` owns a browser panel whose Chromium runs on another
  machine: the connect order (read the endpoint, forward it, start the browser
  and retry once), the typed failures in `attached_host/failure.rs`, and the
  worker thread of the connect. Its tests are in `attached_host/tests.rs`.
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
  Session activity date formatting lives in `runtime_state/session_display.rs`;
  the UI's `app/panels/session_rebind.rs` renders the recovery menu (with focused rendering and input tests in `session_rebind/tests/`) and collects
  resume/copy actions without changing provider bindings.
- `agent_links.rs` answers, from the board alone, the pairs of an agent panel
  and the panels that it controls: the owner of a card, the live claim of a
  browser page, and the actor of a panel that an agent opened for the person. It
  stores nothing and keeps no state for a pair. The kind name of that opener is
  only the text of the chip of the titlebar and names no pair; a Device viewer is
  left out, because its owner is a field of the UI caches and not of the board.
- `agent_work/` keeps restart-work evidence separate from conversation binding.
  `command.rs` verifies external executable ownership with a bounded shell probe; functions and aliases keep their ordinary launch.
  `ledger.rs` correlates lifecycle events by prompt; `transcript.rs` reads bounded
  provider tails without retaining their content; `policy.rs` makes conservative
  restart decisions from explicit evidence. `store.rs` owns bounded, private,
  atomic metadata writes and one-shot handoff claims. The UI's internal
  `--agent-work-hook` command records events before initializing plugins or the
  GUI; the dedicated opt-in plugin is leased with the owning host, and the state
  report that a pi launch loads with `-e` is written beside it by
  `plugin_install::work_extension` from the module under `assets/plugins/pi`. A
  launch names that file only while it is a file. These primitives do not launch
  turns. `lifecycle.rs` attaches opted-in launch
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
  store, the launch-time environment snapshot (`environment`), the resolved-value
  store that a caller offers back, the shared handle of one store,
  and the resolver that turns bindings into one origin-bound authorization
  header through `horizon-browser-control::remote_authorization`. It never serializes values, never writes a secret back into the
  process environment, and holds no transport, allocation or UI code. `workbench`
  runs file operations on a worker thread with a presence cache the UI
  reads, so a disk write never blocks the render loop. The file itself is
  `secret_store`, which owns the schema, the modes and the rows of every value.
- `browser/remote_session.rs` turns a configured remote target into the
  driver's `RemoteSessionRequest`: endpoint and limits from the provider,
  `alwaysMatch` capabilities from the target (device fields placed by the
  provider's adapter kind), and the authorization header resolved from the
  secrets file of this computer at create time. Nothing here allocates or renders; the
  panel keeps the request for Retry and never consults a store again.

- `browser/remote_identity.rs` caches friendly remote-session labels and full
  hover details from provider evidence, keeping requested settings separate.
  Remote lifecycle transitions clear the evidence; the UI only sizes and
  truncates the cached label.

### `horizon-sync`

- Owns the server store and the link that a computer uses to reach it: the log,
  the objects, the identifiers of decided intents, the frames, the local queue of
  a computer and the socket that a later slice reaches over `ssh`. The design page
  is [`horizon-server.md`](horizon-server.md) and the boundary is
  [`server-link.md`](../review/trust-boundaries/server-link.md).
- `store/` is the only code that opens the server file. `store/schema.rs` holds
  the pragmas, the tables and the schema version; `store/apply.rs` decides one
  intent; `store/writer.rs` is the one writer task; `store/rows.rs` shapes what a
  pull returns. No other module of the crate reads or writes the file.
- `protocol.rs` and `frame.rs` carry the frames over any pair of byte streams;
  `server.rs` answers one link, `client.rs` is the computer side with its queue,
  its mark and its mirror trait, and `link.rs` binds a socket and relays one
  child. The crate holds no UI and no `ssh` call: a later slice adds both.
- The crate may depend on `horizon-core` for a machine record and its validators.
  It must not depend on `horizon-ui`, egui, eframe, wgpu or winit, so a server can
  run without a desktop.
- `live/` owns the shared terminals of a server. `live/protocol.rs` holds the
  frames and their bounds, and `live/hub.rs` is the relay. `live/outbox.rs`
  holds the frames that wait for one link, and `live/relay.rs` serves one live
  link. `supervisor/live.rs` is the live link of a computer. No module of `live/`
  opens the store or a file. The design page is
  [`shared-terminals.md`](shared-terminals.md) and the boundary is
  [`shared-terminals.md`](../review/trust-boundaries/shared-terminals.md).

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
  - `browser_requests`: transient host polling and the host state of a
    requested browser panel, and the actor panel lookup that routes a request
    from a live agent panel
  - `browser_create_dispatch`: the host side of a claimed create request, which
    creates the panel, audits its dispatch and publishes its result
  - `browser_create_lifecycle`: the pending creates that the host starts,
    judges for readiness and retires when the board goes away; a starting
    create is judged on the manifest snapshot the coordination worker last
    read at a bounded interval, and a remote create waits there for its
    credential values and its provider slot
  - `browser_create_failure`: the terminal failures that a create reports
  - `browser_manifest_stamp`: the host stamp of the browser manifests, and the
    workspace stamp that keeps MCP authorization current
  - `browser_host_io`: the one coordination worker that runs the claims,
    audits, manifest stamps and results of the browser request pumps in the
    order they were asked for; a later frame applies each outcome in the same
    order; an outcome asked for on a board that a session switch or a prototype
    restore replaced refuses its request and changes no panel of the
    board that stands now
  - `browser_request_claims`: the claims of create, visibility and close
    requests, which run on the coordination worker
  - `browser_visibility_requests`: visibility changes for agent requests; the
    board decides whether a request may apply, the worker audits and publishes
    it, and a later frame shows or hides the panel
  - `browser_close_requests`: the audited close queue, kept pending until the
    panel's teardown signal settles and the remote release is established
  - `browser_cleanup`: closes ended browser panels on the host polling cadence,
    after pending creates report their failure; uses the core board's ended-session
    query and existing close path so remote holds and teardown remain tracked
  - `browser_remote_create`: planning for a create that names a remote target
    from configuration and from the values the workbench read, so no frame
    takes a credential-store lock: provider, capabilities and credentials
    resolved before any panel exists, typed refusals that carry no value, the
    per-provider session limit, and the bookkeeping of holds and leases
  - `browser_remote_credentials`: the OS-store values a remote create needs,
    read one at a time on the workbench thread and polled without waiting, so
    a locked store never holds a frame
  - `browser_remote_slots`: the provider slot of a remote create, leased on
    the coordination worker, and the allocation each applied lease is recorded
    in until its release is established
  - `canvas`: canvas rendering and HUD
  - `agent_lines`: the lines between an agent panel and the panels that it
    controls. It holds the geometry of the curves, the strength and the colour
    of a line, the agent in focus, and the cache of the shapes, keyed by the
    pairs, the places of the panels, the view, the focus and the theme. The
    pairs and the canvas setting are read elsewhere: `horizon_core::AgentLink`
    answers the pairs and `CanvasConfig::agent_lines` turns the drawing off.
  - `canvas_scroll`: viewport-local scroll gesture ownership and canvas event consumption
  - `canvas_drag`: viewport-local ownership for primary drags starting on empty canvas
  - `lifecycle`: frame orchestration and repaint pacing, with application-exit
    ownership and persistence sequencing in `lifecycle/shutdown.rs`
  - `panel_chrome`: panel titlebar chrome, badges, and rename UI
  - `panels`: panel-area orchestration and body rendering, with gesture and
    context-menu handling and outcome application in `panels/interaction.rs`
    and the screen-sized resize grip (six corner dots on a 32-point hit target), its input sublayer and focused tests in
    `panels/resize.rs` and `panels/resize/tests.rs`
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
  - `sidebar`: sidebar rendering and deferred sidebar actions, with the root
    toolbar in `sidebar/toolbar.rs` (the Menu in `sidebar/toolbar/buttons.rs`),
    the choice of where a new workspace starts in `sidebar/new_workspace.rs` and
    the described Menu rows in
    `sidebar/rich_menu.rs`
  - `settings`: settings editor state and save/apply flows
  - `shared_terminals`: the join of the shared terminals. `shared_terminals.rs`
    starts and stops the live link and its pump, and does in each frame the
    work that needs the board. `shared_terminals/list.rs` builds the list of
    this computer. `shared_terminals/pump.rs` is the share pump: the one thread
    that moves the bytes, so a window that gets no frame shares and follows.
    `shared_terminals/owner.rs` is its owner side, with the watched panels and
    the check of the typed bytes, and `shared_terminals/follower.rs` is its
    follower side, with the other computers and the follow panels. A lock
    between the window and the pump is held for one copy only, and the pump
    takes it with `try_lock`. `shared_terminals/link.rs` is the
    trait of the calls of the link, so a test joins without a server. The
    section of the sidebar is `sidebar/computers.rs`, and the fields of the
    Settings window are `settings/server/sharing.rs`. The design page is
    [`shared-terminals.md`](shared-terminals.md).
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
  to the editing settings, and its input buffers are scrubbed when the settings
  editor closes.

- `horizon_core::speech` is the thin layer of speech input: `command` splits one
  command line of the settings, `endpoint` checks the endpoint of a service,
  `service` builds one multipart request, `text` cleans one answer, `clock` holds
  the bounds that a test replaces, and `take` runs one take on a worker thread.
  `app::speech` holds the hotkey, the icon, the insert and the pill of a
  sentence, `app::panels::mic` draws the icon of the title bar, and
  `app::settings::speech` is the **Speech** tab.

## File Size Policy

The automated line-limit and `too_many_lines` suppression checks cover
`horizon-device`, `horizon-browser-protocol`, `horizon-browser`, `horizon-core`,
`horizon-sync`, `horizon-terminal`, `horizon-ui` and `horizon-vnc`; extracting a
new crate is not an escape hatch for oversized modules.

- Start splitting a Rust source file before it reaches roughly 600 lines.
- CI fails non-test Rust source files above 1000 lines in:
  - `crates/horizon-device/src`
  - `crates/horizon-browser-protocol/src`
  - `crates/horizon-browser/src`
  - `crates/horizon-core/src`
  - `crates/horizon-sync/src`
  - `crates/horizon-terminal/src`
  - `crates/horizon-ui/src`
  - `crates/horizon-vnc/src`
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
- Does code that runs during a frame wait for file I/O, a lock, a process or
  the network? Move it to a worker thread. The
  [UI thread guide](ui-thread.md) gives the patterns and a review checklist.

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

Device view limits and viewport layout are in `crates/horizon-vnc/src/view.rs`;
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

## Panel screenshots

Panel screenshot encoding, bounded private export retention and clipboard dispatch
live in `horizon-ui::screenshot`. Browser and Device widgets expose their retained
source pixels; `app::device_requests` validates workspace, host and ownership before
using the same capture path for MCP requests. Copy buttons acquire pixels only on
click. Browser rendering resets preserve exports, while panel close and explicit
host exit remove them.
