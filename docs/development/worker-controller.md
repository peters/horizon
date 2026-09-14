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

Create, Git setup and task starts sync create-new claims before dispatch and
retain them after failures. Lifecycle APIs use their existing durable core intent
journal, allowing pre-dispatch failures to be corrected without a stranded claim.
An interrupted reply is not permission to delete the claim or submit again.
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
