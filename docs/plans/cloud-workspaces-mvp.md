# Cloud Workspaces MVP

## Delivery and current state

The user accepted the native Linux candidate on 20 September 2026 and authorized
one implementation PR for this cross-subsystem MVP. Merge and release require
separate authorization. Development and validation use an isolated worktree;
the shared checkout and original prototype evidence remain untouched.

The accepted frozen native executable has SHA-256
`c6eca1627d5f6481e64c711f362ae07da353354bbc5d7f6570bcbd6f258ee91e`.
The complete development history, source manifest, logs and original recordings
are retained privately. The public commit preserves the accepted runtime code
and excludes private qualification and account-operation records.

| Milestone | Implemented | Locally verified | Cloud verified | User accepted |
|---|---|---|---|---|
| Cloud grouping, immutable membership, layouts, fullscreen | Yes | Native Linux | Yes | Yes |
| Portable configuration and provider lifecycle | Yes | Yes | CPU and GPU | Yes |
| Local build, push, digest and worker contract | Yes | Yes | Private image pulls | Yes |
| Committed source, worktrees and persistent sessions | Yes | Yes | Multiple agents and restart | Yes |
| Worker browser/device services and ownership | Yes | Yes | Real tools and ownership | Yes |
| Optional repository Git credentials | Yes | Yes | Remote publication smoke | Yes |
| Per-step timing and measured transfer progress | Yes | Tests and native upload | Private push | Yes |
| Capability-selected clean images | Yes | Minimal/full/selected profiles | Yes | Yes |
| Public PR, current-head CI/review, final smoke and cleanup | In progress | Pending final publication | Pending final cleanup | Separate gates |

## Future issue body: scope and behavior

A cloud represents one remote development container and provides defaults to its
ordinary child panels. A workspace can contain multiple named clouds; cloud and
workspace membership is permanent. Multiple instances of supported agents retain
normal interactive terminals. Browser and native VNC Device panels reuse existing
implementations, with labels distinguishing the current browser controller,
read-only desktop viewing and the last agent that supplied desktop input.

Each cloud has independent Default, Rows, Cols and Grid controls. Cloud fullscreen,
nested panel fullscreen and Escape preserve the previous overview. RunPod is the
only operational compute provider. Daytona and Fly.io remain labelled design
fixtures, using real local panel types without claiming cloud provisioning.

Deployment is ordered:

1. Validate configuration, credentials and selected committed source.
2. Build locally when configured, respecting `.dockerignore` and BuildKit layers.
3. Push and resolve an immutable registry digest; validate the worker contract.
4. Provision/reconcile one CPU or GPU worker and check readiness.
5. Transfer the committed revision, selected LFS objects and pinned submodules.
6. Prepare separate branches/worktrees and stable tmux sessions for agents.

Each deployment stage shows elapsed time. Local image transfers show measured
bytes, rate and a remaining-time estimate when Docker exposes a known total.
Cached layers do not inflate transfer speed. Build steps and provider readiness
show current activity without inventing a percentage or completion time. Verbose
output remains expandable. Committed-source uploads use the same progress model.

Build/push/contract failures leave retryable state without allocating compute.
Uncommitted files are excluded. Source transfer does not require a preliminary
remote Git push. Account and registry bindings stay machine-local; repository
YAML never contains credentials. See [setup](../cloud-workspaces.md),
[example YAML](../../crates/horizon-cloud/examples/cloud.yml) and
[worker image contract](../../examples/cloud-worker/README.md).

Disconnecting the client leaves worker sessions and tool services running.
Reconnect restores the same worker, SSH, tunnels and sessions. Persistent operation
identities fence uncertain creation responses before retrying. Missing workers or
processes are reported honestly, without silently launching replacements.
Stop and Delete are explicit operations; stopped storage can remain billable.

Optional Git credentials are bound to an explicit local repository and transferred
over SSH stdin to a private runtime file. A credential helper matches the selected
HTTPS repository, and the remote CLI receives credentials only in its child
environment. Agents can explicitly push their branch and create a PR. The feature
does not autonomously create or merge PRs. Removing a binding and successfully
reconnecting removes its worker copy; it does not revoke the original account
token. Broad tokens retain their original permissions, so use expiring,
repository-restricted credentials when possible.

