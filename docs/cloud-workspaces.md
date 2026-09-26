# Cloud workspaces

A cloud is one remote development container. Its agent panels use separate Git
branches and worktrees; browser and Device panels share that cloud's runtime.
Only RunPod provisions workers. Daytona and Fly.io appear in labelled design
fixtures.

Cloud deployment and lifecycle control currently require a Unix host with supported
file and directory synchronization. Windows cloud operations fail before state or
provider mutation until durable directory updates are implemented; ordinary local
sessions and preserved cloud metadata remain available. The standalone provider
crate remains portable. Windows cloud durability is tracked in #823; native Device
platform qualification remains separately tracked in #741.

## Companion declarations (preparatory support)

Version 1 configuration accepts optional companion repository metadata:

```yaml
companions:
  app:
    repository: example/application
    profile: cpu
```

Repository identities currently use GitHub `owner/repository` notation. Aliases
start with a lowercase letter and contain lowercase letters, digits, `_` or `-`.
Profiles refer to the companion's configuration, not the declaring repository's
profiles. Local checkout paths and target cloud IDs belong in machine-local state.

This parser and selection contract is a prerequisite for #910. It does not yet
add the companion UI, CLI/MCP discovery, SSH grants, or remote execution. A
declaration never starts a cloud or authorizes access. The selection contract pins
the source session/workspace, source cloud, alias, declaration, and target cloud.
A changed declaration or missing target requires a new explicit selection rather
than rebinding to another matching repository. Multiple matches stay distinct.

## One-time machine setup

In an existing workspace, choose **Cloud** from the panel-creation menu (or
**Cloud > New cloud**), enter a title, and press Enter. Horizon discovers the Git
root from the workspace directory, loads `.horizon/cloud.yml`, and uses its named
default profile. Preparation runs while you type. A configured launch starts
provisioning immediately after submission, without a separate Deploy action.
**Advanced** contains repository, committed revision and profile overrides.
Only committed source is transferred; local changes stay on this computer.

Missing account settings open a repair form without losing the title or target
workspace. Enter the compute API key and choose API-key or subscription login for
the profile's agents. **Save and start** continues the submitted launch. A dedicated
SSH identity is created when needed and keys stay in private machine-local files.
Blank replacement fields preserve saved keys. Subscription login happens through
the actual agent on the worker; worker readiness does not prove authentication.
Open **Cloud > Cloud settings** to change machine defaults without launching.
On narrow windows, Cloud appears in the toolbar overflow.

Cloud allocation requires a saved session so worker identity survives reconnect.
An isolated test instance can use its own disposable saved session and private home.

Install Git (and Git LFS for repositories that use it), OpenSSH, and Docker with BuildKit/buildx. Configure Docker registry
authentication in a private configuration directory using `docker login` with
`--password-stdin`. Use separate repository-scoped push and pull credentials;
give the provider only the pull binding. Build the generic image using
[`examples/cloud-worker`](../examples/cloud-worker/README.md).

For guided private-image setup, rotation, revocation and shared CLI/MCP controls,
see [private registry bindings](private-registry-bindings.md).

For manual setup, create an Ed25519 SSH identity and store the provider API key in a
private file (0600 on Unix). Put bindings in `~/.horizon/cloud/settings.json`; all file paths
are absolute. This file contains paths and references, never literal API keys:

```json
{
  "runpod_key_file": "/private/runpod-api-key",
  "ssh_identity_file": "/private/cloud_ed25519",
  "docker_config": "/private/docker-config",
  "registry_pull_auth_id": "provider-pull-credential-reference",
  "cpu_flavors": ["cpu3c"],
  "gpu_types": ["NVIDIA RTX A6000"],
  "data_centers": []
}
```

`cpu_flavors` lists preferred CPU flavors. RunPod CPU pods take 2, 4, 8, 16 or
32 vCPU. The flavor fixes memory per vCPU (2 GB for `cpu3c`/`cpu5c`, 4 GB for
`cpu3g`/`cpu5g`, 8 GB for `cpu3m`/`cpu5m`) and limits container disk per vCPU
(10 GB for `cpu3*`, 15 GB for `cpu5*`). Horizon sends the preferred flavors that
offer the profile's vCPU count, memory and container disk; when none do, it uses
the flavor with the least memory per vCPU and price that does. For example, an
8 vCPU, 32 GB profile uses `cpu3g` when only `cpu3c` is preferred. A size no
flavor offers fails validation before the image is built.

