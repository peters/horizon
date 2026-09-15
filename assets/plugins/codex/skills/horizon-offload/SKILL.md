---
name: horizon-offload
description: Offload repository development issues to a persistent Horizon Linux worker, using repository-defined environments and a configured Azure or local Docker profile. Inspect and reconnect to existing tasks without replay.
---

# Horizon issue offload

Use `horizon-worker` for the persistent allocation, Git handoff, saved task start
and observation. Discover the controller, `gh`, and tooling for the selected provider
before asking the user to install anything. The first agent lane uses Codex CLI. Read the repository's
instructions and issue acceptance criteria before constructing the task.

## Repository environments

Look for `.horizon/worker.yml` at the exact selected Git commit. With no local
checkout, read it through GitHub's contents API using that ref; a local checkout
is not required. Validate a temporary copy with `horizon-worker manifest <file>`.

The version-1 shape is:

```yaml
version: 1
default_environment: frontend
environments:
  frontend:
    image: registry.example/web-worker@sha256:<64 lowercase hex digits>
    directory: apps/web
    checks:
      test: [npm, test]
  backend:
    image: registry.example/api-worker@sha256:<64 lowercase hex digits>
    directory: services/api
    checks:
      test: [cargo, test]
```

Each environment is one worker image. Select an explicitly requested environment,
otherwise the declared default or the sole entry. For ambiguous multi-environment
issues, inspect affected paths and clarify the selection if needed. Cross-cutting
issues can use separate task receipts per environment; this does not orchestrate
networked service containers. An absent manifest calls for a proposed starter
configuration, not an invented registry or mutable `latest` image. Commands are
argv arrays and execute relative to the selected directory only during an
explicit task; reading the manifest never executes them.

Resolve Azure account, profile, placement and spending policy from private user
settings. Repository YAML never grants cloud authority or supplies credentials.
Keep the manifest commit, selected environment and resolved values in private
handoff evidence. The existing provider controller requires explicit selection
and an immutable image. It never guesses an ambient Docker daemon or subscription.

## First use

1. The controller process itself currently runs only on Linux. On macOS or
   Windows, use an already authorized Linux controller host; do not assume the
   installed skill makes the binary runnable on the client platform. Discover the
   controller with `command -v horizon-worker`. During source-based
   MVP development build it with `cargo build -p horizon-core --bin horizon-worker`
   from the reviewed controller checkout. Do not assume released Horizon binaries
   already contain this separately built command.
2. For Azure, discover `az` and resolve an explicitly authorized profile. Set
   `worker_subscription_id` from that profile's `subscription_id`, then inspect
   `az account show --subscription "$worker_subscription_id"` using read-only calls.
   Azure profile fields are `name`, `subscription_id`, `location`, `vm_size`,
   `image_pull_identity_id`, `declared_hourly_cost_micros`, `registry_login_server`
   and `disk_sku`. Reuse configured resources; provider registration, new IAM grants
   and image publication require their own applicable authorization. For
   `local_docker`, use `config.local_docker: [{name, docker_host}]` with an explicit
   authorized Unix socket such as `unix:///run/user/<uid>/docker.sock`; match
   `target.profile` to that name and check that exact daemon. Azure tooling,
   subscriptions and managed identities are not prerequisites for Docker-only work.

   Before every new Azure creation, preflight the controller's subscription-pinned
   ARM token path on the same controller host and with the same CLI configuration:

   ```bash
   az account get-access-token --subscription "$worker_subscription_id" \
     --resource https://management.azure.com/ --query expires_on \
     --output tsv --only-show-errors
   ```

   Require success and a numeric expiry more than five minutes in the future,
   matching the controller's refresh margin. Only the expiry is returned; never
   print or persist the token. Account metadata alone does not prove usable login.
   On failure, repair the login before creating a task directory or claiming creation.