## Architecture decisions

- `horizon-cloud` owns portable configuration, typed specifications/references,
  lifecycle states/errors, REST operations, reconciliation, progress and cancellation.
  It has no dependency on Horizon core/UI, terminals, browser/VNC or credential stores.
- Horizon core/UI own grouping, persistence, coordination and interaction. Local
  image preparation and optional Git credential transfer remain outside the provider
  crate. Credentials are supplied by the caller; no provider CLI is required.
- One container is the trust boundary for a cloud. Separate worktrees prevent
  concurrent file writes but do not guarantee conflict-free merging.
- Worker supervision, browser services and device input run remotely. The laptop
  must remain available through build/upload/bootstrap; ready sessions are independent.
- The generic root-based worker browser uses `--no-sandbox` within its dedicated
  container. Some agent commands require explicit approval outside a nested sandbox
  when the provider disallows user namespaces. This is not a multi-tenant boundary.

## Capability-selected worker images

Profiles select individual agents, browser engines and desktop support. Omitting
`capabilities` preserves the legacy full worker. An explicit `capabilities: {}`
selects the base SSH, Git, tmux, source/worktree and persistent-shell contract.
Native desktop profiles can enable their selected agents and VNC without browsers.
Browser profiles can select either supported engine or both. GPU requirements
remain mandatory; the coordinator does not silently fall back to CPU.

The same selection governs local contract validation, image construction, worker
bootstrap, credentials, tool registration and panel availability. Missing requested
capabilities fail before allocation. Disabled tools are absent from agent discovery,
and unavailable panels explain which capability the profile lacks. Reconnect
preserves browser engine and session identity.

Clean image recipes start from the appropriate base and copy final stripped
helpers. They do not add cleanup layers over obsolete development binaries.
Separate agent layers, compressed image manifests and first/cached local build and
push measurements are retained with the private image audit. Image-only deployment,
worker readiness and application-visible readiness are distinct measurements;
worker Ready is not reported as end-to-end application startup.

## Validation and evidence

The permanent [regression smoke guide](../testing/cloud-workspaces-mvp-smoke.md)
is retained for future runs. Exact commands and original evidence are associated
with the frozen candidate. Publication checks must be rerun on the final commit.

- Required local matrix passed: formatting, maintainability, 2,264 workspace tests,
  2,309 speech-tier tests, blocking Clippy and strict Clippy. Fifteen tests are
  ignored per Rust suite. Sixteen worker capability/authentication regressions pass.
- The complete serialized workspace retry passed after a parallel run exposed
  existing browser deadline timing failures. Advisory pedantic Clippy retains
  three unchanged device CLI test diagnostics and is not reported green.
- Independent review covered the implementation and subsequent navigation,
  deployment-progress, authentication and capability changes. Actionable in-scope
  findings were corrected and reviewed again.
- Real CPU/GPU checks cover immutable private-image deployment, committed source,
  separate worktrees, persistent agent process identities, browser/device control,
  credential transfer/removal and remote Git publication without merging.
- Creation uncertainty and duplicate prevention include a delayed HTTP integration
  test proving exactly one POST. Cancellation tests verify owned child termination.
  These tests do not claim injected live-provider faults or cancellation of a real
  registry upload. One real GPU Resume failed with HTTP 500; an earlier same-worker
  Stop/Resume succeeded. Provider errors remain visible and retryable.
- Capability regression coverage includes minimal/full profiles, individual agents,
  browser-free desktop control, disabled tools, missing requested software before
  allocation, multiple sessions, reconnect and cancellation.
- Native interactive validation used an isolated 3840×2160 desktop displayed live
  through a Horizon Device panel. Connection, receipt, presentation and advancing
  frames were checked; screenshots alone were not treated as live-view proof.
- Same-scale comparison against unchanged approved screenshots matched
  99.038–100% of static pixels within the declared tolerance. Dynamic content and
  intentional fixture/ownership labels were masked, without alignment or resizing.
- All four independent layouts, additional panels, inherited immutable membership,
  nested fullscreen, command-palette Escape, held-key handling, resize/Fit and
  persistence passed. Normal restart preserved three clouds, eight panel identities,
  independent layouts and session references. Controlled disconnect/reconnect
  preserved worker and agent process identities.
