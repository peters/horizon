# Maintainability Guardrails

This document describes the project boundaries that keep Horizon from drifting
back into large multi-purpose modules.

## Module Boundaries

### Remote worker image

- `repository_overlay::bundle::store` keeps anonymous publication as its default;
  the explicit named layout owns permanent digest slots in a focused Linux leaf.
  Neither layout discovers data, transfers it, schedules capture or certifies
  provider durability. See [named publication](named-bundle-publication.md).
- One-shot checkpoint generations keep explicit request/receipt types in
  `repository_overlay::checkpoint`, bounded composition in its `linux` leaf and
  retained-attempt accounting/publication in `storage`; the worker CLI only
  decodes and reports. Existing byte-capture enrollment remains unchanged; see
  [checkpoint generations](checkpoint-generations.md) for coverage and proof limits.
- Explicit worker byte capture keeps request validation and existing core
  capture/bundle reuse in `horizon-repository::capture`; `byte-capture.py` owns
  the detached bounded-attempt loop and `byte_capture_store.py` owns private
  records. No provider lifecycle or automatic enrollment is implied. See
  [worker byte capture](worker-byte-capture.md) for the non-atomic protection
  boundary and retained-volume attestation.
- `repository_overlay::capture::revision` adds explicit complete selected layers
  at an observed work-branch commit, reusing pinned readers and bundle limits.
  Enrollment version 2 opts in; the fixed-base delta API and version 1 remain
  unchanged. This leaf does not export Git history or advance a checkpoint.
- Prepared task admission keeps immutable intake/setup selection and read-only
  checkout inspection in `repository_overlay::intake::setup`. The fixed
  `setup-binding` and `setup-checkout` helpers never materialize or start tasks.
  The panel helper reuses its single claim/nonce path, binds prepared markers to
  that selection, and observes matching retained tasks without checkout reads.
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
  actions, semantic results, bounded network records, video-capture options,
  and redacted audit entries.
- Depends only on serialization support. It must not acquire process, socket,
  async-runtime, image-decoder, filesystem-coordination, MCP, `horizon-core`,
  or UI dependencies.
- Contains no host policy. Browser launch, persistence, authentication,
  retention, steering ownership, backend input serialization, and presentation
  remain in their owning crates.

### `horizon-browser`

- Owns browser processes, CDP/WebDriver/BiDi transports, frame delivery,
  bounded WebM page-pixel recording (`video/`), and deterministic shutdown.
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
  CPU profile configuration reuses the Azure adapter's pure placement validation;
  serialization and exact lookup never acquire credentials or construct a client.
  A saved profile is not UI/dispatcher support or authorization to allocate a worker.
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
  Its `runpod` leaf accepts an explicit supplied profile/key and selects trust from
  positive first-pin intent or a complete retained pin. Selection and non-creating
  recovery bind the same owned snapshot; an unmarked interrupted start is refused,
  never inferred as first-bootstrap authority or automatically cleaned up.
  Its `configured` leaf validates a home/owner/profile-bound new-workspace preview,
  consumes exact image/storage consent and saves one immutable identity before
  task-free allocation. Errors retain recovery coordinates; manual checks distinguish
  dormant and interrupted records and recover only the same allocation snapshot.
  Its `configured/azure` leaf binds the complete approved CPU profile before key
  preparation, reuses shared setup/fence logic and refuses profile drift before
  non-creating recovery. Credential construction is lazy; preview has no I/O.
  This leaf does not prepare Git, deliver repository credentials, start tasks or
  attest caller-supplied image/storage trust. UI confirmation remains a separate layer.
  The overview's `setup` UI leaf binds transient Local Docker/RunPod/Azure CPU requests to
  the actual home/session/config, caches exact confirmation values and consumes
  consent once. Manual noncreating Check retains original attempt coordinates;
  settled history never blocks inventory, repository Prepare or saved Shell Start.
  Azure disclosure and consent use the complete frozen profile; its optional
  billing-currency ceiling is independent of RunPod's US-cents input. No profile
  editor, credential lookup or default provider is introduced by the form.
