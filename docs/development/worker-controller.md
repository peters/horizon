> **Historical document — remote development removed in #693.** The worker binary, provisioning and repository-transfer APIs described below no longer exist. Commands and procedures are retained only as historical design/test evidence and must not be used as current setup instructions. Ordinary SSH terminals and remote browsers remain supported.

# Development worker controller

`cargo build -p horizon-core --bin horizon-worker` builds a Linux-only controller
for one persistent worker/task per private directory. This is an initial source
workflow, not an already distributed release command. It calls Horizon's existing
configured setup, Git, saved-task and attachment APIs; no separate provider or SSH
implementation is introduced.

## Repository convention

A committed `.horizon/worker.yml` describes named environments, including
monorepos with different worker images:

```yaml
version: 1
default_environment: frontend
environments:
  frontend:
    image: registry.example/web-worker@sha256:<64 lowercase hex digits>
    directory: apps/web
    checks:
      build: [npm, run, build]
      test: [npm, test]
  backend:
    image: registry.example/api-worker@sha256:<64 lowercase hex digits>
    directory: services/api
    checks:
      test: [cargo, test]
```

`horizon-worker manifest <file>` validates this bounded, nonexecuting description
and prints JSON. The example's placeholders must be replaced with actual tested
registry digests. There is no implicit `latest` image, provider selection or first
mapping-entry default. The agent selects the requested environment, declared
default or sole entry. Multiple selected environments use independent task
receipts; service dependencies running together require separate orchestration.

Read the file from the exact Git commit, locally or through GitHub. Record the
manifest path and commit, selected environment and resolved values in separate
private handoff evidence. Do not add provenance fields to the create JSON; the
controller receipt stores the resolved execution intent. Subscription,
region, profile, cost authority and credentials stay in user settings. Repository
commands remain subject to task authorization; parsing does not execute them.

## Commands

| Command | Input and outcome |
|---|---|
| `create <new-directory>` | JSON intent on stdin; saves a private session and recovery receipt before provisioning once |
| `check <directory>` | Recovers/observes only the original allocation |
| `git-install <directory>` | First authorized repository PAT on stdin, then detached exact-commit Git setup |
| `credential-install <directory> <operation-id>` | Authorized missing-token installation only; PAT on stdin, fresh non-nil UUID, no Git setup/task start |
| `git-prepare <directory>` | Detached Git setup using existing runtime credentials |
| `git-status <directory>` | Inspects original Git receipt; require `Complete` and null reason |
| `start <directory>` | Starts the complete saved command once after Git readiness |
| `status <directory>` | Observes the saved task without starting it |
| `add-panel <directory>` | Saves additional `{operation_id: UUID, command: {program,args}, directory}` intent; receipt precedes save |
| `snapshot <directory>` | Attaches briefly and returns terminal output; task success requires independent evidence |

The create JSON contains `config` (`RemoteProviderConfig`), `target` (`provider`,
`profile`, digest `image`, `disk_gib`, `lifetime: persistent`, and Azure
`max_hourly_cost_micros`), `repository` (`repository: owner/repo`, exact `commit`,
explicit work `branch`), `working_directory`, `command` (`program`, `args`),
`setup_expires_at_millis` and `issue`. The expiry authorizes initial setup; it is
not an automatic compute shutdown or a total spending cap.

Start with a bounded synthetic command to qualify a provider. For actual issue
work, save the full agent invocation, closed stdin, repository instructions,
acceptance criteria and test/PR requirements in `command`. Keep authentication
separate from arguments. There is deliberately no asynchronous terminal-input
submission: closing an attachment can discard queued input. Reconnect uses the
original saved command and task identity.

Create atomically publishes its identity receipt and dispatch claim together, after
local preflight and before provisioning. There is no separate create-claim write
that can fail after receipt publication. An interruption after publication may
still precede provider dispatch: recovery observes the original identity and never
retries creation. A missing worker is not proof that dispatch was never attempted.
Legacy receipts without the embedded marker remain readable; creation recovery
is observation-only for both formats.
Git setup and task starts sync create-new claims before dispatch and retain them
after failures. Lifecycle APIs use their existing durable core intent
journal, allowing pre-dispatch failures to be corrected without a stranded claim.
An interrupted reply is not permission to delete the claim or submit again.
`credential-install` restores a missing runtime token after restart without changing
Git or task claims. Supply a fresh non-nil UUID for an explicitly authorized
credential disclosure and redirect a protected token file to stdin. The canonical
UUID names a durable `credential-<UUID>.claimed` file; duplicate UUIDs are refused
before stdin is read. Controller-owned stdin and SSH input buffers are bounded
and zeroized on return; the claim stores no token.
Installed/Present results do not establish repository authorization or expiry.
Existing tokens are never replaced. Retain the claim on failure or an uncertain
reply: do not retry automatically or generate a replacement UUID to bypass it.
A new disclosure attempt requires renewed explicit authorization. Verify actual
repository API access separately; rotation of a present expired/revoked token
remains unsupported. No Git preparation, task replay or worker restart follows
this operation.