- Native 4K design and persistence videos were finalized and representative frames
  decoded and inspected. Public copies hide every non-Horizon application surface,
  terminal content and operational identifier. Unchanged originals remain private.

## Remaining gates and known limits

- [x] User accepted the candidate.
- [x] Final-image multi-agent, ownership and controlled disconnect/reconnect checks.
- [x] Same-scale design, resize/fullscreen/persistence checks and native 4K recordings.
- [x] Remote-agent Git publication smoke after the final runtime changes.
- [ ] Verify curated public attachments and downloadable example YAML.
- [ ] Complete current-head CI and the repository-required independent PR review.
- [ ] Rerun final smoke after current-head review and CI settle.
- [ ] Remove task-owned test compute and revoke test registry credentials; verify
      cleanup while preserving all pre-existing resources.
- Linux native graphics/input are verified. macOS and Windows native smoke are
  not claimed. Authentication for every optional agent is not claimed.
- Capacity, pull failures and provider Resume errors can require explicit retry or
  deletion. Destroyed workers lose running processes and Pod-local files.
- Machine setup uses documented private settings/files. The shared deployment
  example is an integration harness, not an installed user-facing cloud CLI.
- Full image-to-first-application-frame instrumentation remains a follow-up. Build,
  push and worker-ready timing must not be substituted for that measurement.

## Suggested next features, in priority order

1. Account/registry/Git setup UI with private credential-store bindings, short-lived
   repository tokens, rotation/removal controls and clear permission scope.
2. Cost estimates and spend ceilings before allocation, idle reminders, expiry
   policies and a resource/cleanup dashboard.
3. A per-agent changes view with explicit cherry-pick/merge/conflict handling and
   opt-in PR actions; retain human control over publication and merging.
4. Durable source/artifact checkpoints and explicit recovery onto a replacement
   worker, clearly distinguishing restored files from lost processes.
5. Installed CLI and public MCP cloud lifecycle parity using the same coordinator,
   followed by repeatable cross-platform/cloud acceptance automation.
6. Faster image workflows: cached image management, prebuilt profiles and optional
   remote builders for users who cannot keep the laptop available during upload.
7. Additional provider adapters after conformance tests; multi-container/Compose
   support only when concrete workloads require it.

Autoscaling, autonomous task scheduling and transparent recovery of destroyed
processes remain outside this MVP. None of these follow-ups is presented as delivered.

## Publication checklist

Do not name qualification repositories or expose their application details in
public issues, PRs, committed evidence or attachments. Public screenshots/videos
show Horizon only; hide other applications even inside its browser/Device panels.
Keep original captures and the detailed qualification ledger private.

- [ ] Create the issue with scope, limitations, remaining gates and next steps.
- [ ] Attach current overview, every cloud fixture and Horizon interaction evidence.
- [ ] Include example YAML inline and link its immutable downloadable file.
- [ ] Open one implementation PR linked to the issue; assign the repository owner.
- [ ] Complete current-head CI and independent review; resolve actionable threads.
- [ ] Repeat final smoke after review, then verify task-only resource cleanup.
- [ ] Await separate explicit merge authorization. No release is authorized.

## Accepted extension: remote mobile browser testing

The user requested this extension after accepting the initial candidate. It remains
inside this implementation PR and must pass the same review, final smoke and
cleanup gates. Repository names and application evidence remain private.

- [x] Declare the remote account capability and worker-local ports per profile.
- [x] Resolve existing machine-local credentials only with a repository-specific grant.
- [x] Transfer credentials over authenticated SSH stdin into private runtime files.
- [x] Share browser provider adaptation across UI and worker; expose public MCP tools.
- [x] Run the private Local tunnel on the worker independently of the laptop.
- [ ] Test iPhone and Android sequentially; retain provider-confirmed hardware identity.
- [ ] Email private application screenshots and results to the user.
- [ ] Cover omitted/disabled capabilities, missing grants, missing image support,
  reconnect and cleanup with retained regression tests.