- `remote_environment_observation.rs` produces overview-safe, point-in-time
  provider observations with exact snapshot checks but no private-key access or
  saved-state writes. It shares allocation observation validation with recovery;
  neither absence nor observed readiness grants management or attachment authority.
  Its `configured` adapter resolves an explicit named local profile and rejects
  stale overview summaries before passing the owned snapshot to the observation gate.
- `remote_worker_inspection.rs` shares the current owned-worker fence between
  workspace and genuine saved-panel queries without granting execution authority.
  `remote_repository_pack.rs` observes an explicitly nominated existing pack via
  the fixed pinned worker command; its protocol leaf binds bounded paths and exact
  identity to the saved repository base. It neither uploads nor grants source,
  publication, synchronization or checkout-readiness authority.
- `remote_worker_storage.rs` reuses the owned-worker fence for read-only fixed-root
  qualification over pinned SSH, including zero-panel workspaces. Its protocol
  leaf requires complete request delivery and exact bounded status/exit pairs;
  valid negative observations are distinct from transport or protocol errors.
  It reuses the worker's storage status type without initialization, provider
  selection, persistence or task authority.
- `remote_github_credential.rs` owns explicit first-PAT delivery admission after
  non-creating provider inspection and retained host-pin checks. Its transport
  leaf reuses the fixed SSH command and bounded stdin-only exchange, accepting
  only exact installer success replies. Post-send drift is unknown, never a
  reason for automatic retry, credential replacement or worker/task mutation.
  The borrowed secret is redacted and is not persisted or discovered implicitly.
- `remote_git_setup.rs` admits explicit ordinary Git submission and read-only
  receipt inspection on an existing owned, pinned worker. The saved repository,
  exact runtime and explicitly matching saved branch bind its fixed stdin frame;
  a separate source/work-branch model and PR base selection remain future work.
  Its protocol leaf distinguishes detached handoff, observed original preparation
  and unknown outcomes, not current checkout cleanliness or task readiness.
  Its transport leaf caps each write by the remaining lease and retains valid
  negative replies without granting replay. No allocation, PAT installation,
  task start, storage writes or implicit recovery occur. Linux clients first.
- `remote_git_setup/configured` separately binds a local confirmation to the full
  saved allocation, configuration, selected volume and credential disclosure mode.
  Its explicit optional first-PAT install precedes detached Git submission only
  after successful acknowledgement and renewed admission; mutation uncertainty
  never grants retry or implies credential rollback. Preview retains no PAT.
  Its Azure leaf admits the immutable CPU profile binding and retained resource
  before lazy client construction; it reuses the common Git/PAT transport.
  Existing ARM host-key attestation must still match the saved SSH pin. Missing
  provenance or private identity is never repaired, and no compute lifecycle API
  is called by repository preparation or its manual receipt check.
- `remote_worker_status.rs` gates non-creating panel inspection on exact owned
  recovery and current lifetime. Its `protocol.rs` leaf owns bounded status-only
  wire types; `ssh.rs` owns the query's private host-pin lifetime and retains the
  panel deadline, response ceiling and public error projection. It neither grants
  attachment/task startup nor changes the general user-configured SSH API or remote
  execution lifetime.
  Explicit saved shell/command verification reuses those gates and compares
  literal argv plus the effective directory with the worker's retained intent;
  unresolved agent launch/handoff/resume semantics stay fail-closed.
  Its `configured` leaf admits one actual owner's saved panel and exact local
  profile, reuses non-writing recovery and returns timestamped status only.
  Missing worker pins are not repaired, and observations never persist recovery.
