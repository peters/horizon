> **Historical document — remote development removed in #693.** The worker binary, provisioning and repository-transfer APIs described below no longer exist. Commands and procedures are retained only as historical design/test evidence and must not be used as current setup instructions. Ordinary SSH terminals and remote browsers remain supported.

# Azure development offload MVP

Status: standard image published and qualified; bounded Azure pilot deleted;
remote-produced source fix locally validated and delivered as PR #658.
Full product acceptance and merges remain separate gates.
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

### 4. Image publication complete; remote issue implemented, full cloud validation incomplete

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
Specific merge and release approvals remain separate. The user has authorized
future image publications to the existing private Azure ACR; do not ask again
within that scope. Public registries and additional paid workers are not covered. Existing
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

## Reviewable delivery slices

- [#652](https://github.com/peters/horizon/pull/652): complete repository build context.
- [#647](https://github.com/peters/horizon/pull/647): native and browser testing image, smoke helper and this plan.
- [#656](https://github.com/peters/horizon/pull/656): usable Rust tooling in clean worker SSH sessions.
- [#657](https://github.com/peters/horizon/pull/657): retained Azure task-status inspection.
- [#649](https://github.com/peters/horizon/pull/649): durable controller and monorepo image manifest, stacked on #657.
- [#650](https://github.com/peters/horizon/pull/650): installed offload skill and one-time setup guidance.
- [#658](https://github.com/peters/horizon/pull/658): the real offloaded font-build diagnostic fix.

These are reviewable source changes; publication of the private testing image does
not merge or release the controller or skill. Merge dependent slices in order only
after their current-head gates pass and the user specifically authorizes the merge.

## Immediate continuation

1. Settle hosted review and CI for the recovered source fix in #658. Its full local
   matrix and isolated native smoke passed; the cloud speech lane remains incomplete.
2. Settle hosted review and CI on each final delivery head, including the focused
   task-status prerequisite and controller stack.
3. Schedule the remaining product acceptance only under applicable authorization;
   the pilot is deleted and no additional paid worker is authorized.

## Repository manifest decision — user clarification

Use `.horizon/worker.yml` at the selected Git commit. Version 1 has named
`environments`, each with an immutable `image`, repository-relative `directory`
and named argv-based `checks`. An optional `default_environment` names one entry.
Monorepos can choose different images for frontend/backend and run cross-cutting
issues as separate task environments. Do not select the first mapping entry
implicitly. Multi-container service stacks need a separate orchestration contract.

Azure subscription/profile, region, cost authority and credentials remain in
private user settings. Persist the selected environment and resolved values in
private handoff evidence alongside the task receipt. Fetch the manifest from GitHub at the exact source commit when
no local checkout exists. No current issue defines this file format; #383/#470/
#474 supply the existing Git/image/provider boundaries it must preserve.

The user selected Codex CLI for the first worker. Authentication must be qualified
independently of the GitHub PAT and registry managed identity.

## Current pilot findings and delivery gates

- The standard image includes Chromium/ChromeDriver and Firefox ESR/geckodriver.
  Published `browser-v7-toolchain-a8556969` resolves to
  `sha256:6c8eaf6304b39b593c04367035a67cbe89f7e3f1bb5529432add43ef4d65060e`.
  Registry manifest and authenticated digest pull were verified. Packaged native,
  Chromium and Firefox smoke passed; launch/resize screenshots were inspected.
  The repository manifest now selects this exact browser-capable digest.
- `--target native` is an explicit smaller build, suitable only when browser
  testing is outside scope. The completed pilot used its original native-v3
  image: publishing or changing a manifest never replaces a live worker silently.
- Native source build and UI smoke passed in the actual Azure worker. Explicit
  Stop and compute-start retained its binary, proof bytes and pinned SSH identity.
  Three independent saved shell tasks completed after controller disconnection.
  This does not prove the three-panel desktop or disposable-client-off acceptance.
- The user approved applying SYS_ADMIN and unconfined container seccomp/AppArmor
  to the exact pilot. It was applied without additional host mounts or privileged
  mode. A live sandbox probe allowed workspace writes and denied `/etc` writes.
  The user completed device login; authenticated issue execution was verified.
  This host-policy qualification must remain explicit for new worker profiles.
- Container replacement cleared the runtime repository PAT while Git remained
  `Complete` and retained agent login survived. Protected stdin restored the token
  to the same explicitly authorized worker; repository API access then passed.
  A standalone controller credential-refresh operation remains missing. The skill
  documents this distinction and forbids deleting claims or replaying Git setup.
- Controller recovery atomically journals panel intent and preserves its original
  panel ID after interruption. Fourteen focused tests and the full matrix passed.
  A disposable local run proved early start refuses without consuming a claim,
  then starts successfully after Git readiness; duplicate starts stay refused.
- The pilot revealed that configured status lacked Azure routing despite supporting
  Azure start/attachment. The focused correction in #657 adds nine regression
  tests; its earlier qualified head passed the full matrix and a live controller
  query returning the original task's
  running state without attachment or replay. Controller #649 is stacked on that
  prerequisite; final lifecycle admission permits only Ready or Reconciling.
- Clean SSH environments lost Cargo/Rustup homes and could not find the installed
  toolchain. Correction #656 restores the homes in login and saved-task shells,
  preserving explicit overrides and credential filtering. The published browser-v7
  correction passed clean-shell real-tool checks and all packaged smoke lanes.
  The repository's stable toolchain can differ from the image's minimum compiler;
  warm caches with the selected repository toolchain before judging task latency.
  The offload skill now performs this preflight for environments with Rust checks
  and pins the resolved toolchain version for the complete saved task.
- The real issue is an early build diagnostic for missing/unhydrated embedded font
  assets, found while dogfooding the worker. The remote agent committed the four-file
  fix. Focused reproduction/recovery checks, default workspace tests, all Clippy
  tiers, independent review and final native smoke passed on the worker. The full
  speech lane did not finish before cleanup; targeted passes are not a substitute.
  A verified source-only patch was recovered to an isolated local worktree at the
  same base. The full local matrix, independent review and isolated native smoke
  passed before opening #658; local launch/resize screenshots were inspected. Logs,
  screenshots, authentication and private task receipts were excluded from remote
  recovery; the local smoke generated separate evidence.
- Exact-resource deletion was submitted at 18:55 UTC and provider/controller
  absence was verified before 19:00 UTC, within the approved four-hour window.
  The pilot VM and its resource group are gone; the existing private registry and
  pull identity were retained. Independent compute auto-shutdown was also configured.
- Agent resume did not inherit the original writable paths, network or approval
  settings. Restoring the original scoped options allowed execution to continue.
  A validation subprocess also failed to survive the agent ending its turn. The
  skill now requires explicit resume scope and either waiting for validation or
  running long checks in a separate saved worker panel with a durable exit receipt.

## Remaining product work

Complete independent-panel UI and disposable-client-off acceptance in #474/#475,
merge the reviewed slices only when specifically authorized, and distribute the
controller and skill through the normal release path. Fresh Azure profiles still
need a qualified, explicitly authorized agent sandbox policy: the pilot required a
manual container-policy correction, so the default bootstrap is not yet proven as
unattended setup. Add controller-supported credential refresh and safe artifact
export, and prove a complete remote validation-to-PR run with the required compiler
and warm caches inside its budget. The source-built controller
is available for experimentation; existing released binaries do not gain it from
an image publication. Monorepo environments select separate images; networked
multi-container service orchestration is a separate contract.

Retained authentication can be reused while its worker storage survives. Pilot
deletion removed its local authentication and worker-only artifacts. The audited
source patch was preserved; a new worker may require fresh login and explicitly
authorized repository credential delivery.