3. Verify the selected image supports the worker SSH/repository contract, the
   chosen agent, and requested build/UI tools. Keep image-pull managed identity,
   repository PAT and coding-agent login separate. A Shell image alone cannot run
   the coding agent. Never bake credentials into an image or copy ambient auth.
4. Obtain the authorized repository-scoped PAT through a protected file or secret
   input. `git-install` reads it from stdin. For Codex, prefer one-time device login
   on the exact worker, with `CODEX_HOME` under private retained worker storage;
   check `codex login status` before another login. Never put tokens in prompts,
   command arguments, receipts or repository files. See the official
   [headless authentication guidance](https://developers.openai.com/codex/auth).

Repository credentials are runtime-only: restarting or replacing the container
clears `/run/horizon/github-token`, even when Git preparation is still `Complete`
and retained agent login survives. Verify repository API access with the installed
protected `gh` helper before issue execution. A missing credential needs protected
stdin delivery to `horizon-github-credential install` on the same authorized worker
through its pinned SSH connection. The controller has no standalone credential
refresh command yet; preserve its Git/start claims and never replay `git-install`
or remove a claim to restore API access. Existing credential-transfer authority
must cover that exact worker; otherwise obtain it before sending the credential.

## One issue, one durable task

The controller accepts JSON on stdin for `create <NEW_PRIVATE_DIRECTORY>`:

- `config`: the existing `RemoteProviderConfig` object (for example `azure: [...]`).
- `target`: `provider`, exact `profile`, digest-pinned `image`, `disk_gib`,
  `lifetime: persistent`, and explicit `max_hourly_cost_micros` for Azure.
- `repository`: GitHub `owner/repo`, exact 40-character `commit`, dedicated
  `issue-<number>-<description>` work `branch`.
- `working_directory`: the selected environment's repository-relative directory.
- `command`: `{program, args}` containing the complete saved task intent.
- `setup_expires_at_millis`: absolute setup admission expiry, **not** an automatic
  compute shutdown or disk billing limit.
- `issue`: issue URL or task label.

Prepare the full request privately. If existing authorization does not cover the
allocation, obtain explicit user approval for its concrete image, profile, `disk_gib`,
cost and lifetime before provisioning billable resources. Reuse authorization already given within
its scope; do not ask again. After approval, run the ARM preflight above for Azure,
then create once. The directory must be new and its
parent must exist. The controller persists coordinates before dispatch.

Run `git-install <directory> < <protected-token-file>` once, then poll
`git-status <directory>` until `Complete` with null reason. An already installed
credential uses `git-prepare` instead. Neither submission acknowledgement nor a
worker being present proves a ready checkout. Start the saved command once with
`start <directory>` after preparation succeeds.

For first login, make the initial saved command an authentication bootstrap:
`/bin/bash -lc 'umask 077; export CODEX_HOME=/workspace/horizon/agent-auth/codex;
mkdir -p "$CODEX_HOME"; codex login status || codex login --device-auth;
codex login status; result=$?; printf "AGENT_LOGIN_EXIT=%s\n" "$result";
sleep 300; exit "$result"'`. Start it after Git readiness and read `snapshot` for
the login URL/code. Ask the user to complete that device login; never complete
account consent on their behalf. Confirm the success marker and agent login
before preparing the issue task. The bootstrap is a separate panel, so its
start cannot consume the issue prompt. Keep its terminal available long enough
for bounded login observation; extend the explicit observation window when needed.

Save the issue task with `add-panel <directory>` using stdin JSON
`{operation_id: <fresh UUID>, command: {program, args}, directory: <relative path>}`.
It returns `panel`; retain that identity. The same operation UUID is never reused
for another intent. An interrupted save has an `add-<UUID>.json` receipt containing
the original panel ID. Inspect it and `check`. Retry `add-panel` with the same
operation UUID and exact command/directory: it observes the saved panel or completes
the interrupted local save with the original panel ID, after checking ownership
and allocation. This never starts a task. Do not generate a second intent or remove
the journal to recover an interrupted save. Legacy receipts can only observe an
existing matching panel.
Then `start <directory> <panel-id>` starts the complete saved task once;
`status` and `snapshot` accept the same optional panel ID.

For environments with Rust checks, inspect the selected commit's toolchain files
and CI configuration before saving the issue task. Qualify `rustc`, Cargo, formatting and Clippy from the actual
worker shell; the image's minimum compiler may differ from the repository's CI
toolchain. Install the required toolchain and components when missing, record its
resolved version, and set `RUSTUP_TOOLCHAIN` to that version for the complete saved
task and its checks. Reuse the retained Cargo cache with that toolchain. Do not change source
just to accommodate warnings from a different compiler.

Put the entire initial agent task in that saved command. Use noninteractive
`codex exec` with stdin closed and the same retained `CODEX_HOME`. Record its
exit status and result file on the worker. Pass the issue, repository instructions,
selected checks and expected PR outcome. Never derive completion from agent prose
alone: inspect the remote Git head, changed files, tests, artifacts and PR through
independent reads. Resolve model/effort and sandbox options against the installed
CLI and existing user preferences. Do not bypass sandboxing just because a worker
is remote.

If the agent process exits before completion, first confirm that it has ended and
inspect any surviving task-owned validation process. Resume the exact recorded
thread in a new saved panel, preserving its working directory, explicit writable
task paths, sandbox and approval policy. Verify those settings against the installed
CLI; do not assume resume inherits them, select an unrelated most-recent thread,
or duplicate an active validation command. Long validation must either finish
before the agent exits or run as its own saved worker panel with a durable exit
receipt; a background command inside an agent tool call is not guaranteed to
survive the agent process. Keep the original resource deadline.

Use `check`, `git-status`, `status`, and `snapshot` on the **same directory** after
an interruption. These never create a replacement or submit the task again.
A snapshot is terminal output, not proof of task success. This MVP controller has
no terminal-input submission command; the saved-command start avoids losing input
when an asynchronous terminal transport closes. An uncertain mutation retains
its claim: investigate the original state rather than deleting the claim.

## UI proof and lifecycle

Build the candidate inside the selected image to match its libc. The testing
layer supplies `horizon-linux-ui-smoke`. The standard Horizon development image
includes Chromium and Firefox plus their drivers, because `horizon-browser` needs
both engines for smoke testing. Run native smoke and the applicable browser lanes
using fresh artifact directories. Use the smaller `native` target only when browser
testing is explicitly outside the selected task scope. Browser automation uses only the
Horizon browser skill and public `browser_*` MCP tools. The Docker lane needs a
qualified unprivileged user-namespace policy. Installed browsers alone do not
prove they can run under an Azure worker's container policy.

Inspect screenshots and verify receipt status, binary SHA, cleanup and test exit
codes before reporting success. Software rendering does not prove hardware GPU
performance. A CLI-only rehearsal does not satisfy the separate three-panel and
client-off product acceptance gates in #474/#475.

Closing a controller detaches; persistent workers keep running and billing.
Use `management-preview <directory>` for the exact Azure resource, saved
revision, profile and loss/billing scope. After applicable explicit authorization,
`stop`, `compute-start`, `delete` or `delete-retry` reads stdin JSON with
`workspace`, `revision`, `resource_id`,
`action` and `acknowledge_data_loss` (true is required for Delete). The selected
resource, revision and action must match; a stale confirmation is refused. `stop-check`
and `delete-check` observe existing intent without resending it. An uncertain
Delete requires a fresh preview and explicit `delete-retry` confirmation. Lifecycle
APIs journal their own intent; pre-dispatch failures do not strand a local claim. This CLI lifecycle
path currently supports Azure only; local Docker rehearsals need an exact
container-ID cleanup procedure. Verify that procedure before allocating. Pushed Git commits protect published work; retained
worker-disk loss can destroy unpushed changes. Report the reconnect directory,
worker/task identity, verified outcome, PR and remaining cost/cleanup state.