- `remote_worker_ssh.rs` centralizes crate-private pinned client options and SSH
  path quoting without granting attachment authority. Typed query/attach modes
  share isolation options; each owns private trust material. Status queries retain
  bounded I/O, while interactive trust follows both terminal event proxies through
  detached Drop and asynchronous join. General user-configured SSH is unchanged.
  Its Linux `trust.rs` leaf uses strict anonymous private inodes and an owner-PID
  descriptor path for both modes, with no named-file fallback. Holding the exact
  descriptor preserves each pin through local teardown; process exit cannot leave
  a named trust file, even when normal shutdown bypasses Rust destructors.
  Its shared `query.rs` leaf owns bounded nonblocking local-child I/O, explicit
  caller response ceilings and typed transport failures without owning protocol
  parsing, admission, remote tasks or provider lifetime. Query tests are colocated.
  The existing success-only query adapter now uses the same streaming engine:
  known-input queries check actual written bytes, while streaming exchanges retain
  early/nonzero replies and distinguish source EOF. One bounded read/write per
  iteration preserves cancellation/deadline checks; only no-progress passes sleep.
- `remote_panel_attachment.rs` consumes explicit exact-allocation admission, fresh
  read-only identity/worker inspection and saved-intent verification before a pinned
  local PTY. Only explicit recovery commits observations; attachment preserves saved phases.
  Its target-bound attempt rechecks snapshots before input-capable handoff; it is
  not authenticated attachment, saved Ready state or an atomic Stop/attach fence.
  Board/UI admission and inert restore remain separate from this Linux-only API.
- `board/remote.rs` prepares a protected, short-lived local handoff after the final
  off-thread store fence, then consumes it into one disconnected same-owner view
  without I/O. The actual client session and current view identity must match;
  queued admission expires without affecting remote task lifetime. Visual rehoming
  preserves execution identity. It does not persist transport arguments, promote
  readiness or enable implicit restore. UI request/config/session invalidation and
  global cross-session Open remain separate; admission is not continuous revocation.
  Handoff synchronizes terminal grid and PTY geometry to the current view, not the
  earlier asynchronous request. Its age bound conservatively includes store latency.
- `remote_panel_attachment/configured.rs` admits only the actual owner session,
  exact saved selection and explicitly named local provider profile before calling
  non-creating attachment. It does not infer authority from inventory visibility or
  copied client references, and has no ambient provider or profile fallback.
- `board/remote_views/` separates metadata-only reopen target/persistence checks
  from off-thread owned-record lookup and inert snapshot preparation. Consuming
  adoption uses the existing insertion/layout path without starting a process or
  connecting. Reopening never copies executable intent or grants remote authority;
  current-client/request invalidation and later fresh reconnect remain mandatory.
- UI `remote_environments/reconnect/` caches view labels on explicit interaction.
  Its single-flight worker prepares connections off-thread; lifecycle and modal
  actions invalidate before queued handoff adoption. Pending discarded receivers
  retain the slot until completion. Same-owner views do not implement global Open.
