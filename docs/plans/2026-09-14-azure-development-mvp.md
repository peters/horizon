# Azure development offload MVP

Status: implementation started; no end-to-end delivery claimed.
Updated: 2026-09-14.

## Outcome

After one-time setup, the user says "Work on Horizon issue #123 in Azure".
An agent prepares a remote issue branch, implements the issue, runs applicable
Linux checks and UI smoke, and returns a reconnectable session and reviewable PR.
The worker continues when the client disconnects. A reconnect observes the
original task rather than starting it again.

## Existing foundations and scope

Reuse the remote-worker image, Azure Linux VM adapter, managed data disk,
managed-identity registry pulls, exact-commit Git preparation, protected runtime
repository credentials, saved Shell tasks, retained SSH identity and lifecycle
APIs. GitHub is the repository handoff; local dirty-tree synchronization is out
of scope. Automatic checkpoints and worker-loss recovery remain post-MVP.

Track current product acceptance in [#474](https://github.com/peters/horizon/issues/474)
and [#475](https://github.com/peters/horizon/issues/475); implementation details
and active ownership there supersede older snapshots. Independent Shell panel UI
is already assigned to another contributor; do not duplicate that work.

## Serial delivery slices

### 1. Reproducible Linux UI testing image — native and both browser proofs complete, review pending

- Extend the existing worker image with Xvfb, a lightweight window manager,
  software rendering and native desktop test tools.
- Provide one bounded smoke command accepting an exact candidate binary and a
  new artifact directory. Start a private display and isolated Horizon config;
  check launch, resize and terminal interaction and retain screenshots and logs.
- Never attach to a user's desktop, config, active process or agent session.
- Prove the helper inside Docker, including failure reporting and cleanup.
- User clarification: UI smoke must actually run inside the Docker worker. A
  host-side launch or screenshot does not satisfy this slice. Build the candidate
  in the worker runtime as well, avoiding host/container libc incompatibility.
- Latest user clarification: the standard Horizon development image must include
  Chromium/ChromeDriver and Firefox ESR/geckodriver to smoke-test `horizon-browser`.
  Keep the smaller native-only image available as an explicit build target.
  Prove installed versions, then qualify browser-panel interaction separately
  through the public Horizon browser MCP tools inside the worker environment.
- Document the normal repository validation matrix and image build commands.
- Software-rendered desktop proof does not establish hardware GPU performance,
  real microphone capture or browser functionality. Browser interaction must use
  Horizon's public browser MCP tools and requires a separately verified lane.

### 2. Small controller entry point — implemented and locally validated, review pending

- Expose a narrow, structured interface over existing core APIs for preflight,
  setup, issue-task start, status and reconnect. Keep Stop/Delete explicit.
- Discover `az` and existing non-secret infrastructure, validate an explicitly
  bound subscription/profile, and report only missing prerequisites.
- Preserve allocation/task identity across interrupted replies; observation must
  never create a replacement worker or replay a task.
- Retain a private result record: issue, base/head SHA, image digest, worker and
  task identity, status, validation artifacts and PR URL.
- Prefer an agent launcher as a saved Shell command for the first iteration;
  introducing a new agent-panel kind is not required.

### 3. Installed offload skill and onboarding — implemented and locally validated, review pending

- Package a `horizon-offload` skill through the existing plugin installation path.
- First use checks tooling and prepares a reusable profile. Ask only for missing
  account/resource boundaries, spending policy and secret delivery/login.
- Keep Azure controller access, image-pull identity, repository PAT and agent
  authentication distinct. No secrets in images, plans, task arguments or logs.
- Subsequent calls accept an issue URL/number and optional acceptance details.
- Resolve a fresh base commit and use an isolated `issue-<number>-<description>`
  branch. Pass repository instructions and issue acceptance criteria remotely.
- Validate authenticated agent execution and reconnect without relying on an
  always-online client. Do not promise compatibility based on installed binaries.

### 4. Image publication and one real Azure issue — pending

- Validate and publish the exact image digest through the approved publication
  path; add reproducible publication automation in its own slice if necessary.
- Retain Cargo caches separately from source to make repeated issues practical.
- Choose a small reproducible Linux issue; record baseline, implement remotely,
  run repository checks and applicable UI smoke, then create a reviewed PR.
- Run the existing disposable-client-off acceptance method: independently observe
  original-task progress while the client is deallocated, then reconnect without
  another create/start. Complete independent-panel and explicit worker Stop/start
  retention gates before calling the full product MVP complete.
- Verify exact-resource cleanup; pushed work is protected by GitHub, unpushed
  work is not protected against worker-disk loss. Stop and Delete are distinct.

## Delivery and validation rules

Each slice is independently testable and uses a separate worktree from fresh
`origin/main`. Stay within the repository's source/test file and changed-line
limits; use focused serial PRs. Before pushing, run the full matrix in AGENTS.md
on the exact final branch, plus applicable image/helper tests and live smoke.
Complete independent local review and the required external review/CI gates.
Specific merge and release/image-publication approvals remain separate; existing
cloud resources and running user sessions must not be changed by a local rehearsal.

## Progress log

- 2026-09-14: inspected source and current Azure issues; verified local Docker and
  Azure CLI account availability without provisioning. Product acceptance remains
  open; recorded adapter readiness of roughly 3–5 minutes excludes build/task setup.
- 2026-09-14: created `feature/linux-worker-ui-smoke` at `f4caa2b4` in
  `/tmp/horizon-linux-worker-ui-smoke`. Primary checkout's unrelated handoffs are
  untouched. Starting slice 1; later slices have not been implemented.
- 2026-09-14: built the optional image in `containers/linux-ui-worker`; installed
  Chromium 152.0.7977.82, matching ChromeDriver, Firefox ESR 140.15.0 and geckodriver
  0.37.1. Native runtime qualification found and fixed the missing
  `libxkbcommon-x11-0`; the host-built binary negative run demonstrated why the
  candidate must be built in the container runtime.
- 2026-09-14: built Horizon from the isolated worktree inside the image. Native
  Docker smoke passed launch, terminal input, resize, input after resize and
  normal close; launch/resized screenshots were inspected after fitting the
  workspace. The `/bin/false` negative control failed as required with complete
  cleanup. Fifteen process/isolation tests pass and are wired into CI. Independent
  review found an orphan-process cleanup edge case; subreaper/pidfd cleanup and
  two regression tests resolve it, and re-review found no remaining action items.
- Current private evidence: `/tmp/horizon-linux-ui-evidence-20260914/run-d/`,
  `chromium-final/run/` and `firefox-final/run/`. Both browser engines passed
  create/navigation/form interaction through the isolated candidate's public MCP;
  embedded-panel screenshots were inspected. The unprivileged browser lanes use
  per-container `seccomp=unconfined` for user namespaces; browser sandboxing remains
  enabled. No host browser MCP connection was changed.
- Candidate binary SHA-256:
  `4399c9e11ee7e88699626d350ef7e07cbc8801d9dc275c76a54b80a45ef878f3`.
  Final local image ID:
  `sha256:1cc9a6b00fc986998cd9e8b55f56f4c2ac59dece8e48d1c86fad70cd84065c99`.
  These identify a local rehearsal, not a published registry release.
- Default highly concurrent workspace tests encountered an unchanged browser
  routine-storage test failure. The exact test passed in isolation. Complete
  default and speech workspace suites passed with `RUST_TEST_THREADS=8`, without
  skipping tests. Blocking Clippy, formatting, maintainability and version sync
  pass; strict and advisory Clippy also passed. The reviewed relative repository
  path correction passed an additional packaged Chromium public-MCP run.
- The final packaged command also refused reuse of the passing artifact directory;
  its original receipt and images remain intact. All task-owned build/smoke/probe
  containers were removed. The local image, isolated worktree, build caches and
  private proof files remain available for continuation.

## Immediate continuation

1. Complete hosted review/CI for the testing image (#647), controller (#649),
   skill (#650), and focused base-image correction (#652). All local validation
   matrices passed, including the required strict Clippy tier.
2. The full-agent base was rebuilt with current Git helpers and its routines
   dependency. Exact public Git preparation, saved-task execution and reconnect
   passed. The user-approved native-only variant was published and its authenticated
   digest pull verified. The repository manifest selects that immutable image;
   browsers remain an optional build target. The older published browser image
   with outdated Git helpers is not qualified for issue offloads.
3. Continue the explicitly approved bounded Azure pilot using its original private
   receipt. The exact native image, agent CLI and smoke helper were observed on
   the running VM, and its compute shutdown schedule was verified. Repository
   credential transfer was explicitly authorized and Git preparation completed. Coding-agent device login remains pending.
   Record live execution, UI proof and exact cleanup in private pilot evidence;
   worker allocation alone does not establish authenticated-agent or client-off
   acceptance. The browser namespace policy remains a separate optional Azure lane.

## Repository manifest decision — user clarification

Use `.horizon/worker.yml` at the selected Git commit. Version 1 has named
`environments`, each with an immutable `image`, repository-relative `directory`
and named argv-based `checks`. An optional `default_environment` names one entry.
Monorepos can choose different images for frontend/backend and run cross-cutting
issues as separate task environments. Do not select the first mapping entry
implicitly. Multi-container service stacks need a separate orchestration contract.

Azure subscription/profile, region, cost authority and credentials remain in
private user settings. Persist the selected environment and resolved values with
the task receipt. Fetch the manifest from GitHub at the exact source commit when
no local checkout exists. No current issue defines this file format; #383/#470/
#474 supply the existing Git/image/provider boundaries it must preserve.

The user selected Codex CLI for the first worker. Authentication must be qualified
independently of the GitHub PAT and registry managed identity.

## Latest pilot findings

- The native-only image passed an actual Azure source build and native UI smoke,
  including terminal input before/after resize, normal close and cleanup. Exact
  pinned-SSH reconnect after explicit Stop and compute-start retained the binary
  and proof. These checks do not establish authenticated issue execution.
- Default Azure Docker restrictions prevent the coding agent's filesystem sandbox
  from starting. A disposable probe qualified SYS_ADMIN plus unconfined container
  seccomp/AppArmor while retaining workspace-write enforcement: workspace writes
  succeeded and writes to `/etc` were denied. Applying this broader container
  policy to the live pilot requires separate explicit approval. No extra host
  mounts or Docker socket are part of the proposed configuration.
- The corrected local native-v5 image includes failure receipts for initialization
  and cleanup errors, and handles termination during binary snapshotting. Native,
  Chromium and Firefox smoke passed; all launch/resized screenshots were inspected.
  Browser lanes used the optional cached browser image with the corrected helper.
  Evidence: `/tmp/horizon-ui-v5-proof`; publication is not yet approved.
- Controller recovery now atomically journals additional panel intent, preserving
  the original panel ID across interrupted saves without replaying execution.
  Fourteen focused tests pass; final complete validation and hosted review remain
  gates. Git/task-start claims conservatively retain uncertain outcomes.
- The pilot remains bounded by its approved four-hour limit. An exact-identity
  cleanup timer is scheduled before that deadline, alongside independent Azure
  compute auto-shutdown. The local cleanup timer depends on this client being up;
  verify provider absence explicitly after deletion.

## Browser default clarification

The user's latest instruction supersedes the earlier native-only default: Horizon
includes `horizon-browser`, so its standard development image must contain both
Chromium and Firefox with their drivers. The Docker default now selects that stage;
`--target native` remains available for explicitly narrower tasks. Rebuild and smoke
the complete packaged default image, then request publication of that exact image.
Update the repository manifest's default digest only after publication is verified.
The existing native-only pilot receipt continues to identify its original image;
changing that live worker is a separate operation, not a silent manifest update.
