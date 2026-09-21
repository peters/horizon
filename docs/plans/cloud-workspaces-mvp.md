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
| Guided cloud accounts and missing-config setup | Yes | Native settings, resize, real setup agent and YAML reload | Pending final candidate | Pending |
| Automatic native viewer recovery | Yes | Native recovery, ownership and reconnect | Not a compute feature | Pending |
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

Post-review container-restart correction, 2026-09-20: the preceding candidate
passed hosted checks and its ninth review, but the required live fault test found
a tunnel recovery defect. Runtime files survived container restart while the
supervisor did not, leaving the old shutdown fence permanently unresolved. New
images record container incarnation before spawning the tunnel and accept either
matching shutdown evidence or a proven container change. Legacy/unverifiable
records remain fenced. Changed account bindings cannot overwrite credentials
until existing remote allocations are released. The explicit release action also
remains available after reconnect failure, with an independent completion channel
that does not erase the original setup failure or falsely promote the cloud to Ready.

Independent local source review is clear on manifest
`f30373304e15d1b3d08f6485016fa5a896c3ec3b0f67535caf963976ed4545fb`.
The smoke plan retains the container-restart, credential-preservation and failed-
reconnect release scenarios. After incorporating the separate current-PR update
from main, `final-validation-19/` passed all required tiers: 2,328 workspace tests,
2,373 speech tests, zero failures and 15 ignored per tier. All worker regressions,
formatting, maintainability, blocking and strict Clippy passed. The advisory tier
retains three unchanged device-test diagnostics. An earlier five-second CLI test
startup timeout is retained; its focused rerun and the complete final matrix pass.
Frozen native candidate `development-60/` has SHA-256
`3cea9b694d14c1663c6018a3e4e3f173f6ebb5ebbf51cdf23e1220cd9a531299`.

The corrected immutable test image reached worker Ready in approximately 178
seconds from the native Deploy action. This is worker readiness, not application
startup. Its fresh rebuild reused existing base layers; cached-build measurements
and compressed-layer sizes are retained privately. Live application, process-loss
recovery and the next current-head hosted review/final-smoke gates remain open.
Hosted-device release on the preceding worker was confirmed through the public
allocation tool; private copies remain on that task worker until verified cleanup.
No shared checkout, baseline worker, merge or release was changed.

The queued Cloud onboarding/menu phase also includes readiness timing: display
elapsed setup time while working and the completed time to worker Ready. Show an
ETA only when grounded in measured progress; unknown stages show activity and
elapsed time. Keep initial deployment separate from reconnect and from application
readiness. This requested UI work remains gated on the current-settings acceptance,
review, post-review smoke and task-resource cleanup described above.


Final restart acceptance, 2026-09-20: the corrected immutable image completed
private application qualification with 5,901 tests passing, 35 not executed and
zero failures. Two real interactive agents proved separate worktrees and observed
shared browser views without taking control. Both local browser engines and a
physical mobile device completed private navigation/capture checks. A test-only
map integration error is retained privately; that application feature is not
claimed as passed. The latest mobile export contains one frame and therefore
proves a screenshot, not movement.

The deliberate container restart retained worker identity, recovered the private
tunnel using its new incarnation and reported vanished processes without silently
replacing them. Exact hosted-allocation release was confirmed through public MCP
before the explicit cleanup retry stopped the tunnel and removed copied runtime
credentials. A bounded SSH authentication interruption then verified failed release
and successful retry. Cleanup clears its own stale error while preserving a separate
discovery failure. Authoritative Stop/Snapshot cleanup also clears stale release
errors. The permanent smoke plan includes these regressions.

Independent review is clear on the final six-file correction manifest
`ff3b45922957add84a801830dde704cb8a8ab74698208f8adf7448440b1bc224`.
Pending browser discovery is represented separately from an authoritative empty
result, eliminating the redundant flag exposed by the updated advisory checks.
The complete final matrix, frozen candidate and hosted review receipts are being
updated after preserving the latest CI-only update from main. The user agreed to
keep the PR head stable through this review-and-smoke pass. New Cloud setup/menu
work remains queued behind review, final smoke and verified task-resource cleanup.