- UI `remote_environments/reopen/` explicitly loads saved panel identities and
  prepares inert missing views off-thread. It shares lifecycle invalidation with
  reconnect; competing actions discard queued results before either handoff drains.
  Adoption marks reference-only runtime state dirty without starting remote tasks.
  Its `inspection` leaf caches explicit retained-task observations in the same
  bounded worker slot, including with no local views. Labels are point-in-time;
  session, selection, config and Stop invalidation also discard late task results.
  Its `start` leaf separately previews and confirms saved Shell execution in that
  same single-flight slot. It never attaches a view or provisions a checkout.
  Late dispatched starts retain an unattributed unknown-outcome warning rather
  than reporting stale success or scheduling a retry.
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
  `namespace/` composes exact-base Git leaves with both verified overlay layers into
  ordered immutable namespaces. Whole-layer topology, effective link resolution
  and expanded path/logical-byte budgets are checked before returning file references.
  It does not read regular base payloads, write a checkout or authorize export;
  the later confined writer must revalidate referenced base bytes independently.
  `namespace/source/` resolves the same immutable result through an explicit raw-object
  inspector: bounded payload reads, supported SHA-1 commit/tree records and streamed
  traversal stay separate. Regular base blobs are inspected without payload reads;
  aggregate metadata work counts repeated expansion. The existing trusted-repository
  resolver remains available and shares the whole-layer composition path. Header-only
  inspection is additive to `seed/` object streaming, without abandoning lazy payloads;
  the packed adapter shares one strict failure/poison path for both operations.
  `recovery/` compares two borrowed exact-source/base snapshots without I/O or a
  merged result. Index and unstaged changes retain their separate baselines; differing
  indexes and cross-side leaf/directory conflicts remain explicit. Cached Git blob
  identities normalize verified overlay bytes without reading base payloads. Both
  inputs survive errors/cancellation; comparison grants no recovery or durability.
  `seed/` streams verified exact-base objects into an internally fresh private Git
  repository and constructs its detached HEAD, shallow boundary and independent
  staged index. Source adapters own their I/O behavior; private Git pathname writes
  require stable ancestry and exclusive same-user ownership. Failed residues remain
  explicit. Working files and publication are separate. Linux `seed/packed/` owns
  explicit stable-object-store validation, isolated bare metadata, resource-limited
  native Git acquisition and strict lazy batch framing. It needs trusted system Git
  and prlimit executables; native delta allocations remain subject to those ceilings.
  No source config, recursive alternates, hooks, filters or network transports are
  inherited. Cancellation/failure or an unfinished stream terminates only its owned
  child; private metadata remains explicit. Stable source/ancestry and exclusive
  scratch ownership are prerequisites, not confinement against concurrent mutation.
  Linux `seed/export/` prepares one bounded standard non-thin base pack from an
  existing verified seed. It shares the isolated metadata view and owned native
  process with packed acquisition, but selects an exact synthesized shallow
  boundary and closes request input before draining/validating producer completion.
  Framing, chunked file writes and digesting live in its output leaf. Private
  success/partial artifacts remain explicit; no network, setup start, immutable
  publication or durability is inferred. Input/output overlap checks cover the
  entire seed, not only its object store. See [pack preparation](../remote-repository-pack.md).
  `seed/receive/` verifies expected encoded pack identity before isolated strict
  indexing and bounded exact-base closure enumeration. Its native leaf handles
  bounded hash-line responses, private index validation and no-replace relocation;
  command isolation and framed chunk-copy/digest work remain shared with existing
  packed acquisition/export helpers. A received object directory still requires
  namespace/seed/setup policy checks; no publication or task admission is inferred.
  Its `observe/` subtree reopens only the fixed generated layout without mutation:
  a layout leaf pins/checks known nodes and pack identities; orchestration streams
  encoded verification before shared read-only native index/closure checks. The
  reader's existing metadata fingerprint is reused for held/named comparisons.
  See [private pack receipt](../remote-repository-pack-receipt.md).
  The opt-in `seed/receive/named` leaf owns caller-named exclusive attempts and
  held-private-directory ordinary renames without changing the default receiver.
  It reuses stream/native/observer validation and retains uncertain attempt names;
  see [named private receipt](named-git-pack-receive.md) for ownership and proof limits.
  Its peer `publication/` subtree owns explicit qualified no-replace publication
  and distinct pre/post/uncertain rename receipts. It reuses the read-only fixed
  layout's bound handles and fingerprints for synchronization instead of adding
  another repository walker. Storage qualification and sibling-name policy remain
  shared; non-root fingerprints survive relocation unchanged. See
  [pack publication](../remote-repository-pack-publication.md).
  Its explicit `named` leaf instead uses permanent digest-directory claims and
  ordinary rename only for the fresh claim owner. Independent named outcomes
  preserve uncertain operations and unused inputs; fixed-layout verification and
  synchronization remain shared. See [named packs](named-git-pack-publication.md).
  `intake/` combines fixed retained-root initialization with pack/bundle receipt and
  noncreating observation. Its pure request/response shell keeps strict identities;
  the Linux leaf owns the create-new claim, held directory bindings and component
  orchestration. Existing claims never consume payload or grant another receiver.
  Its Linux `controller` owns explicit approval and retained-owner/pin validation;
  `controller/input` owns held local pack verification. The repository binary's
  `intake` leaf handles bounded command framing, not export approval or task setup.
  See [combined intake](../remote-repository-intake.md); setup/task authority remains
  separate from both worker receipt and controller handoff.
  `checkout/` prepares the seed and raw working namespace together, using exclusive
  descriptor-relative writes, incremental loose decoding and pinned-file verification.
  Literal links are created last. This private logical checkout retains failures;
  full synchronization, no-replace publication and task authorization remain separate.
  Its Linux compression dependency avoids whole compressed-object memory mappings.
  `checkout/publication/` consumes a held private checkout identity for explicit
  no-replace sibling publication on verified journaled ext4 storage. Its bounded
  no-follow walk synchronizes files and then directories bottom-up; rename transfers
  the receipt to the destination before final root/parent synchronization. Post-rename
  failures retain an explicitly published, unsynchronized receipt; uncertain rename
  errors retain both candidate names for inspection. No rollback,
  deletion or task launch occurs; healthy storage and unchanged, exclusively held
  private ancestry, trusted kernel metadata and stable mounts remain required.
  This is not cloud recovery or power-loss proof.
  Linux `storage.rs` owns the shared, read-only journaled-filesystem qualification
  and bounded kernel-option validation. Publication maps its errors to the existing
  receipt contract; qualification alone does not synchronize, write or grant execution.
  `retained_setup/` owns one immutable setup claim per existing private retained
  workspace root. Constructor-checked intent and bounded canonical records are
  separate from Linux confined storage. Only fresh no-replace publication followed
  by file/directory synchronization creates a non-cloneable grant. Observation,
  identical retries and grant Drop never execute, repair, expire or remove a claim.
  A claim means unknown outcome, not running/completed; trusted stable private
  ancestry, healthy qualified storage and separately verified remote ownership
  remain prerequisites. Admission alone never creates scratch or runs setup.
  `retained_setup/execution.rs` consumes the in-process grant and its immutable
  intent for one materialization. Linux `scratch.rs` creates one fixed private
  child without replacement and synchronizes it and its retained parent before
  handing the held subtree to the existing materializer. Existing nodes are never
  adopted or cleaned; cancellation, failed synchronization and component failures
  retain every named residue. Execution errors distinguish the consumed boundary
  from original admission and preserve typed materializer receipts. Observation
  stays unknown; the original synchronous method does not store its result.
  `retained_setup/outcome/` owns the separate recorded-execution API, redacted
  historical snapshot and bounded canonical intent-bound result schema. Its Linux
  storage leaf privately reads or publishes one fixed no-replace result slot with
  file/parent synchronization and read-back identity verification. Preflight errors
  never execute; later recording errors retain the complete actual execution result.
  Read-only completion never adopts the referenced paths, acknowledges its own
  synchronization or establishes liveness. Missing results remain unknown; nothing
  permits replay, cleanup or construction of another grant. Private wire types keep
  deserialization separate from public validated snapshots. This is not independent
  supervision, transport, cloud recovery or proof of power-loss durability.
  `materialize/` composes existing private bundle retrieval, isolated raw-object
  resolution, checkout preparation and explicit publication into one worker-callable
  operation. Its typed result preserves source metadata and every known checkout or
  uncertain destination; no retry or cleanup is implied. `horizon-repository` owns
  only the bounded versioned command boundaries and truthful response/exit status.
  Its `setup/` tree separates immutable input validation, one-shot core API
  orchestration and response projection. Core `admit_materialization` performs
  bounded read-only input-tree identity separation before fresh admission; existing
  claims skip that preflight so missing inputs cannot prevent observation. The
  original claim-only admission API is unchanged. Public core projection accepts only typed
  execution results, not unchecked deserialized snapshots. Setup/status distinguish
  fresh recording, historical observation, unknown claims and retained execution
  when recording fails. Status never creates or replays; the original materialize
  protocol stays compatible. These Rust commands remain synchronous.
  Its separate `receive/` tree validates a bounded framed header and complete
  canonical overlay bundle before any store access. `receive-overlay` reuses the
  immutable bundle store; `overlay-status` reads only and never acknowledges new
  synchronization. Lost output/write acknowledgement retains possible publication
  without overwrite, cleanup, setup/task start or inferred export permission. Input
  storage synchronization is not the retained-setup qualifier or cloud durability.
  Its separate `pack/` tree owns bounded framed request validation, typed response
  projection and Linux routing to existing pack receipt/publication/observation.
  Shared core path/name validators expose lexical checks only, not storage authority.
  Pack payloads stream to core; uncertain receipt, pre/post-rename and unknown-rename
  failures retain distinct candidates. Status never writes or fabricates missing,
  readiness or synchronization acknowledgement. See [pack commands](../remote-pack-transfer.md).
