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
memory, so create a new cloud for a different size.

Add normal panels inside the cloud using the existing panel picker. Choose
Default, Rows, Cols or Grid independently for each cloud. Cloud and workspace
membership is permanent. Removing the last cloud from a workspace also removes the
workspace when no panel remains in it and no new cloud is being created there;
**Close All Panels** keeps its workspace. Full screen opens one cloud; F11 opens
its focused panel, and Escape returns through the previous views. The shared canvas supports zooming
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

Stop is explicit and ends running processes; storage can remain billable. Resume
starts the same worker when provider capacity permits, but lost processes are
reported rather than silently recreated. Delete permanently destroys the worker
and its Pod-local files. Uncertain creation responses are reconciled before any
retry; Horizon never allocates a replacement for a missing worker automatically.

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
explicit deletion flow confirms cleanup; termination retains its identity permanently.

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
deletion and `reconcile SETTINGS STATE_ROOT [WORKER_ID]`. Reconciliation prints a
structured outcome without worker environment or credentials and an explanation;
the UI and this harness use the same locked coordinator and provider policy.
An unresolved outcome is a successful check, not permission to deploy again.
Cloud provisioning/reconciliation has no public MCP operation yet; the worker's
browser/device MCP tools do not allocate or reconcile compute. This example is
an integration harness, not an installed user command.

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