Omitted declarations transfer nothing and cause no credential prompt. A declared
requirement without its local grant fails before allocation. YAML never grants
itself permission to export account credentials.

User scope update: remove per-device credential-transfer allowlisting while retaining
an explicit account-level local grant. Agents may discover and select any device/OS/browser
combination offered by the provider, without a preconfigured target. CLI and MCP
expose catalog discovery and selection under the same account policy; provider
entitlement and capacity remain authoritative. Additional private repository qualification is authorized after the
current acceptance gates; keep names, application captures and operational
details out of this public record. Those results will be reported privately.

Scope freeze, 2026-09-20: the user explicitly deferred the remote-device picker UI.
Remove its implementation and supporting presentation-only transport. Keep account
capabilities in YAML and device selection in CLI/MCP. The only approved visual pass
is polishing the existing New cloud form while preserving its current actions.
No issue picker, new provider UI, wizard or additional feature is authorized.
Continue acceptance, independent review, current-head CI, final smoke and cleanup.

New cloud presentation, 2026-09-20: implemented and locally verified; user
acceptance of this visual revision remains pending. The existing flow now uses
full-width fields, a clear type hierarchy, resource summaries for YAML profiles
and a fixed action row. Native VNC validation covered pointer opening, CPU/GPU
selection, 900×600 and the actual 800×600 minimum, Escape and reopening at 4K.
The retained regression suite covers profile choice, terminal-input isolation,
Tab containment and close/reopen layering. Independent review found a retained
layer-order defect on reopen; the fix and a meaningful pointer regression passed.

Private evidence is retained separately under `development-51/`: candidate and
validation manifests, native-resolution screenshots and decoded 4K recordings.
Candidate binary SHA-256:
`b52899e04838de0a11eda232914aacffd9eefc63adcb311390fa0bcafe92f740`.
This is a dirty local candidate based on `345633f671afc5a8fa5cfb25d9754101fed4e039`,
not a new published or accepted commit. Full repository validation with
loopback/PTY access passed formatting, maintainability, worker scripts, both
workspace test tiers and blocking/strict Clippy. The advisory pedantic tier
reported three unchanged device CLI test warnings. The private
`final-validation-05/` record identifies the tested source manifest. A prior
restricted-sandbox run could not bind mock server sockets and is not treated as
product-test evidence. Independent local review of the final source is clear;
current-head hosted review, CI and the final post-review smoke remain pending.

Timeout reconciliation follow-up, 2026-09-20: a live remote browser timed out
before application navigation. The provider's exact-session report confirmed
termination, but the previous recovery code retained capacity because it did not
recognize the execution statuses `timeout` and `error`. These statuses now release
only after a successful report with the matching session identity. Editable test
metadata, identity mismatch, authentication failure and uncertain transport still
retain the hold. Independent review found no actionable issues. Regression tests
cover both terminal statuses and misleading metadata. All required validation
tiers passed in `final-validation-06/`; the same three advisory device-test lint
findings remain. Live proof of the updated recovery path and successful mobile
application captures are still pending and are not implied by these local results.

The New cloud and remote browser changes are committed locally as
`359221c844132f49730ed9e077575c622d3af221`; publication and current-head review
will include this follow-up. The original public candidate's review and smoke
do not validate the additional behavior. Detailed private application qualification
and resource ledgers remain outside the public repository.

Review corrections, 2026-09-20: source validation now rejects oversized attributed
LFS pointers using a bounded, cancellable read; ordinary files remain supported.
Remote browser process loss remains distinct from transport interruption. Inert
lost-process views can be closed, and failed attachments retain only their pending
identities so retries neither replace healthy terminals nor reopen dismissed views.
Worker preflight checks the Python APIs required for safe source import. Independent
local review is clear after adding the packed-blob, cancellation and partial-retry
regressions. The apparent progress-rate finding was disproved by a preceding-stage
regression; the timestamp baseline cancels in the sample-duration calculation.

Next-phase gate, explicitly requested by the user: finish current-settings
acceptance and the current-head hosted review first, then rerun final smoke and
delete every task-created compute allocation while preserving pre-existing workers.
Only after those gates, start a clean-state UI setup walkthrough. Record the missing
repository-YAML path, agent-assisted configuration preparation and explicit profile
reload as candidates for that phase. Machine-local provider and agent API-key
bindings currently have a file-based setup contract; there is no cloud credential
entry UI yet. Do not implement a new setup wizard, picker or credential UI before
the review gate. No merge or release is authorized.