- `containers/remote-worker/setup-launch.py` owns the separate worker-only bounded
  handoff: it delegates strict input/root observation to `setup-status`, returns
  existing observations without launch, and passes only an absent intent through a
  private pipe to the fixed `setup` child in an independent process session. The
  child alone admits/consumes the core grant. Submission and handoff uncertainty
  never prove current liveness, create replay authority or authorize a kill. The
  launcher writes no request/log files; retained Rust completion remains the
  recovery surface, not transient child output. Missing recording stays unknown.
  It does not own task supervision, transport, provider lifetime or client wiring.
  Its explicit `--git` mode reuses that detached handoff for ordinary Git:
  `git-status` owns strict request/root admission and `git-prepare` owns the
  exclusive retained claim. Git responses have their own bounded validator;
  overlay grants and storage qualification are not involved. No implicit retry
  or clone cancellation follows from client-channel loss.
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
  Persistent post-create verification failures retain the resource and consumed
  grant for explicit non-creating recovery or management. An in-flight creation
  response cannot undo another client's retained Stop through failure cleanup.
- `cloud_run/interactive_worker_delete.rs` is an opt-in deletion-scope observer,
  separate from issuing deletion or recording durable completion. The Azure
  implementation reads only the exact saved resource group with owned identity
  tags; a surviving or deleting group remains present even when its VM is absent.
  It never uses guest commands, host-key lookup, lifecycle mutation or polling.
  The RunPod `runpod/interactive/deletion.rs` leaf observes only the exact Pod,
  retaining ownership and bound-attachment checks without host-key or volume
  lookup. Even a terminated Pod is present until its exact GET reports absence;
  this does not establish the state of independent storage or grant its deletion.
  Its deletion-only wrapper binds the saved persistent request and network
  selection without an SSH pin. It refuses provisioning, recovery and generic
  inspection before I/O, exposes no Start/Stop capability and cannot install a
  host-key source. Explicit Delete delegates the unchanged volume/Pod admission;
  only the separate observation path is Pod-GET-only.