Only one controller may use a directory at a time. Keep the original directory;
its keys, session and receipts are required for reconnect. Do not share it with a
running desktop Horizon instance or copy it to impersonate another session.

Azure lifecycle uses the same configured APIs through `management-preview`,
`stop`, `stop-check`, `compute-start`, `delete`, `delete-retry` and `delete-check`. A mutation reads
`{workspace, revision, resource_id, action, acknowledge_data_loss}` on stdin, matching the
current preview; Delete requires the data-loss acknowledgement. Observation never
resends a management request. After an uncertain Delete, inspect `delete-check`
and obtain a fresh resource/revision confirmation for `delete-retry`. Local Docker cleanup remains an explicitly guarded
container-ID action outside this CLI. Additional saved panels are selected by an
optional panel ID on `start`, `status` and `snapshot`. Closing the CLI leaves the
worker running. Use an explicit exact-resource cleanup plan for paid tests.
Repository PAT and agent authentication are never supplied by the manifest.
Terminal snapshots can contain private task output, so do not publish them raw.

## Local proof

The controller was rehearsed with the cached complete Shell image on rootless
Docker: private setup, exact Git checkout, saved synthetic task start and
noncreating reconnect. Installed browser tools and cloud client-off acceptance
have their own gates; controller creation alone establishes neither.

Additional-panel journals are published atomically before the local database save.
Retrying the same operation ID and exact command/directory observes its saved panel,
or saves the original journaled panel ID after rechecking ownership, allocation and
current revision. A changed intent or allocation is rejected. Legacy partial receipts
can only observe a matching existing panel; they cannot recreate missing intent.
These retries never start a remote process. Git and start claims remain conservative:
a failure before dispatch can require manual investigation when the controller cannot
prove whether any remote work occurred. Never remove those claims to force a retry.

Status responses use structured JSON: panel `status.state` is `running`, `exited` or
`unavailable`, with numeric `pid` and nullable `exit_status` where applicable. Git
`submission.state` is `submitted`, `observed` or `unknown`; observed submissions include
an `observation` object. Saved phases use their existing tagged serialization or null.

The repository's default image includes Chromium and Firefox with their drivers.
The `browser` check runs both public-MCP smoke lanes. For a private root-owned
worker checkout it stages only the candidate executable and public MCP client in
a new private directory, then drops to `horizon-smoke`; repository and credential
permissions stay private. The large input copy is removed after the run, while
receipts, screenshots and logs remain available. Both UI checks respect an explicit
`CARGO_TARGET_DIR` used for retained build caches. Container namespace policy must
be qualified separately on the selected host.

Task start inspects the worker Git receipt before claiming execution. An absent,
incomplete or degraded checkout is refused with no start claim; after Git becomes
`Complete` with no reason, the original task may be started normally. An already
claimed start remains protected against replay.

## Explicit Azure container runtime

Set `container_runtime: workspace_sandbox_v1` on the authorized private Azure
profile to enable the qualified nested filesystem-sandbox runtime for new workers.
Omitting this field preserves Docker defaults. The repository manifest cannot
select or broaden this host policy, and changing the private profile does not
upgrade an existing worker: retained profile bindings reject policy drift.

This policy requires Docker 29.1.3, AppArmor enforcement and kernel seccomp. Its
pinned filters add user/mount namespace operations while retaining system write
denials and the default capability set. A mandatory Docker pre-start check rejects
unsupported versions, disabled enforcement or policy-file changes after reboot.
Bootstrap runs the image's offline sandbox probe as root and UID 1000 before
creating the SSH worker. Failed probes are removed by their owned identity; no
credential transfer, task replay or permissive fallback occurs.

The image must include `horizon-agent-sandbox-smoke`. Native UI and both browser
engines require their separate smoke lanes. The policy does not establish agent
login, repository authorization, custom agent-policy support or network isolation
for development tasks. See the runtime asset notice for qualification provenance.