Next-phase design preference: the user requests a polished Cloud settings pane
for compute credentials and agent authentication, with a clear choice between
supported subscription login and API-key authentication. Evaluate its relationship
to existing remote-browser settings, keeping credential storage machine-local and
the repository YAML free of secrets. This preference does not lift the review,
current-settings smoke or task-compute cleanup gates above.

Final review batch, 2026-09-20: expired browser-create requests are rejected
before allocation and retired before ready results can be published. Automatic
cleanup polls shutdown without blocking the worker loop and retains uncertain
release identities. Authenticated malformed catalog deadlines saturate safely.
Missing-process placeholders preserve their saved remote target through persistence.
The remote-only image smoke expects its declared browser tools. Live acceptance
also found that worker video capture used the wrong retention directory; captures
now use the encoded per-panel profile path. That capture correction still needs
cloud verification on the rebuilt immutable image.

The nine-file review batch passed independent review and every required local
validation tier in `final-validation-11/`: 2,304 workspace tests and 2,349 tests
with speech, zero failures, 15 ignored in each tier. The advisory tier retains
three unchanged device CLI test diagnostics. Existing immutable images passed
minimal, remote-only and full-browser capability smoke, with task-container cleanup
confirmed. Native candidate `development-54/` reattached after restart; the existing
form was checked at 900×600 and the 4K resize recording was decoded. Its binary
SHA-256 is `5afe45b47544591980edfebc96917c8fded773a416bbc55d23819d9830eb4993`.
Hosted review, final immutable-image acceptance and post-review smoke/cleanup
remain open; these local results do not close those gates.


Sixth hosted-review disposition, 2026-09-20: five narrow findings were confirmed.
Preflight now validates the Ed25519 wire structure, rejects empty selected-agent
credential files, and rejects non-UTF-8 tree components. Browser-only attachment
retries keep repainting after discovery stops and retire dismissed placeholders.
Sidebar workspace focus includes empty/collapsed cloud frames and runtime cards.
SHA-256 Git repositories remain unsupported by the current source reader; they
now receive an explicit preflight error rather than a misleading submodule error.
This does not add a new repository format. The portable provider crate reuses the
existing workspace base64 dependency solely for public-key wire validation.

Two other hosted suggestions do not establish the claimed failure: Ready cards
already expose explicit reconnect, which recreates a failed desktop tunnel, and
repeated remote-browser release is idempotent in both worker shutdown and private
file removal. Independent review of the narrow fix batch is clear. Regression
coverage includes malformed keys, selected/disabled credential bindings, nested
invalid-byte files/directories, unsupported source format, attachment recovery,
dismissed placeholders, and sidebar selection of cloud-only workspaces. Required
validation and exact-candidate live proof are being rerun; no new settings UI or
other feature expansion is included in this batch.


The sixth-review fix batch passed all required local tiers in
`final-validation-12/`: 2,311 workspace tests and 2,356 tests with speech,
zero failures and 15 ignored in each tier. Blocking/strict Clippy, formatting,
maintainability and worker/helper checks passed. The advisory tier still reports
only the three unchanged device CLI test diagnostics. Independent review is clear
on the 12-file code/test/dependency batch. The final immutable web worker reached
Ready and its complete applicable application suite passed; mobile video capture,
current-candidate UI smoke and the next hosted review remain pending.


Additional queued UI preference: replace the floating in-canvas provider/control
strip with a **Cloud** menu in the top toolbar. Reuse the existing New cloud and
Fit all clouds actions; do not add a provider/issue/device picker. This belongs to
the already-gated UI polish phase alongside Cloud settings, after current review,
post-review smoke and task-compute cleanup.