- `remote_environment_delete.rs` coordinates explicit Delete, separately confirmed
  Retry and read-only confirmation over an exact owned allocation. Its dedicated store transition
  retains worker identity and creation fences as a tombstone; generic writes
  cannot introduce, resolve or erase the new destructive intent. Provider
  acceptance is not absence, and independent network storage is not implicitly
  deleted. Legacy cleanup phases remain separate from this explicit operation.
- `cloud_run/interactive_worker_stop.rs` is an opt-in Stop contract, separate from
  deletion and client lifetime. The local adapter's `local_docker/stop.rs` verifies
  exact ownership and disabled automatic removal before a bounded stop, then
  verifies the same resource is retained and inactive. Lost responses need that
  independent proof; absence is not retained storage. Durable management intent,
  checkpointing and user-facing actions remain separate caller responsibilities.
- `remote_workspace/stop.rs` coordinates an explicit exact-worker Stop around
  durable `Stopping`/`Stopped` phases. The allocation store's `stop.rs` commits
  each phase against both owned snapshots without holding a transaction across
  provider I/O. Generic writes cannot manufacture completion or erase/rewind its request,
  and setup/recovery cannot consume it as normal client activity. Legacy cleanup
  reasons remain distinct; saved completion is not current provider status.
  This durable entrypoint admits only persistent execution: timed creation/expiry
  cleanup needs separate coordination before it can promise retained Stop intent.