The combined candidate passed `final-validation-23/`: 2,330 workspace tests
(15 ignored), 790 speech-feature UI tests (zero ignored), all worker regressions,
formatting, maintainability, device CLI checks, and blocking/strict/advisory lint
tiers. The speech scope follows the updated repository matrix. The CI change-
classification and dependency-installer suites also passed 18 tests. The earlier
complete speech workspace tier passed 2,375 tests. Frozen native candidate
`development-62/` has SHA-256
`99413d0636ecb1472e8f0157d9190c9fd345a341c9883ec9611823b9e6e2cbd7`;
its actual isolated application PID and hash were verified. It restored existing
cloud membership/lost-process presentation and created a fresh agent through the
normal in-cloud picker. Hosted review/CI and post-review final smoke remain pending.

Frozen-head review pass, 2026-09-20: all applicable hosted checks passed on
`fffa92b892f7d61c84cd537505da230c5ae87af1`. The review returned two comments.
Independent inspection retained automatic desktop-tunnel retry as a follow-up:
the documented explicit Reconnect action already retries its setup. Sidebar
detachment of cloud workspaces is a confirmed in-scope defect. Its correction is
local and uncommitted while the user-requested PR head freeze remains active.
It guards detachment centrally and in the sidebar, restores older detached-cloud
states into the main window, and refuses cloud creation in a detached workspace.
Focused cloud UI regressions passed 46 tests; full validation and native regression
of this local correction are tracked separately from the frozen candidate.

The frozen candidate preserved two agent process trees, separate worktree files,
branches and tmux identities through a clean 90-second client disconnect. An agent
also completed a bounded file assertion and public browser inspection while the
client was absent. The same browser returned and the selected Grid layout survived.
The private clean reconnect video contains 2,400 decoded 3840x2160 frames across
four minutes. A preceding test-launcher failure interrupted its local fixture and
recording; those artifacts are retained separately and are not counted as the clean
recording. The launcher was corrected without changing the candidate binary.
Current-head review closure, post-correction smoke and task-resource cleanup remain
open. No new compute was allocated during this pass; no merge or release occurred.

The local detachment correction passed `final-validation-25/`: 2,334 workspace
tests (15 ignored), 794 speech-feature UI tests (zero ignored), the worker and
device suites, formatting, maintainability, and blocking/strict/advisory lint
tiers. The additional full workspace speech command passed 2,379 tests with zero
failures and 15 ignored. A build without default features also passed. Independent review of the
final seven-file source manifest is clear:
`e18bf69fd8f1bfb66203c735a4fa7ab098032a6e6d3258bb7d743670af0673c1`.

Frozen local candidate `development-64/` has SHA-256
`7758345f405765adf794681af5bddd890538d4bd7085a693869724c95d700a78`.
The native Device panel displayed advancing frames during the regression pass.
A real saved detached-cloud state restored into one main window, the disabled
sidebar action could not detach it, ordinary workspace detachment and reattachment
still worked, and resize plus cloud fullscreen passed. Creation in a detached
workspace was covered by the regression test, not an interactive deployment.
Two private 4K recordings contain 1,800 and 600 decoded frames; representative
frames were inspected. Receipts, checksums, source hashes and the exact application
PID are retained in `development-64/`. These captures include operational details
and are not approved public evidence.

Both local and hosted PR heads remain `fffa92b8`. The correction is implemented
and locally verified, but uncommitted and unpublished under the user's head
freeze. Its hosted review thread stays open until the correction reaches the PR.
The next step requires permission to advance that head, followed by hosted review,
affected final smoke and verified task-resource cleanup. The queued onboarding
phase has not started.

