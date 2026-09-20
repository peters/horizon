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