- `remote_workspace/stop/confirmation.rs` completes only existing Stop intent,
  using the separate provider-read-only observer and exact allocation/selection fences.
  RunPod reuses retained-state checks without Stop, SSH or host-key lookup. Only
  verified retention permits local completion; pending/absence preserve saved state.
  The saved public pin is shape-checked, not live-attested.
- `remote_workspace/stop/configured_confirmation.rs` admits manual Linux RunPod
  checks through the exact saved request, pin, named profile and HPS selection before
  lazy credential lookup. It calls the same confirmation coordinator, not Stop or
  setup recovery. Only verified completion changes local state; absence is not retention.
- `remote_workspace/stop/configured_runpod.rs` shares retained request/profile/public-pin
  admission with confirmation and admits first Stop only with a saved HPS selection.
  Existing Stop intent refuses mutation before credential lookup; uncertainty hands off
  to Check, never another Stop. The internal coordinator consumes the exact admitted
  allocation, so workflow drift cannot be adopted between validation and the intent CAS.
  No private identity, host-key lookup, storage discovery or creation is authorized.
- `remote_workspace/stop/configured.rs` admits one explicitly confirmed saved
  selection through its exact named local provider profile. It reloads the owned
  record and compares the full summary/revision before durable Stop coordination,
  returning only overview-safe metadata. It preserves persistent-only admission;
  unsupported profiles and stale selections
  gain no fallback authority; failures may require refreshing retained Stop intent.
- The overview's `remote_environments/stop.rs` owns only confirmation, single-flight
  background execution and cached outcome presentation; its `stop/paint.rs` collects
  explicit actions for retained persistent local and Linux RunPod workers; timed and
  other cloud Stop stays disabled. RunPod confirmation discloses process-memory loss,
  metadata-only retention proof and possible continuing storage cost. Its first-Stop
  callback uses existing-only, non-migrating storage. A separate Check saved Stop action observes existing RunPod intent in
  the same single-flight slot, without replay or private SSH identity. Closing
  invalidates presentation, not an admitted operation. Its completion writer uses
  existing-only, non-migrating store admission; legacy or corrupt storage is refused
  before configured-provider admission.
  Completion invalidates provider observations and refreshes saved inventory while
  keeping its target-bound result readable. All mutation remains in core coordination.
- `cloud_run/runpod.rs` coordinates provider operations and exact ownership
  reconciliation. Its `models.rs` leaf owns profiles, persisted worker identity,
  lifecycle results and typed errors; `create_request.rs` owns serialized
  creation-request construction. Public type re-exports remain stable. HTTP
  transport and common interactive-worker adaptation stay in their existing
  `http.rs` and `interactive.rs` leaves.
- `cloud_run/runpod/network_attachment.rs` binds one complete interactive request
  and caller-selected HPS volume to the client and host trust. Fresh volume
  metadata precedes the creation claim; exact v2 Pod attachment checks precede
  readiness and both first-pin observations. Ordinary clients reject network
  adoption. This leaf grants no storage ownership, exclusivity, contents trust,
  volume mutation or implicit Stop authority. Setup persists the selection before
  identity preparation and reloads it on retry/recovery.
  Its colocated tests retain the ordinary lifecycle regression boundary.
- `cloud_run/runpod/stop.rs` adds only explicit, exact-Pod Stop for persistent
  workers with a verified ordinary or request-bound HPS `/workspace` volume.
  Selected-volume Stop checks fresh volume metadata and exact Pod attachment
  before and after one action, including a lost response, without deletion,
  restart, SSH, or UI admission. Provider observations do not prove a backup,
  filesystem durability or process survival; the existing durable Stop coordinator
  owns saved intent. Colocated network attachment Stop tests cover identity drift
  and uncertain results independently of the ordinary retention regression suite.
- `cloud_run/runpod/host_key.rs` separates explicit task-free first-pin bootstrap
  from saved-full-pin reconnect. Its `sample.rs` leaf validates bounded authenticated
  log samples without claiming current-boot freshness or exhaustive history. The
  worker emits a versioned public binding only after retained identity preparation;
  the caller must persist the owned pin before admitting setup or tasks.