Follow-up on 2026-09-21: the user authorized the validated detachment correction,
which was pushed as `cb364664bcfebd93fb80331c7e8f5383a7001517`. Every applicable
hosted CI lane passed. The new review accepted the detachment fix and raised one
release-journal recovery finding plus two layout observations. Independent
inspection confirmed all three. Their corrections are being validated locally as
one batch: durable released identities no longer require old provider credentials;
cloud placement and initial Fit use consistent workspace coordinates; parent
sidebar layouts and ordinary resize collisions preserve cloud members.

The released-journal regression also verifies conservative handling of unreleased,
nonprivate, malformed and mismatched identities. Its independent source review is
clear. The layout review additionally caught initial Fit occurring before origin
reconciliation; this is corrected and covered for a workspace far from the origin.
The current PR head stays fixed during this local validation. Post-review smoke,
verified cleanup and the queued first-use UI remain open. No new worker was
allocated for the detachment push; the task-owned worker inventory and protected
pre-existing resources were verified before preparing cleanup.

The user additionally requested a separate playground configuration containing all
repositories exercised during acceptance, prepared after the remaining delivery
gates. Keep the current Horizon instance, settings and sessions running untouched.
Use a distinct configuration/session for the playground, with machine-local
credential bindings and deliberate deployment actions; do not include private
repository details in public issue or PR prose. Verify the playground against the
actual completed acceptance inventory rather than including untested examples.

The settled review correction passes all ten lanes in `final-validation-28/`:
2,341 workspace tests and 2,386 full workspace speech-feature tests, with 15
ignored tests in each tier; worker/device suites, formatting, maintainability and
all lint tiers also pass. The no-default-features build passes. Independent source
review is clear for manifest
`8dcd4941da7b31241dae4850f8470a4d5af24ed821371ee7a0c16b86dc906592`.
Earlier validation attempts retained test-lint/import failures; they are not the
passing receipt.

The frozen `development-67/` candidate has SHA-256
`65b58fe2e0e246f731902668e76c6e39a584b93213a531de7197ee78fcddfc88`.
Native checks observed disabled parent layout controls and two synthetic cloud
cards at the correct positions in a translated workspace, including the expected
48-unit gap after the taller runtime card. No compute was allocated by those
creation checks. The initial synthetic fixture lacked a committed revision and
correctly failed validation; it was then initialized with a synthetic commit.
The native viewer reported displayed, advancing frames during these checks, but
intermittently reported no presentation afterward. Two private 4K recordings
retain 1,800 and 2,400 decoded frames. They contain private operational content
and are not public attachments.

A clean immutable worker image with the corrected release recovery passes the
local contract without network access or credentials. Its service image grew
from 1,464,068,186 to 1,464,074,380 compressed bytes. Build/push took 66.13/13.13
seconds; cached build/push took 0.74/0.51 seconds. These are image-preparation
measurements, not worker or application startup timing. New-image cloud
qualification and the post-review final smoke remain pending.