Seventh hosted-review disposition, 2026-09-20: independent reproduction disproved
both the claimed missing-parent clone failure and the claimed restored-agent
placeholder failure. Existing member-attachment restoration handles placeholders;
Git creates nested clone destination parents. Four verified defects are corrected
in the next bounded batch: retained allocation recovery after worker-service loss,
remote-target identity after transport errors, rejected-resize controller attribution,
and periodic repainting of idle Ready clouds. Private recovery journals retain exact
session identity without credentials, bind to the original provider/account and
owner, and clear only after provider-verified release. Trusted host revocation can
reconcile ownerless UI allocations. Legacy identity-less journals remain explicitly
unresolved. Pre-launch persistence failure rolls back its unallocated admission.
The macOS invalid-byte-name regression now constructs Git trees directly instead
of asking the host filesystem to accept an invalid name. Independent review and
final validation for this batch remain in progress; no new UI features are included.

Live mobile acceptance additionally found that an explicit browser close returning
pending shutdown was not queued for deferred cleanup. After exact-session release,
its local presentation could retain the last frame. Explicit closes now join the
existing cleanup queue until shutdown and release are confirmed; the regression
covers failed close followed by provider-confirmed release and automatic removal.

The seventh-review correction batch passed independent local review on its exact
17-file source manifest and every required local tier in `final-validation-15/`:
2,317 workspace tests and 2,362 speech tests, zero failures and 15 ignored in each
tier. Formatting, maintainability, worker checks and blocking/strict Clippy passed.
The advisory tier retains the same three device-test diagnostics. Frozen native
candidate `development-56/` is visible through the task-owned native VNC panel;
its binary SHA-256 is
`24fc0c8f59970bbe3e2be352045c91b56bb75f94c32ef6bfc3d2eea5f8c85441`.
Both physical mobile operating systems have now completed private application
interaction and decoded video capture on the preceding immutable worker image,
with exact-session release confirmed. That evidence verifies the capture fix but
does not validate this batch's retained-allocation or deferred-close corrections.
Fresh immutable-image acceptance, current-head hosted checks/review, post-review
smoke and cleanup remain open. The queued Cloud menu/settings phase remains gated.


Eighth hosted-review disposition, 2026-09-20: four recovery findings were confirmed.
The desktop presentation now retains its loopback listener throughout its lifetime,
relays each bounded viewer connection over authenticated SSH and uses one absolute
readiness deadline. Shutdown closes sockets and reaps the owned transports before
joining threads. Accepted sockets explicitly use blocking mode across platforms.
Persistence reconciliation can repair a cloud member's saved workspace without
unlocking user moves. Worker browser fences now return typed process loss, stop
client polling and disable Retry while retaining engine and target identity.

Independent local review is clear on the eight-file source manifest
`efa054b44130742c3b495d8d6939eb941fa932f7fd03bc108071daa1a3ff4ce2`.
Targeted regressions pass; full validation is recorded separately in
`final-validation-16/`. Optional automatic desktop retry and remembering locally
dismissed terminal views are outside this correction batch; explicit reconnect
and its documented session restoration remain the MVP behavior.

The first-use acceptance story is now explicit for the gated UI phase: start an
empty ephemeral Horizon profile, choose Cloud → New cloud, enter the compute key,
select one or several supported agents, choose API-key or supported subscription
authentication per agent, prepare/reload a repository profile, then deploy. Keep
good defaults centralized, advanced settings secondary and errors actionable.
The walkthrough must succeed without inherited secrets or manual hidden-file
edits, including missing YAML, keyboard navigation, cancellation and resume.
A real supported login flow is required; do not present unsupported subscription
transfer as automatic. This is requested work, not yet implemented or verified.


The eighth-review batch passed formatting, maintainability, worker checks and both
required Clippy tiers. Workspace tests passed 2,324 with zero failures and 15
ignored. The first speech run hit the existing dead-host CLI startup assertion;
its complete log is retained. A full speech-tier retry passed 2,369 with zero
failures and 15 ignored. Advisory diagnostics remain the same three device-test
findings. Frozen native candidate `development-57/` has SHA-256
`d5b460db18e837b42b426fa3363ecc6543ca7125a364f1061dbd92ab74760555`.
It restored the existing remote session through the live native viewer. A later
isolated-display interruption is retained as an unconfirmed fixture failure;
the task desktop was restored without changing remote workers or shared sessions.
Fresh final-image and post-review smoke remain pending.