- `cloud_run/store.rs` owns workflow snapshots and creation-claim transactions.
  Its `cloud_run/store/database.rs` leaf owns private-path preparation, connection policy,
  schema initialization, and compatibility checks. Keep database opening separate
  from domain-specific record operations.
  Schema seven reserves immutable provider-binding metadata without backfilling
  legacy allocations; valid schema-four/five/six inventory stays read-only.
  See [retained provider selection](remote-provider-bindings.md) for the staged API boundary.
  Existing-store observers use clone-preserved read-only handles, without private-path
  creation or migration. Single workspace/allocation getters always use read-only
  connections; live WAL updates remain visible without granting schema repair.
  Explicit existing-only writers refuse creation, migration, permission repair and
  journal-mode changes; every writable connection requires the current schema.
  Unix device/inode checks reject detected replacement under a stable trusted directory;
  they do not protect against concurrent same-user path swaps. This new mode refuses
  non-Unix platforms; existing constructors and domain-specific CAS admission are unchanged.
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
- `job/agent.rs` selects and invokes the optional local prompt-job adapter
  (Grok headless or Codex `exec`); MCP actions stay on the existing contract.
- `run_state.rs` coordinates lifecycle metadata and exclusive resume leases.
  Large verified step results live in immutable files managed by
  `run_state/checkpoint_artifacts.rs`; `state.json` retains only compact result
  references so intent updates never rewrite prior payloads.
- `standalone/lease.rs` records keep-alive host identity, process-start identity,
  and creation order so resume can reconnect to the newest live browser or fail
  closed after a crash or PID reuse.

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
    Shell inventory/modal tests live in the colocated `remote_environments/tests`.
    `remote_environments/observation` owns single-flight manual provider checks,
    event-based invalidation and cached point-in-time labels, never provider I/O
    on the render thread or passive repaint polling.
    `remote_environments/repository` owns explicit repository confirmation,
    transient masked PAT input and manual receipt checks. Its paint leaf clears
    text-edit undo history; stale mutation results retain an unattributed warning.
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

`repository_git` owns the worker-only ordinary Git preparation identity and
observation API. Its `linux` leaf confines the separate one-shot claim and fixed
task-accessible checkout, while `git` owns sanitized bounded Git subprocesses.
The `lfs` leaf admits standard bounded pointers, hydrates them through the packaged
client with per-child file limits, and verifies the fresh checkout/index before completion.
The `submodules` leaf admits committed metadata and GitHub-only URLs, then hydrates
exact gitlink commits in confined, initially empty directories with detached child HEADs.
It does not delegate recursion, update strategies or configuration to `.gitmodules`.
Root and children share one 300-second Git command deadline and cumulative LFS budget
(1,024 paths, 64 MiB per object, 512 MiB payload); recursion is capped at depth four
and 32 children with 1 MiB aggregate planning metadata. These are not an aggregate
quota on Git packfiles or filesystem usage. Custom LFS configuration/filters and
unsupported submodule metadata fail without a completion receipt. The CLI only
frames existing requests/responses; this does not use the retained overlay qualifier.
Observation never mutates user commits or worktree changes, and old failed claims
are never upgraded or replayed by newly supported preparation behavior.

Ordinary Git task admission uses `repository_git` canonical binding and read-only
checkout identity projections, framed in the existing CLI Git leaf. The worker's
`start-git` shares prepared-task marker and held-directory checks without invoking
overlay setup or Git. `remote_worker_status::git_start` owns the separate explicit
saved-Shell mutation, provider/pin admission and lease-bounded transport; existing
inspection functions never call it. A matching retained marker is observed before
current checkout inspection, preserving completed/dirty tasks without replay.
Its `configured` child prepares an opaque saved-command confirmation without
credentials or I/O outside the local store. Explicit execution consumes that
snapshot, rechecks the complete allocation/configuration/storage selection and
uses only the existing pinned start path. Post-dispatch drift is an unknown
outcome, not authority to retry. Inert view catalogs never carry executable intent.

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