The user requested automatic native viewer diagnosis and recovery. Separate
[issue #801](https://github.com/peters/horizon/issues/801) records the observed
intermittent status and the missing presentation/freshness diagnostics. The
viewer was closed and recreated with permission; both Horizon applications
continued running. Test guidance now requires bounded automatic diagnosis and
precise blocked reporting instead of repeated human visibility confirmation.

The correction was published as `3f90759be75c4691a7340da89f86229b615bde40`.
All applicable hosted checks passed; the current-head review arrived on
2026-09-21 and found a release-persistence failure path plus an enabled Default
action in the cloud workspace context menu. It also raised legacy-checker
compatibility: the supplied historical checker ignores added arguments, but a
strict legacy checker can reject them. All three are addressed locally as a
bounded correction batch. No new UI flow or provider is added.

Failed release-journal writes now retain both the exact identity and confirmed
release outcome. Reconciliation retries persistence before allowing cleanup,
including cancellation before any provider identity exists. Local image checks
and SSH readiness share environment-based capability transport; modern minimal
images still reject full defaults, and modern readiness still checks services.
The combined readiness timeout preserves the original budget for each check.
Focused durability, cleanup, contract and capability regressions are retained;
the complete matrix and final native smoke are separate gates.

The fresh immutable service image reached worker Ready in 119.81 seconds through
the shared deployment coordinator. This includes source transfer and private
tunnel setup, not application readiness or first visible frame. Real agents
started in separate worktrees. Their bounded work and reconnect checks are in
progress, with results held in private evidence. The worker must not be described
as qualifying the later uncommitted correction.

Issue #801 now explicitly requires automatic freshness diagnostics and an
owner-scoped public presentation operation. Timestamped current observations
show a connected viewer with no confirmed presentation after bounded recovery.
That blocks interactive native smoke, not headless tests. No human visibility
confirmation is required, and neither Horizon application is restarted to repair
the viewer. The API/runtime follow-up remains unimplemented.

The settled durability/compatibility correction passes `final-validation-30/`,
including a repeated complete workspace tier after the last guarded transition:
2,345 workspace tests and 2,390 speech-feature tests pass, with 15 ignored in each
tier. All ten validation lanes pass. Independent review is clear for source
manifest `fb1cc45ea3255891446dc04b921d1e9603e0f0a1c4b9a382b8869d93285efbd9`.
The preceding pass retained a state-model lint failure and is not the final receipt.

On the preceding immutable image, both real agents passed worktree/branch/marker
assertions and controlled separate public-browser sessions. A 134-second client
disconnect preserved both agent and browser-tool process identities; an agent
completed a timestamped assertion and inspected its existing browser during that
interval. This is headless cloud evidence, not native UI presentation or evidence
for the later worker correction. Browser control leases expire after inactivity;
an idle owner value must not be mistaken for lost cloud or panel membership.
The later correction still needs hosted review and affected final smoke.

The current candidate is `c9526b933dbefdf1f868b57f9029f3f8ae3b1918`.
All applicable hosted checks passed and the current-head review arrived with no
inline findings; all review threads are resolved. Its additional startup-I/O
observation was independently triaged: journal loading can delay a first frame,
but lock contention is nonblocking, explicit Deploy/Reconnect retries the load,
and missing or unreadable records remain fenced before allocation. Background
loading and bounded retry are retained as a separate responsiveness follow-up.

At the user's request, the repository and installed native-device skill now
prohibit human visibility-confirmation prompts. The shared smoke procedure uses
timestamped public inspections, distinguishes static content from a proven stall,
limits recovery to the task-owned viewer, and carries recovery budgets across
resumed turns. Three further public inspections retained the
`presentation_unverified` outcome. No reconnect loop, desktop automation or host
restart was used. The missing runtime telemetry/reveal operation remains open in
#801; these instructions do not implement that product behavior. The latest
documentation changes are local, and the reviewed PR head remains unchanged.
Final native smoke, final-image cloud qualification and cleanup remain pending.

### Approved continuation, 21 September

The user approved cleanup first, requested the automatic VNC product fix last,
and removed playground preparation. The active Horizon instance remains intact;
no merge or release is authorized. The hosted head remains `c9526b93`; subsequent
setup changes are local and require a new complete validation/review pass.

Resource reconciliation confirms 23 of the original 24 task workers removed,
with all five protected baseline workers preserved. One legacy task worker is
retained because its hosted-device release remains unresolved. One retirement
required a narrowly scoped operator cleanup after public device-release proof
and verification that the recorded tunnel process was absent; this is not proof
that the latest product lifecycle passed. Private receipts are retained under
`../cloud-workspaces-evidence/final-delivery/retired-workers-20260921/`.

A fresh deployment of the final immutable worker image reached provider create
and received HTTP 500. Its durable operation remains Requested. Subsequent
read-only reconciliation found no matching worker; that does not establish that
the request was never applied and does not authorize another create request.
The original operation identity is preserved. Logs and reconciliation receipts
are under `../cloud-workspaces-evidence/development-68/`.

Local implementation now includes a Cloud toolbar menu, private account setup
with selected agents and API/subscription modes, dedicated SSH identity creation,
and a missing-YAML route through a real local agent. That local setup terminal
uses its existing local authentication; remote API bindings are not silently
exported into arbitrary local processes. Saved agent preferences are passed as
nonsecret setup context, while existing YAML profiles remain authoritative.
Failed YAML reloads invalidate stale profile selections. Readiness records the
successful attempt duration separately from application-visible startup and
reconnect; no speculative full-deployment ETA is promised.

Seven focused core setup tests pass, including actual SSH key generation,
private credential storage, rollback, stale saves and intervening external edits.
Toolbar layout tests and menu/modal reopening pass. Independent review identified
durability, Windows file-handle, Escape-gesture and preference-propagation issues;
corrections and affected regression tests are in progress. The retained first-use
plan is `docs/testing/cloud-first-use-smoke.md`. Native visual acceptance, final
immutable-image cloud scenarios, the complete matrix, current-head hosted review
and final cleanup are still pending. No new screenshots or final pass are claimed.

### Local candidate verification, 21 September 07:20 UTC

The setup, readiness and automatic native-view changes pass all ten lanes in
`../cloud-workspaces-evidence/final-validation-32/`: 2,362 workspace tests and
2,407 speech-feature tests, with 15 ignored in each tier; format, maintainability,
worker capability/auth checks, device CLI checks and all three lint tiers pass.
The additional build without default features also passes. Independent review is
clear for the unchanged 32-file Rust manifest
`4aa940891a5cecda7ba7eb789dce38dee09b9b736afc229179c792e0f63c9b23`.
The validation source manifest was rechecked before this documentation update.

Automatic native-view support is now implemented locally, superseding the earlier
unimplemented status: owner-scoped Reveal restores presentation without taking
keyboard focus or changing active workspace, including detached viewports.
Independent decoded-frame and presentation diagnostics distinguish paused sampling,
clipping, missing frames and disconnection. Legacy hosts remain compatible.
The retained regression plan is `docs/testing/device-automatic-presentation-smoke.md`.

The frozen candidate is in private `development-69/`, based on `c9526b93` plus
the reviewed local changes. Its application SHA-256 is
`564a404b43090e4000b22075ee0ce504dcf6055f0e22a1898d756a866118f673`.
Its actual child executable was verified on the existing task-owned desktop.
A 4K startup diagnostic capture shows the Cloud toolbar entry and independently
changing terminal heartbeat. This capture is not interactive acceptance evidence.
The user's current viewer host lacks Reveal/diagnostics; timestamped public
observations still do not establish live presentation. Native interaction and
final video remain blocked without changing or restarting that user instance.
No repeated reconnect or human visibility-confirmation prompt is used.

Read-only reconciliation at 07:17 UTC still found no worker for the uncertain
create operation, and all five protected workers remained present. The original
Requested operation is retained; an empty list is not permission to create again.
The remaining legacy worker's hosted-device report is terminal, but the initial
correlation sample was insufficient for complete cleanup proof. A paginated,
read-only reporting audit is in progress; product release journals remain intact.
Final-image cloud qualification, native acceptance, new-head hosted review/checks,
and final resource/credential cleanup remain open. Nothing is merged or released.

### Cleanup checkpoint, 21 September 07:23 UTC

All 24 known task workers are now absent. The provider inventory contains only
the five protected pre-existing workers. The last legacy worker was retired by
an explicitly recorded operator action after paginated provider reporting proved
its sole correlated hosted-device session terminal; independent review accepted
that bounded evidence. The old unsupported release response and journal remain
unchanged. This is cleanup proof, not a successful product lifecycle test.

All three task registry pull bindings, both temporary repository-scoped registry
tokens and their scope maps are revoked and verified absent. Registry images are
preserved. Receipts are in private `final-delivery/registry-cleanup-receipts.json`
and `final-delivery/retired-workers-20260921/`. Resumed cloud testing will require
fresh expiring registry credentials; existing deleted bindings must not be reused.

The uncertain create still has no matching worker. Its operation remains fenced
and is not reported resolved merely because the current inventory is empty.
Native acceptance still needs the running viewer host to load the new automatic
presentation support. The user was asked for explicit permission for one
controlled host restart because the prior instruction preserves that instance.
Until answered, neither the host nor its sessions will be restarted. The public
PR head is unchanged; the reviewed setup/viewer changes remain local, with no
merge or release. The playground remains out of scope.

### Ephemeral viewer established, 21 September 07:35 UTC

The user selected a separate ephemeral instance instead of restarting the main
host. The frozen reviewed candidate now runs on the user's display with a clean
private home/config and `--ephemeral`. Its workspace is named
`Cloud MVP — temporary viewer`. The main host's PID and process start identity
are verified unchanged. No account credentials or saved sessions were imported.

A clearly labelled verification controller in the new workspace uses public MCP
to create and Reveal the native VNC viewer for the existing isolated test desktop.
Six timestamped observations confirm connected, received and displayed frames;
uploaded frame sequences advance 5, 11, 16, 22, 27, 33. New diagnostics independently
report decoded progress and displayed presentation. Actual child executable/hash
matches the frozen candidate. Receipts are in private `ephemeral-viewer-70/`.
The controller is a test harness, not evidence of a coding-agent login.

This supersedes the old-host live-view blocker without restarting that host.
It establishes the viewing prerequisite; first-use UI, layout, persistence,
reconnect and video acceptance remain to be executed in the isolated target.

### Native first-use and resize checkpoint, 21 September 09:15 UTC

The separate ephemeral viewer now establishes live presentation automatically;
main-host restart and human visibility confirmation are no longer prerequisites.
Public observations show connected, received and displayed frames advancing on
the same connection generation. Both the main host and viewer process identities
remain unchanged while only the isolated candidate window is replaced normally.

Native first-use checks passed for missing-account routing, cancellation without
writes, masked synthetic credential entry, individual API/subscription choices,
continuation to New cloud, blank replacement fields preserving saved bindings,
and missing-YAML guidance. Private credential files are mode 0600. Synthetic
markers are absent from settings JSON and logs. These checks allocate no compute
and do not establish actual agent authentication or repository bootstrap.

The native 800x600 resize exposed clipped Cloud settings. A regression reproduced
it, and review identified the same footer sizing pattern in New cloud. Both
footers now request a bounded height; the creation body reserves sufficient room
for its heading and actions. Full-app regressions cover shrink from 4K through
900x700 to 800x600, scroll-saturated content, whole-dialog containment and unclipped
footer labels. All 41 focused production tests passed, as did the strengthened
settings regression. Independent review found no further actionable issue in the
four-file correction (manifest `37b058f6be231364bfc413e918736a3e5c1b77f4912d375531f5e263c16faa8e`).

The corrected frozen native candidate is private `development-71/horizon`, SHA-256
`2a1411c736376304964614b7c448e9681c0631526a18c987b0b0d41c13971200`.
Its actual application child and private profile were verified. Validation 33
supersedes validation 32 for these source changes. Initial workspace/speech runs
were denied loopback mock-server binds by the restricted sandbox; their logs are
retained and both suites are rerunning with local socket access. Other lanes pass.
No commit or push has occurred during this correction.

The initial X11-grab recorder stalled and its empty files are not evidence.
A continuous native X11 frame recorder produces valid 3840x2160 video. The first
settings clip contains 559 frames; the pre-fix resize clip contains 598. Decoded
frames were inspected. Capture timestamps are retained separately: constant-frame
encoding is not a wall-clock deployment benchmark. All captures remain private.
Corrected native resize, remaining UI/provider acceptance, final review/checks and
final smoke remain open. No merge, release or playground preparation is performed.

### Corrected native candidate, 21 September 09:35 UTC

Validation 33 now passes all ten required lanes. The network-enabled reruns pass
2,364 workspace tests and 2,409 speech tests (15 ignored in each); initial socket
permission failures remain recorded separately. Rust source hashes still match
the frozen validation manifest. Native screenshots confirm both corrected dialogs
fit at 800x600, New cloud survives the 4K/900x700/800x600 shrink sequence, scrolling
keeps the footer visible, Escape restores the overview, and toolbar overflow keeps
Cloud accessible. Normal restart preserves settings and both synthetic credential
bindings byte-for-byte. This is synthetic first-use proof, not remote login proof.

The automatic native-view lane now has live public-MCP evidence on the corrected
candidate, viewed through the separate ephemeral native panel:

- A different agent's mutation is refused with `not_owner`.
- Hidden and off-canvas viewers report paused sampling and the correct reason;
  one Reveal restores displayed, advancing frames on the original connection.
- Reveal exits an obstructing panel fullscreen without taking panel focus.
- Detached off-canvas Reveal restores fresh frames while the separate root window
  keeps OS focus; both native window geometries remain unchanged.
- Stopping the owned synthetic VNC target reports disconnected and no displayed
  image. One explicit reconnect starts generation 2 with zero sequences and no
  received/displayed flags, then reports genuinely new advancing frames.

Private receipts and continuous 4K video are in `development-71/`; representative
video frames were decoded and inspected. The native test controllers are labelled
harnesses using real Horizon identities and public MCP, not coding-agent sessions.
Cloud-group collapse, cross-workspace active selection and restored-viewer ownership
have headless coverage but are not claimed as new native passes here. Known cloud
compute and registry cleanup receipts remain unchanged. Read-only provider
reconciliation at 09:21 still finds no worker for the uncertain create; no second
POST or replacement operation is issued. Main Horizon remains untouched.

Cross-workspace native follow-up also passed: selecting a second workspace,
revealing the first workspace's Device panel, and creating a terminal placed that
terminal under the second workspace. The nested viewer was closed through public
MCP; all synthetic target processes from that pass are verified absent. The outer
live viewer remains open. The 09:43 private progress email includes a 4K Horizon
capture. Final independent local source review is clear for Rust manifest
`85b900984d4b703adbbe3d101158f8a5a67aff96fa0df399ed0d4e5cdceed7f6`.
The actual missing-config setup-agent smoke is now underway in a disposable
repository, using private runtime authentication and no image build or allocation.

### Real repository bootstrap, 21 September 10:11 UTC

The missing-config action launched the actual selected local agent in a disposable
repository. API authentication succeeded. The agent inspected the supplied worker
references, created an explicit two-agent/no-browser/no-desktop CPU profile, copied
the required worker helpers, documented stripped-binary prerequisites, and passed
the repository unit test plus offline YAML, copy-integrity and syntax checks.
New cloud loaded its `development` profile through the real schema. Replacing the
fixture YAML with malformed content cleared the profile and disabled creation;
restoring the original file allowed it to load again.

This proves local onboarding and configuration reload, not deployment: the image
was an explicitly labelled placeholder, and no image build, push, commit or worker
allocation occurred. The private agent install needed its matching tool-host
executable; nested sandbox creation was unavailable, so individual bounded fixture
commands were approved inside the existing outer isolation. Temporary local agent
authentication was removed afterward. Generated YAML, private receipts and native
4K recordings are retained separately from the original prototype evidence.

All runtime source still matches validation 33 and its independent review. User
documentation now describes guided accounts, missing-config preparation and the
distinction between per-attempt timing and persistent worker-readiness duration.
Current-head hosted checks, review and post-review smoke must be repeated after
publication. Final-image cloud qualification remains blocked by the uncertain
provider create: read-only reconciliation at 09:59 found no matching worker, and
no replacement request bypassed that fence. Known task-compute and temporary
registry credential cleanup is verified; protected workers remain untouched.


### Hosted-review correction, 21 September 10:50 UTC

Published candidate `ca230e07a8532b8ed9c3d84be3121e3eceab55cb` received its
current-head hosted review. Linux CI exposed a settings transaction lock that
could remain held by a duplicated/inherited handle. A deterministic regression
reproduced that lifetime defect; dropping the transaction now explicitly unlocks
only after its rollback cleanup. Validation 34 passed all ten lanes for this
lock-only correction, including concurrent setup tests and two native saves.

The bounded review correction also removes unbound worker API authentication
(including interrupted-upload staging files), persists completed source transfer
before session setup, retains both image-contract and cleanup errors, and checks
boot/process-start identity before attributing desktop input. Retained hosted-device
allocations recover through the authenticated host after worker-service loss;
the historical requester cannot regain ownership that may have been transferred.
Subscription login state remains intact. Running agents can retain previously
loaded authentication, so a new session is needed to apply an authentication change.

Independent review is clear for the settings lock, recovery ownership and the
remaining eight-file correction; manifests are retained in private evidence.
Validation 35 caught strict lint errors in a new assertion and two local variable
names. These are corrected. That run also had five browser-CLI deadline failures
while recording and testing concurrently; the speech suite passed. Validation 36
repeats the full final-source matrix with the recorder stopped. No timeout is
silently waived and the original failure logs remain available.

Two hosted observations have explicit scope dispositions. Per-agent root-user
isolation is a separate security architecture; the accepted one-container trust
boundary is documented and private files are not claimed to isolate root agents.
The desktop entrypoint already supervises background desktop processes with
bare `wait -n`. A narrower startup race, where a process exits before synchronous
configuration finishes, remains a follow-up: readiness currently proves the VNC
handshake, not window-manager liveness. Do not represent that as full desktop
application-readiness evidence.

The frozen native UI is private `development-74/horizon`, SHA-256
`28e2f4cb5bd2c631e34892b2cf9d0db80f990a52cf297e00549269c785eaa578`.
Final lint edits leave that executable unchanged; updated device/worker helpers
are frozen separately in `development-75/`. The original Horizon and ephemeral
viewer remain unchanged. Live viewer samples confirm displayed, advancing frames.
An older task window was exposed during candidate replacement; those initial
captures are excluded from acceptance, and subsequent actions use the verified
candidate PID. Current-head hosted checks/review and post-review smoke remain open.

Read-only provider reconciliation at 10:50 found no matching worker for the
uncertain create and confirmed protected workers remain present. No repeat create
or replacement UUID bypasses that fence. Final immutable-image cloud qualification
remains incomplete; local tests and worker Ready timing are not end-to-end proof.
The 10:44 progress email was sent. No merge, release or playground is performed.


Validation 36 passed all required lanes after the final formatting check:
2,369 workspace tests and 2,414 speech tests passed (15 ignored in each), plus
worker/auth regressions, device CLI tests and all lint tiers. The earlier browser
CLI timeouts did not reproduce with the recorder stopped. The final-source
manifest, initial failures and rerun receipts remain distinct. Independent final
documentation/lint review is clear; no further runtime behavior changed.

Two repeated native settings saves passed on the final frozen UI; private bindings
remained byte-identical and the settings timestamp advanced twice. The current
4K capture has 329 continuously captured frames over 90.16 wall-clock seconds.
Representative frames are decoded for inspection; encoding time is not used as a
deployment benchmark. Initial captures with an older task window are excluded.