For rootless Docker, set `docker_host` to its Unix socket URI. Public images can
use a null `registry_pull_auth_id`. Optional `anthropic_api_key_file`,
`anthropic_workspace_id` and `openai_api_key_file` bind explicit API authentication;
otherwise agents use their own interactive login. OpenAI authentication uses the
agent CLI's supported `login --with-api-key` command with SSH stdin; credentials
and login output are never included in command arguments or progress logs.
Agent login state stays on the worker. Workspaces
and profile names do not create separate credential sets.
Reconnect removes unbound API-key files, including interrupted-upload staging
files, while retaining supported subscription login state. Existing agent
processes can retain authentication already loaded into memory or their
environment; start a new session to apply the changed authentication choice.

Optional repository Git authentication uses explicit `git_credentials` bindings.
See the [worker credential setup](../examples/cloud-worker/README.md#optional-git-credentials)
for private file permissions, repository matching, removal and token-scope limits.

## Repository setup and deployment

If the repository has no `.horizon/cloud.yml`, enter its directory in
**Cloud > New cloud**, expand **No cloud configuration yet?**, and open a setup
agent. This is a normal local agent panel using its existing local login; remote
worker API-key bindings are not automatically exported to it. The agent inspects
repository requirements and proposes the selected capabilities before preparing
files. Review its changes and prerequisites, run the repository's checks, and
commit the setup. Return to New cloud and choose **Read .horizon/cloud.yml**.
Changing the repository or encountering invalid YAML clears previously loaded
profiles, so a stale profile cannot be deployed accidentally.

Commit `.horizon/cloud.yml` using the [example](../crates/horizon-cloud/examples/cloud.yml).
Image-only profiles omit `build`. Repository Dockerfiles build from the selected
committed tree, honor `.dockerignore`, and reuse local BuildKit layers. Dirty and
untracked files are excluded. Selected Git LFS objects and recursively pinned
submodule commits must be available locally. They are verified before allocation,
transferred without local Git configuration, and checked out independently for
each agent. Only attributes from the selected commit determine LFS hydration.
Extended LFS pointer formats are rejected explicitly. Source repositories must use
SHA-1 object IDs and UTF-8 paths; unsupported formats fail validation before
compute allocation.

Before each image build, Horizon looks up the release npm currently tags `latest`
for every supported agent CLI. It passes them as the `HORIZON_CODEX_VERSION`,
`HORIZON_CLAUDE_VERSION` and `HORIZON_GROK_VERSION` build arguments, next to
`HORIZON_AGENTS`, `HORIZON_BROWSERS`, `HORIZON_DESKTOP` and `HORIZON_BROWSERSTACK`,
and names the versions in verbose output. All three are passed because an image
can install an agent its profile does not enable. A failed lookup stops the
deployment before the build. In a custom Dockerfile, declare each version argument
directly before that agent's install step and install the exact release, for
example `ARG HORIZON_CLAUDE_VERSION=` followed by
`RUN npm install -g "@anthropic-ai/claude-code@${HORIZON_CLAUDE_VERSION:-latest}"`.
A newer release then rebuilds only that step and the steps after it, so place agent
installs after heavier toolchain steps. An install step without a version stays
cached at the first release it installed. Record the installed versions in
`/etc/horizon-worker/agent-versions.json`, for example `{"claude": "2.1.281"}`.
When that file exists, `horizon-worker-check` requires each enabled agent to have
an entry and to report that version from `--version`; Horizon runs the check
before pushing the image. The [example worker Dockerfile](../examples/cloud-worker/Dockerfile)
follows this pattern. Existing clouds keep the image they were built with; new
clouds receive the latest agents.

Choose **New cloud**, enter its title, repository and base revision, load profiles,
then create and deploy. Horizon validates and uploads the image before allocating
compute. It resolves an immutable digest and checks the worker contract. Keep the
computer online until image/source upload and readiness complete. Expand verbose
output for build and push progress. Failures retain a retryable card and the
persisted operation identity.

Until a worker is requested, including after a failed attempt or a definite
provider rejection, the cloud card offers vCPU and memory drop-downs for CPU
profiles, listing only sizes RunPod offers with the profile's container disk.
The next attempt reuses the built image and applies the new size and the current
`cpu_flavors`, `gpu_types` and `data_centers` settings. Once a worker is
requested the size is fixed: RunPod cannot change an existing pod's vCPU or
memory, so create a new cloud for a different size. After confirmed deletion,
the drop-downs return until the replacement worker is requested.

Add normal panels inside the cloud using the existing panel picker. Choose
Default, Rows, Cols or Grid independently for each cloud. Cloud and workspace
membership is permanent. Removing the last cloud from a workspace also removes the
workspace when no panel remains in it; a new cloud being created there keeps it
until that creation ends. **Close All Panels** keeps its workspace. Full screen
opens one cloud; F11 opens its focused panel, and Escape returns through the
previous views. The shared canvas supports zooming
out to 5%; Fit reserves space for the overview controls and minimap on smaller
windows, and saved views use the same zoom limits as manual zoom.

Cloud workspaces stay in the main window so their frames, runtime controls and
child panels remain together. Use cloud Full screen for a focused view. Move an
ordinary detached workspace back to the main window before creating a cloud in
it. Older saved detached-cloud entries restore in the main window, retaining
their cloud and panel identities.
Workspace layout presets are disabled for workspaces containing clouds. Each
cloud keeps its own layout; ordinary panel resize collision handling does not
move cloud members. Drag the bottom-right corner of a cloud frame to resize it.
With Rows, Cols, or Grid, every visible member grows or shrinks together to fill
the frame, down to the ordinary panel minimum. Default placement keeps each
panel where it is and will not shrink the frame through those panels. A collapsed
cloud has no resize corner. New clouds are placed relative to their workspace and are
reconciled before the overview is fitted.

## Sessions and lifecycle

Each agent has a stable tmux session, branch and worktree. Worktree isolation
prevents concurrent writes to the same files; combining changes is explicit and
can still produce merge conflicts. Closing Horizon detaches presentation while
the worker and tools continue. Reconnect inspects the same worker, restores SSH
tunnels and attaches existing sessions. Reconnect also restores closed terminal
views from their saved remote references.

Before readiness, Horizon verifies the provider's assigned container disk and
persistent volume sizes and the `/workspace` mount path against the profile.
Missing, undersized or differently mounted storage blocks source and agent-credential
transfer. New CPU clouds allocate an owned standard network volume in a data center
that has the cloud's exact CPU size in stock, honoring configured location
preferences, and attach it at worker creation. The provider catalog rates only CPU
flavor families, so Horizon confirms stock for the requested vCPU and memory size
before allocating storage; when no configured data center has it, no volume is
created. New CPU profiles require `storage.volume_gb` between 10 and 4000 GB;
unsupported sizes are rejected before placement lookups or allocation. Saved
allocation journals remain reconcilable and deletable under their original sizes.
GPU clouds and existing deployments retain their Pod-local
storage contract. Unexpected volume identities, locations or capacities block
readiness. CPU mount verification uses the current provider API because the legacy
worker response omits CPU network attachments. Deletion checks current mounts on
all listed workers before removing storage. Network storage remains billable when the worker is stopped or deleted;
explicit cloud deletion removes the worker first, then confirms deletion of the
volume allocated and tracked by this cloud. Separately attached network volumes
are not adopted or deleted: their files and credentials remain, and their storage
charges continue. Cleanup messages preserve that distinction even for older
records whose worker attachment details are no longer available.
Failed cleanup keeps the cloud available for retry and prevents local removal.
Allocation and deletion are journaled so uncertain responses never create a second
volume or adopt an unrelated one. The worker remains allocated and bound to its original operation for
inspection or explicit deletion; retry does not create a replacement. Old saved
worker records without storage fields remain readable. Deployment and reconnect
attempts that reach readiness inspect fresh provider data; this does not change
panel eligibility for a cached Ready record when an earlier preflight fails. This capacity check does not itself prove that files
survive a provider restart; persistence still needs a live recovery test.

Stop ends running processes; storage can remain billable. Idle stop is off unless
a profile opts in: set `idle_stop_minutes` (10 to 1440) so a dedicated worker stops itself after that
long without agent activity, even while this computer is offline. The worker
counts as active while any agent terminal prints output or its container uses
at least half a CPU core, so a quiet build keeps it running. It stops only
itself, using the provider's credential scoped to that worker, and never deletes
anything. Choose **Check provider** on the card afterwards: a worker confirmed
stopped offers Resume like an explicitly stopped one, and nothing resumes it
automatically. Profiles without the field never stop on their own, and workers
shared across workspaces and profiles with hosted devices do not support it. Resume
starts the same worker when provider capacity permits, but lost processes are
reported rather than silently recreated. Delete permanently destroys the worker
and its Pod-local files. Uncertain creation responses are reconciled before any
retry; Horizon never allocates a replacement for a missing worker automatically.
After managed workspace storage cleanup is confirmed, **Redeploy cloud** on the
same card allocates a new worker and managed storage for that cloud. It keeps
the repository, revision, profile, panels and saved sessions, uploads source
again, and reuses the recorded image. vCPU and memory can change before the new
worker is requested. Redeploy is refused while worker termination, hosted-device
release, or managed storage cleanup is unfinished. Separately attached network
volumes stay untouched.

When creation needs confirmation, choose **Check provider** on the cloud card.
This action queries the original operation without building images, preparing
source, transferring agent credentials or changing provider resources. If the
provider supplies a worker ID, enter it under **Provider-confirmed worker ID**;
Horizon verifies its cloud identity and immutable image before adopting it.
Conflicting matches remain blocked. A missing ID, an empty listing, a lookup
authorization failure and elapsed time do not prove that creation failed.
Keep the operation record and ask provider support for an authoritative outcome
when lookup cannot resolve it. There is no force-reset or replacement action.

The [provider create API](https://docs.runpod.io/api-reference/pods/POST/pods)
documents no creation idempotency key or request-status lookup. Its
[billing history](https://docs.runpod.io/api-reference/billing/GET/billing/pods)
identifies worker IDs, not Horizon operation IDs; an absent billing record is
not proof that no worker was created. A definitive rejection received from the
original create request permits another explicit deployment attempt. Lookup
errors cannot provide that permission. A previously bound worker that disappears
is reported as missing. A non-running match retains its verified worker ID but
is reported as inactive: a desired termination status is not proof that deletion
has finished. Check again or explicitly delete that same worker. Only the
explicit deletion flow confirms cleanup. The terminated worker identity remains
until that cleanup is confirmed; **Redeploy cloud** is the later explicit action
that starts a new allocation.

After a worker-service crash, a private journal that durably confirms a remote
device was released can be cleaned up without the old provider credentials.
Unreleased, malformed or mismatched identities remain blocked until their exact
release can be verified.
Recovery after worker-service loss is host-only: the journal records the original
requester, which may no longer own a transferred allocation. Agents cannot reclaim
that historical ownership. Use Horizon's explicit remote-device release action
to reconcile the exact retained allocation through the authenticated host.
If saving confirmed release fails, the allocation keeps its exact identity and
blocks cleanup. Reconcile after restoring writable storage to retry that save;
the live process retains the provider's release result and need not query it again.

Browser panels display their actual controller. Native VNC Device panels are
read-only viewers; worker-local device tools perform input and report its agent.
Choose **Add desktop viewer** on a ready cloud to view its worker desktop.
All agent tools run on the worker, independently of the laptop connection.
The worker supports the public browser tools and standalone device input tools.
`device_panel` manages viewers in a running Horizon window, so it is unavailable
inside the headless worker and returns `viewer_requires_horizon` immediately.
It does not substitute a browser viewer or depend on the laptop for device input.

The development example `cargo run -p horizon-core --example cloud_deploy -- ...`
uses the same coordinator. Run it without arguments for its command synopsis.
It supports image preparation without allocation, deployment, stop, resume,
deletion, `rebuild SETTINGS STATE_ROOT PROFILE` with `continue-rebuild` and
`cancel-rebuild`, and `reconcile SETTINGS STATE_ROOT [WORKER_ID]`. Reconciliation prints a
structured outcome without worker environment or credentials and an explanation;
the UI and this harness use the same locked coordinator and provider policy.
An unresolved outcome is a successful check, not permission to deploy again.
`deploy` on a cloud whose worker and managed storage are confirmed deleted
starts the same explicit redeploy as the card; it does not replace a missing or
unresolved worker.
Cloud provisioning/reconciliation has no public MCP operation yet; the worker's
browser/device MCP tools do not allocate or reconcile compute. This example is
an integration harness, not an installed user command.

### Rebuilding a cloud's image

A ready dedicated cloud can rebuild its worker image and restart on it, for
example to pick up newer agent CLIs or a committed change to its `.horizon`
recipe. Horizon resolves the repository's latest committed `HEAD` (uncommitted
changes are not used) and reads `.horizon/cloud.yml` there. It refuses when the
cloud's profile is missing there or differs from the one the cloud was created
with, naming the change: a running worker's size, capabilities, image repository
and build section are fixed. A CPU cloud's vCPU and memory are chosen when the
cloud is created, so different committed values for them are not a change. A
profile without a build section has no recipe to rebuild. Otherwise Horizon builds the recipe with the newest agent CLIs under a
new tag, validates the worker contract, pushes the image and verifies the
worker's pull binding. An unchanged image digest is reported, and nothing restarts.

Horizon then releases any hosted devices and switches the existing worker to the
new image through the provider's pod update. The worker ID and `/workspace`
(worktrees, agent logins, SSH host keys) survive; the container disk is reset and
running processes end. After readiness, Horizon relaunches each session's process
in its existing worktree without resetting it. Images without the relaunch
contract report those sessions lost, as Stop and Resume do.

The replacement is journaled in the deployment record before each provider
mutation, and older Horizon versions cannot read the record while the update may
be in flight. After an interruption, reconnect and **Check provider** only read
the provider: a worker already on the new image completes the switch, and anything
else stays blocked as a pending replacement. Continuing it sends the update again
only while the worker still reports its previous image. Cancelling it drops an
unsent update or switches the worker back and relaunches its sessions. Deletion
stays available throughout.

On the runtime card of a Ready cloud whose profile has a build section,
**Rebuild image & restart…** asks for confirmation and names these consequences
first. While it runs, the card lists the rebuild's steps with their durations and
offers **Cancel rebuild** only until the image switch is requested. A rebuild
cancelled while its committed recipe is read leaves nothing pending, and the card
offers **Reconnect cloud**; cancelled later, it stays pending. A pending replacement shows a notice with **Continue
rebuild** and **Cancel rebuild**, and Stop waits until it is resolved. Cancelling
a switch that may have been sent asks for confirmation, because switching back
restarts the worker again, and Horizon does not reconnect such a cloud on its own
at startup. The card keeps the outcome, such as an unchanged image or a session
that could not be relaunched, until the next deployment.

### Deployment progress

The cloud runtime card shows the active stage and elapsed time for completed
stages. Image push/download and source upload expose measured progress, bytes per
second and an approximate remaining time when a total and a stable rate exist.
BuildKit reports completed/discovered build steps. Provisioning and readiness show
current activity and elapsed time because the provider supplies no reliable ETA.
Ready shows the measured time to worker readiness for a successfully timed
deployment. It does not claim application-visible startup time or invent a time
for older records without measurements.
Cached image layers are excluded from transfer speed. Expand verbose output for
command details. Per-stage timing resets on retry. The recorded worker-readiness
duration survives reconnect.

## Planned shared workers

Explicit sharing of a compatible CPU worker across trusted projects is tracked in
[#805](https://github.com/peters/horizon/issues/805). The
[shared-worker contract](architecture/shared-cloud-workers.md) defines the proposed
identity, migration and lifecycle design. This is not yet a supported placement
choice; existing clouds continue to use dedicated workers.

### Companion access in cloud panels

In a saved session, each cloud panel lists the companions declared in its
committed `.horizon/cloud.yml`. Check a repository to allow access to an existing
cloud with that repository and profile in the same workspace. When more than
one cloud matches, choose its stable cloud ID first. A missing cloud cannot be
selected. The selection does not create, start, resume, or keep a worker running.

Running workers establish a dedicated SSH alias and a separate target worktree.
Ready means source-to-target SSH was verified. Stopped and unreachable states
remain visible; checking a stopped cloud leaves it stopped. Refresh retries
access through the existing SSH endpoints without contacting provider lifecycle
APIs. No overlay network is needed when those endpoints are reachable.

The source worker must be able to reach the target's published SSH endpoint.
Some providers refuse this connection when workers share a public address;
Horizon reports Unreachable and does not add a relay or start another worker.

Workers with enabled agents automatically advertise `horizon-cloud-companions`.
The read-only `cloud_companions_list` and `cloud_companion_inspect` tools expose
the same catalog as `horizon-cloud-worker companions list` and `inspect <alias>`.
Use the returned SSH alias and worktree with ordinary SSH, Git, and rsync. A
stale catalog loses Ready status; inspection can verify an unchanged connection
independently. M0 has no agent tool for starting or provisioning a cloud.

Uncheck to remove access. If either worker is offline, removal stays pending
until that original worker can confirm cleanup. Dirty worktrees are preserved;
existing shells and copied data cannot be recalled. Clear old selections before
retiring their source cloud. Changing the workspace, declaration, target worker,
or initial revision requires cleanup and explicit selection again. These clouds
share trusted shell access; this is not credential isolation. Both workers need
an image containing the updated companion helper and rsync.

When closing, Horizon waits for earlier companion jobs and durably saves queued
unchecks locally. This save does not require provider settings or reachable
workers; remote cleanup resumes when the session is opened again. If the local
journal cannot be saved, shutdown displays the error and retries instead of
discarding the uncheck. The fallback exit path also waits for that save, so a
persistent disk or journal-lock failure can keep the process alive until repaired.
