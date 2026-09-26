# Worker image contract v1

Copy these files into a dedicated build context. Put the matching built
`horizon-browser`, `horizon-cloud-worker` and `horizon-device` (`--features cli`) executables under `bin/`.
Build with Docker/BuildKit for linux/amd64. The image contains no repository files
or credentials. Agent login uses each actual CLI and stores its supported state
under `/workspace/home`, on the worker volume.

The image-only profile is checked locally by invoking `horizon-worker-check`
with networking disabled before any worker allocation. Repository Dockerfiles
must provide this same contract. Use `.dockerignore` to exclude unrelated files.
The deployment pipeline exports the selected committed revision for repository
builds, so uncommitted files are never silently included.

Worker contract v1 has an additive capability contract. An omitted `capabilities`
section in existing cloud.yml and saved deployments retains all three original
agents, Chromium and desktop. An explicit section enables only its entries:

```yaml
capabilities:
  agents: [codex, claude]
  browsers: []
  desktop: true
```

`capabilities: {}` is a shell-only worker. The supported agent names are `codex`,
`claude` and `grok`; browser names are `chromium` and `firefox`. Firefox includes
its native executable and geckodriver. A new browser panel without an engine
preference uses the first enabled engine in the centralized profile order; an
explicitly disabled engine fails, and an existing session never switches engine
on reconnect. Inside an explicit capability section, omitted lists are empty and
omitted desktop support is disabled.

The base always includes SSH, Git/LFS, tmux, the source/worktree helpers and worker
control service. When the worker is created with `HORIZON_IDLE_STOP_MINUTES`, the
supervisor also owns `horizon-worker-idle`, which stops this worker through the
provider after that period without agent terminal output and without the container
averaging at least half a CPU core (cgroup v2 `cpu.stat`, or `cpuacct.usage` on cgroup v1
hosts); lighter background work does not keep it running.
`horizon-worker-check` reports `horizon-idle-stop-contract=1` for images that support
this, and Horizon refuses to deploy a profile with `idle_stop_minutes` to other images. Device control/ownership works without a browser executable or
browser MCP registration. Agent configuration contains only enabled tool servers.
Disabled agent requests are rejected before writing session or worktree state.

Horizon supplies `HORIZON_AGENTS`, `HORIZON_BROWSERS` and `HORIZON_DESKTOP` Docker
build arguments from the profile, and the latest release of each agent CLI as
`HORIZON_CODEX_VERSION`, `HORIZON_CLAUDE_VERSION` and `HORIZON_GROK_VERSION`. The
standalone Dockerfile defaults to a minimal image; pass these arguments explicitly
when building outside Horizon. An empty version installs the current latest release.
The image records available features in `/etc/horizon-worker/capabilities.json`
and installed agent versions in `/etc/horizon-worker/agent-versions.json`. Before any
allocation, the image checker verifies the requested subset and actual executable
runtimes, with networking disabled. Requested features are supplied through the
nonsecret `HORIZON_WORKER_CAPABILITIES` environment binding, preserving legacy
checker arguments. Capability-aware images still validate the entire requested
selection, including the full default selection, and run their readiness checks.
Managed Claude sessions disable background CLI updates so their runtime continues
to match the image's recorded version when reconnecting. Upgrade the runtime by
rebuilding the image. Manual runtime changes can still fail the version check.
Old images can satisfy the legacy selection;
explicit reduced/expanded selections need the capability-aware worker bootstrap.
The provider receives the nonsecret selected features, and worker startup writes
`/workspace/capabilities.json` before starting services or configuring tools.

Build clean images from `BASE_IMAGE` (Ubuntu 24.04 by default). For a GPU workload,
use the project's compatible CUDA/TensorRT runtime as that base, retaining its
required libraries and GPU environment. Never derive a minimal image from an old
full worker: deleting files in later layers does not remove inherited bytes.
CPU/GPU selection remains a hard profile requirement; there is no CPU fallback.

Prepare a new context with `python3 examples/cloud-worker/prepare-context.py
--bin-dir target/release --output <new-directory>` after building the matching
release helpers. It copies only worker scripts and helpers, strips the final
copies before Docker sees them, and records sizes/checksums. No source, settings,
credentials or previous image directory is copied. Keep each agent install in a
separate layer so the compressed manifest records its actual contribution.

For a GPU context, add `--gpu-base <upstream-runtime>@sha256:<digest>` to that
preparation command. Choose the workload's compatible CUDA/TensorRT runtime,
not a previous worker image. Preparation generates a standalone `Dockerfile.gpu`
from the shared recipe with that immutable base and NVIDIA runtime environment.
Use the generated file in the GPU profile; it accepts the same `HORIZON_*`
arguments Horizon already supplies and needs no additional build argument.
Without `--gpu-base`, preparation produces only the CPU recipe. Unpinned or
credential-bearing bases fail before any context is written. The base digest
must actually contain the required GPU libraries; its name alone proves nothing.

The contract reports `horizon-worker-contract=1`, `horizon-source-contract=1` and
`horizon-capabilities-contract=1`, plus the optional `horizon-session-restart-contract=1`
([session relaunch](#session-relaunch-after-a-container-reset)). Source transfer carries verified LFS objects and
selected submodule history separately from images. A persisted session launch
fence prevents replaying a process whose launch or survival is uncertain.

SSH accepts only the caller-supplied public key. The client uses a dedicated
known-host file per provider worker: first connection uses OpenSSH trust on first
use; later host-key changes fail. VNC listens on worker loopback and is read-only;
SSH tunnels are required for presentation. Disconnecting the client detaches
tmux. Deleting a worker loses its processes and Pod-local volume.

The root-based Pod image uses Chromium with `--no-sandbox` inside the dedicated
one-cloud container. Treat that container as the trust boundary: do not reuse
it across unrelated repositories/accounts. The browser service and VNC endpoint
listen only on worker loopback; presentation uses authenticated SSH. Browser
and device MCP processes run on the worker and retain their injected agent identity.
Private credential files protect against accidental inclusion in source, images
and logs; they do not isolate agents from other root processes in the same cloud.
Per-agent operating-system isolation requires a separate security architecture.

## Session relaunch after a container reset

A container reset, such as an image replacement, keeps `/workspace` but ends tmux
and every session process; attaching then reports the session lost. An image whose
checker reports `horizon-session-restart-contract=1` can start the process again:

```
horizon-worker-session --relaunch OPERATION SESSION AGENT REVISION
```

Relaunch needs ready worker services and the session's persisted agent and revision
binding. It runs the original launch command, `horizon-worker-run SESSION AGENT`, in
the existing `/workspace/agents/SESSION` worktree. It never creates, resets or checks
out that worktree and never imports source again, so commits, uncommitted files and
agent logins under `/workspace` survive. The process builds its environment from the
worker volume and the new container's services, so Horizon sends no credentials for
it. A session whose process had already exited starts again; its stale `exit-status`
is removed.

Before starting the process, relaunch persists `relaunch-requested-OPERATION` in the
session directory, so each operation starts at most one process per session:

| Exit | Meaning |
|------|---------|
| 0 | Relaunched, or this operation's relaunched process is still running |
| 3 | Unknown session, binding mismatch or missing worktree; nothing started |
| 4 | This operation's relaunch was lost, or its launch failed or was interrupted after the fence was persisted; either way it is not replayed |
| 5 | Nothing to relaunch: a process is running or was never launched; attach normally |

Any other failure, such as services that are not ready, starts nothing. A launch
that fails after the fence is persisted is not replayed. A later operation with a new
identifier can relaunch the session again. Images without the marker keep reporting
such sessions lost.

## Optional Git credentials

Git transfer is opt-in in machine-local `~/.horizon/cloud/settings.json`, never
repository YAML. Add a `git_credentials` entry with `local_repository` (absolute
checkout path), `repository` (`owner/name` on github.com), `token_file` (absolute
private 0600 file), `author_name` and `author_email`. Only the matching local
repository receives that binding. Duplicate matching bindings fail before any
allocation. Without a binding, the normal agent login flow remains available.

The image must pass `horizon-worker-check --git-auth` before allocation. Transfer
uses SSH stdin with output suppressed. The worker stores the token separately
from source, images and session state in `/run/horizon-credentials/github.json`
(0600). Git's HTTPS helper matches the exact repository path; `gh` reads the same
binding into only its child environment. The bare repository receives a clean
HTTPS origin without embedded credentials, so each agent worktree can push its
own branch and explicitly create a PR while the laptop is disconnected.

Use an expiring fine-grained token restricted on GitHub to the intended repository,
with Contents and Pull requests write permissions. The local binding and Git
helper are routing guards, **not** a restriction on a broad token's GitHub API
permissions. All agents in one cloud share the container trust boundary.
Removing the binding and reconnecting removes its worker copy; deleting the
worker also removes the Pod-local copy. Neither action revokes the original
account token. Rotate/revoke that token separately when required. Existing `gh`
commands pick up an updated binding on their next invocation.
The protected runtime directory avoids provider volumes that do not preserve
POSIX permissions. Installation checks effective permissions before writing a
token. A worker stop/recreation can lose this runtime copy; reconnect transfers
the still-enabled local binding again before reporting Ready.

Run `python3 -m unittest discover -s examples/cloud-worker -p 'test_*.py'` for
synthetic credential matching, private storage, removal and Git/gh integration
checks. No test credentials are included in this image.

## Repeatable capability image smoke

Run `python3 examples/cloud-worker/smoke-image.py IMAGE@sha256:DIGEST
--capabilities '{"agents":["codex","claude"],"desktop":true}'
--expect-absent horizon-browser --expect-absent google-chrome-stable
--expect-absent firefox --expect-absent grok` against a native image. The script
checks the real offline contract, selected service startup, managed MCP servers,
disabled agents and runtime drift, then removes only its own temporary container.
Use `--capabilities '{}'` for minimal and both agent/browser lists plus desktop
for full. This is a local service integration check, not a live UI or cloud pass.

Worker readiness checks the selected active capability set, the control service,
and the supervisor's recorded process identities. Profiles with desktop also
require a responding X display, the owned Openbox window manager's live root
registration, and a responding VNC server. A listening VNC socket alone cannot
make a worker ready. The supervisor watches desktop services throughout tool
configuration, then starts and watches the control and SSH services. Once a
required service has started, its exit fails the worker and stops its owned
process groups without replacing them. Stale readiness records
are rejected after a restart or PID reuse. The `x11-utils` desktop dependency
provides the bounded X display and window-manager probes. Display startup has a
30-second deadline and tool configuration has a 120-second deadline; failure
invalidates readiness and ends the worker. Application readiness
and the first displayed frame are separate measurements. Record first and cached
build/push durations with cache conditions, compressed manifest layer sizes, exact
digests, fresh worker Ready, app readiness/first frame and same-worker reconnect.
Do not label worker Ready as end-to-end application startup.

### Agent execution on restricted container hosts

CLI presence and device/browser service readiness do not prove that an agent can
run shell commands. Some provider containers deny the user namespaces needed by
the default Linux agent sandbox. The native acceptance test observed this on a
real worker: device MCP worked, while the first sandboxed shell command failed.
Keep the agent's own approval flow and verify an actual worktree edit and test.
Do not silently disable agent security settings or change host namespace policy.

For trusted disposable workers where the container is the chosen isolation
boundary, OpenAI documents an explicit `--sandbox danger-full-access` launch mode;
approval policy remains a separate choice. See the [official container and
approval guidance](https://learn.chatgpt.com/docs/agent-approvals-security). Normal
interactive sessions retain their supported permission controls. An approved
long-running command continues remotely after disconnect; a command waiting for
interactive approval remains waiting. Record the chosen mode in acceptance
evidence instead of claiming unattended shell execution from a version check.

### Optional remote mobile browsers

A profile can enable hosted device testing without installing a local browser:

```yaml
capabilities:
  agents: [codex, claude]
  browsers: []
  desktop: false
  browserstack:
    provider: browserstack
    local_ports: [8080]
```

The provider names an existing Horizon `browser.remote.providers` account.
Agents may select **any device, OS and browser combination in the live provider
catalog**, without preconfiguring a Horizon target. `browser_provider_devices`
accepts the account name, search words and a pagination offset; pass a returned
`target` to `browser_create`. The CLI plan runner exposes the same public tool.
Device selection is agent-driven; this MVP has no device picker UI. Discovery never reserves capacity. Account entitlement and
live availability are authoritative at allocation; provider evidence still verifies
the actual device before a panel becomes ready.

Existing `browser.remote.targets` remain supported. Optional YAML `targets`
entries choose preferred starting targets only, never restrict allocation.
One profile selects one account; different profiles can use separate bindings.
Catalog references expire from the host cache after five minutes; rediscover
before a new allocation when asked. This does not stop existing sessions.

Authorize the exact local checkout once in the cloud `settings.json`:

```json
{
  "browserstack_credentials": [{
    "local_repository": "/absolute/path/to/checkout",
    "configuration_file": "/absolute/path/to/horizon/config.yaml",
    "providers": ["browserstack"]
  }]
}
```

This is an additional field in the existing settings object. It contains no
credential values. Existing OS-keychain and explicitly named environment
bindings are resolved at deployment. Session-only settings credentials must be
bound to one of those persistent sources before deployment. The UI, Rust
orchestrator and other deployment callers use the same grant and resolver.

If `browserstack` is absent, Horizon neither prompts nor transfers credentials.
A declaration is a requirement, not authority to export secrets: absent grants,
ungranted accounts, missing starting targets and missing credentials fail before compute allocation.
Existing version-1 profiles retain their previous defaults; remote device
support is never added implicitly.

Build with `HORIZON_BROWSERSTACK=true` (the deployment flow supplies this from
capabilities). This adds the stripped browser tool helper and the official Local
8.9 tunnel binary; it does not install Chrome, Firefox or a desktop. The pinned
archive checksum fails the build if the upstream download changes. Update the
checksum only after verifying the replacement archive from the official source.

Credentials travel through SSH stdin into mode-0600 files under a mode-0700
`/run/horizon-credentials` directory, outside images, source, build contexts and
persistent provider volumes. Selected agents use the existing public browser
MCP tools, including `browser_create(target=...)`, provider usage and allocation
recovery. They never receive credentials in tool responses. Agents select remote
devices through CLI/MCP; restored panels retain their target and show the
provider-confirmed device identity. BrowserStack Local runs in its own worker
tmux session. Only the listed loopback ports are tunneled, with a cloud-specific
identifier, so the laptop can disconnect. Other public website resources can
still load directly at the device.

Use **Release devices and remove remote credentials** to close hosted devices,
stop this cloud's private tunnel and remove its credential copies. Removing a
local grant alone blocks future transfers; it cannot remotely revoke a secret
already copied. Reconnect reinstalls credentials only when the grant remains.
Stop/Delete first verifies hosted-device release. Uncertain release keeps its
recovery record and blocks destructive cleanup; use `browser_remote_allocations`
to inspect/reconcile the exact allocation. A lost worker service reports retained
allocations as requiring provider verification; it does not claim recovery or
silently retry allocation. An externally destroyed worker cannot revoke the
account key; rotate it at the provider when necessary.

Local tunnel flags and isolation rules follow the provider documentation:
[Local binary options](https://www.browserstack.com/docs/local-testing/binary-params)
and [multiple connections](https://www.browserstack.com/docs/automate/selenium/manage-multiple-connections).

The source-import contract requires Python's `hashlib.file_digest` and safe
`tarfile.data_filter` extraction APIs. Python 3.12+ provides both; older
distribution images must supply compatible APIs before use. The worker check
rejects missing APIs during local image validation, before allocating compute.
A pinned GPU base does not by itself satisfy this runtime contract.

### Experimental allocation bootstrap recovery

`horizon-cloud-worker recover-allocation` is a Linux recovery-only command for
an existing pre-admission bootstrap at `/workspace/.horizon-allocation`. It accepts
one bounded JSON object on stdin, with `message` and `payload` strings preserving
the exact signed request bytes, and returns a versioned recovery receipt on stdout.
The signed bootstrap payload must be `{"action":"recover","token":"<saved-token>"}`.
Use the same persisted operation and exact payload for every uncertain retry.

The command requires an existing private directory and allocation lock, a pinned
bootstrap record, matching immutable `HORIZON_WORKER_STARTUP`, the original
`HORIZON_CLOUD_OPERATION`, matching provider runtime pod/volume/data-center IDs,
and an actual `/workspace` mount. None of these alone proves storage freshness.
It can complete only a matching initializing record and revision-zero empty
manifest; initialized state requires that manifest to remain intact. It rejects
missing, corrupt, conflicting or admitted membership instead of resetting it.
There is no path override, initialize action, controller enrollment or fresh-store
flag. Error output does not include request data.

Allocation startup metadata selects an experimental SSH-only entrypoint before
legacy workspace writes. `prepare-allocation-ssh` captures the independent runtime
identities at a fixed private `/run/sshd/horizon-allocation/runtime.json` path and
prepares the exact Ed25519 key. The v2 commands never fall back to SSH login
environment variables if that capture is absent. Experimental v1 records remain
supported only by direct `recover-allocation` with the original runtime environment.
They had no production initialization or entrypoint integration and lack retained
host-key provenance: v1 cold start through `prepare-allocation-ssh` and automatic
image upgrade are unsupported and rejected before workspace or key publication.
They never fall back to the legacy startup path, which would create workspace
directories and start services outside the pre-admission fence.

The host library's `bootstrap_initialization::create` is the first-initialization
producer. It requires a CPU profile, immutable image, explicit sharing mode,
private account/SSH files and a new dedicated pin path. It performs verified direct
provider creation and first attachment while holding the Owner lock. Persisted
creation receipts, later inspection and arbitrary JSON cannot recreate its live
initialization permission. SSH enrollment retries within the live attempt while
retaining its first accepted key. Before sending signed `initialize-allocation`,
the host anchors the exact future Recover and consumes permission as Requested.
The worker saves the exact running key before its initializing marker; only the
subsequent Recover publishes the empty membership manifest. Missing initialized
keys, markers or membership are never regenerated.

`resume` uses only the existing Recover record. `cleanup` is explicit: after
Requested it anchors a signed `abandon-bootstrap` operation, verifies the terminal
worker receipt and anchors deletion intent before deleting exact owned resources.
Abandoned state rejects Initialize and Recover permanently, but SSH-only startup
allows the same cleanup receipt to be retrieved after a restart. Missing or
unreachable worker state cannot grant new deletion permission. Before Requested,
cleanup uses anchored proof that initialization was never sent. No error triggers
automatic cleanup, replacement or allocation.

`bootstrap_initialization::inspect` verifies a completed allocation against the
prospective project's immutable image digest, retained account, SSH identity and
host key. It makes read-only provider observations and sends a signed
`inspect-allocation` request. The Linux worker requires its version-2 initialized,
empty manifest and retains the allocation lock while running the image's existing
`horizon-worker-check --capabilities-json` for exactly the requested capabilities.
The checker uses a private temporary HOME and cleared environment; it neither
starts services nor writes `/workspace/capabilities.json`. Its process group is
bounded to 60 seconds and captured output to 64 KiB. Host transport is cancellable
and uses one deadline capped at 180 seconds. Missing tools, unavailable selected
capabilities, changed state, stale revisions or foreign signatures fail closed.
The response binds the startup identity, operation, fingerprint and revision-zero
capability observation. It is not admission authority: future admission must
recheck compatibility under its own lock. This pre-admission command rejects
nonempty manifests and does not yet implement resource-capacity or grant checks.

These library APIs do not activate shared projects or advertise full shared-worker
support. Shared membership, source/session/tool isolation and UI/CLI/MCP routing
remain unfinished under #805. Provider storage qualification and spending approval
are prerequisites for live allocation; local synthetic tests do not qualify a
provider image or power-loss durability. The existing entrypoint without allocation
startup metadata preserves its dedicated-worker behavior.

The host library's `cloud_runtime::bootstrap_recovery::recover` holds an `Owner`
through request anchoring, SSH transport and receipt anchoring. It requires the
expected startup/worker identity, the existing SSH identity and a previously
recorded plain `horizon-cloud-<worker-id>` host-key entry. It refuses absent or
changed pins and never accepts a new host key during recovery. The dedicated pin
file may contain only that exact alias, blank lines and comments; wildcard,
hashed, multi-host, authority, revocation and unrelated entries are rejected.
The private snapshot and anchored pin hash use normalized exact-alias entries. OpenSSH's
`ssh-keygen` validates the key encoding and signs and verifies a fixed private
probe before the request is anchored; both `ssh` and `ssh-keygen` with
`-Y sign`/`-Y verify` support must be available on the host. Recovery requires an
unencrypted private identity and validates its captured bytes before anchoring;
encrypted keys and agent-only identities are unsupported by this API. Transport uses
private snapshots of the verified key/pin bytes, so changing their source files
cannot change an in-flight connection. Requests and replies
are limited to 64 KiB; transport has a maximum 60-second deadline and supports
cancellation. The exact signed request survives lost replies and host restarts.
A completed local receipt still requires a matching remote response on every retry.
Calling interfaces must validate allocation ownership before entering this API.

For a repeatable Linux host/worker integration smoke, build the candidate worker
and run `scripts/cloud-recovery-smoke.py --worker /absolute/horizon-cloud-worker
--evidence /absolute/new/private-directory` from the same checkout. Use its
isolated Cargo target and an isolated Python environment with Paramiko installed;
the fixture also requires OpenSSH and bubblewrap. It starts a task-owned loopback
SSH server, seeds synthetic test-only pinned state, and exercises the actual
worker executable in a mount namespace. It checks a lost reply, reopening the
anchored owner, identical signed retries and a missing initialized manifest, then
closes its server and removes its task SSH private key. It does not create provider
resources or qualify provider power-loss durability.

For first-initialization integration, use `scripts/cloud-initialization-smoke.py`
with the same arguments and prerequisites, plus tmux, git-lfs and an actual
OpenSSH server executable (`--sshd /absolute/sshd` accepts a task-local extracted
binary without installing or starting a system service). The fixture starts with an empty
synthetic mounted workspace, uses the actual worker's startup capture and key
preparation, and checks delayed SSH readiness, initialization, completion-save
failure/reopen, cold restarts with the same key, successful shell-only inspection,
rejection of unavailable desktop capability, lost abandonment replies and
terminal rejection of delayed commands. Synthetic provider ownership is seeded
only in the host test; provider witness transitions have separate adapter tests.
The actual checker runs against a synthetic shell-only image inventory and real
locally available base tools; this is not qualification of a deployable image.
No provider resources are created. Evidence directories are private and each run
removes its task SSH private keys and closes all fixture connections.

### Durable project reservations (low-level Linux commands)

After version-2 initialization and anchored recovery, `reserve-project` accepts a
signed project-scoped `AttachProject` with a typed `reserve` payload containing
selected capabilities and explicit application `ports`. The installed capability
checker must pass before first publication. `cancel-project-reservation` accepts
a signed `RemoveProject` with `{"action":"cancel"}`. Both commands read the same
bounded stdin envelope as bootstrap and require the pinned controller, immutable
project identity and current expected manifest revision.

These commands reserve logical names and resources only: `attaching` is never
active or provisioning permission. The canonical namespace is `project-<UUID>`.
They create no source trees, project homes, credentials, tools, processes, routes
or provider resources. Cancellation records a permanent `removed` tombstone and
releases only logical application-port and desktop reservations. Source/session provisioning, full removal, host admission coordination and
UI/CLI/MCP activation remain future work. Namespace preparation below is a
separate signed step after reservation. The allocation lock covers authentication, capability probing and atomic
manifest publication; all retained transitions carry verifiable signed history.

Only `trusted_shared` may reserve multiple identities. A `dedicated` allocation
retains its first project identity even after cancellation. Project and cloud IDs
cannot be reused. Each request permits at most 16 application ports; privileged
ports, 5900–6099 (VNC/X11) and 47280 (worker control) are unavailable. Live projects
cannot share reserved ports or the exclusive desktop. Remote-browser local ports
must be a subset of the project's application ports. These are logical conflict
checks, not OS resource quotas or demonstrated application readiness.

State retains at most 32 project identities and 64 mutations without eviction.
Every live project retains one mutation slot for cancellation; preparation also
consumes a slot.
The 64-KiB manifest limit can be reached earlier. Every live reservation reserves
4 KiB for its eventual cancellation mutation; cancellation's complete encoded
mutation must fit that bound (canonical `cancel` requests do). The worker stores
canonical signed-message encoding while preserving authenticated payload bytes.
An exact successful retry returns historical evidence without rechecking installed
capabilities. A changed operation, stale revision, or attach retry after terminal
cancellation fails. On uncertain publication, retry the same signed request;
acknowledgement waits for file and directory synchronization.

Cold SSH startup validates populated manifests and reinstalls the retained host
key. Initialize, Recover, Abandon and pre-admission inspection remain fenced to
empty revision-zero state. Even an allocation with only tombstones cannot use
pre-admission cleanup; worker-wide deletion needs a future transition contract.
The local `scripts/cloud-initialization-smoke.py --scenario reservations` lane
uses the real worker/checker over SSH with synthetic provider identities. It does
not qualify a deployed image, actual provider execution or physical power loss.

### Owning-host reservation recovery

The low-level core `cloud_runtime::project_reservations` API exposes `reserve`,
`cancel` and `resume` for a registered allocation owner. It reuses initialization's
read-only provider qualification and pinned SSH snapshots. The immutable image,
account, controller, worker and SSH bindings must still match. No provider mutation
or project provisioning is performed; calling interfaces must first resolve the
immutable owning-workspace/project identity. UI/CLI/MCP attachment remains pending.

Before sending a mutation, the controller commits its exact signed request,
proposed manifest and expected receipt in the existing native-anchored owner
journal. Only one operation may be pending. Changed inputs and successor actions
are rejected until that operation is reconciled. After a lost response or uncertain
completion save, reopen the owner and retry the original `reserve` or `cancel`
call; no replacement operation is signed. `resume` is also available while a
pending entry remains. Reopening can finish an interrupted completion save, in
which case `resume` reports no pending operation and the original call recovers
its recorded receipt. Complete receipt verification and a current ownership check
precede local completion. A failure or worker rejection without a matching receipt
leaves the operation pending; this API cannot discard uncertain intent. Confirmed
retries contact the worker again using the original request;
they remain historical reservation evidence, not fresh readiness observations.

Any retained reservation entry, including pending, corrupt or tombstone-only state,
blocks the older bootstrap cleanup, resume and empty-allocation inspection paths.
This protects an uncertain reservation even when no reply was received. Cleanup of
a previously used allocation requires the future worker-wide transition protocol.
The API timeout bounds further SSH sends; provider reads and synchronous native
journal/credential-store durability have their own bounds.

`scripts/cloud-initialization-smoke.py --scenario host-reservations` exercises the
production host coordinator and real worker over isolated SSH with synthetic
provider ownership. It covers three projects, a lost reserve response, a failed
local cancellation completion, cold restarts, exact retries and retained sibling
state. It does not qualify a deployed image or physical power-loss recovery.

### Recoverable project runtime directories

After a logical reservation, `prepare-project-namespace` accepts a signed
project-scoped `ReconcileProject` with `{"action":"prepare_namespace"}`. The owning
host exposes `project_reservations::prepare_namespace` and persists that exact
request before SSH using the same recovery rules as reserve/cancel. No caller
paths are accepted. The worker records a `preparing` transition before filesystem
creation; this state alone does not prove that a directory exists or that source,
credentials, tools or sessions are ready. An exact successful receipt records
historical namespace publication, not fresh application readiness.

Layout version 1 is `/workspace/projects/<project-UUID>/`, with private
`repository`, `worktrees`, `homes`, `runtime`, `logs` and `tools` directories.
The `.namespace-owner.json` header binds the allocation, full project identity,
preparation operation and directory inode. The allocation journal separately
anchors the inode and whether publication finished. Retained internal records
and private staging directories are not project source or credential grants.
No process, tmux session, route, credential or source tree is installed here.

The allocation lock covers intent, staged creation, ownership verification and
publication. Exact recovery can complete anchored staging or a published rename
whose acknowledgement was lost; it never replaces a published tree or recreates
missing published children. A crash between initial directory creation and its
durable inode anchor leaves uncertain, unowned staging and remains fenced. This
case is not automatically adopted or deleted. Missing or conflicting ownership,
symlinks and directory replacement also remain fenced. The same limits apply to
initial creation of the shared `projects` container.

Cancellation requires namespace preparation to be settled before publishing a
terminal tombstone. It releases logical port/desktop reservations but **retains
all project directories and data**. This is safe only because this stage creates
no processes or tool grants; future provisioning requires additional removal
semantics. A delayed preparation after cancellation fails. Worker startup
validates published ownership and permits valid anchored partial staging only
for explicit recovery. Uncertain membership still blocks bootstrap cleanup.

`scripts/cloud-initialization-smoke.py --scenario namespaces` covers three
project trees, host/worker restarts after a lost preparation reply and a failed
host completion save, exact retries and retained sibling files over real local
SSH. Provider identity is synthetic; this does not qualify a deployed image,
live provider execution or physical power loss. Public UI/CLI/MCP attachment,
session provisioning and worker-wide lifecycle remain pending.

### First committed project source

The initial source-export host lane requires Linux descriptor paths; other host
platforms fail closed until their anchored subprocess export is qualified.
`project_reservations::import_source` exports one selected committed SHA-1 Git
revision, its reachable history, local verified LFS objects and recursively pinned
submodules. It retains private immutable transfer files beside the anchored host
journal before signing `ImportProjectSource`. Retries of the same local repository
and revision selector reuse those bytes even if the selector now resolves to a
new commit; `resume` needs no original repository. There is no refresh operation.
Before creating an export directory, the host durably records one generation
intent with the selected project, repository, revision and directory name. Successful
artifact publication replaces that intent atomically. An incomplete generation,
including an uncertain save or ordinary export failure, blocks new exports on that
owner after restart; it cannot accumulate a fresh directory on each retry. Its files
remain retained. Automatic recovery or cleanup of incomplete generation is not
supported; replacement paths are never adopted or recursively removed.
Generation enforces one 4 GiB byte budget while writing the retained pack and tar
and the disposable LFS/submodule staging files. It reserves a second copy of each
retained byte plus the maximum request header for the temporary SSH input frame.
Framing rechecks that budget and rejects artifact growth before copying excess
bytes. This conservative reservation keeps retained transfer data below 2 GiB;
material staging reduces the available capacity further. Scratch is private, outside
the owner directory, and removed on ordinary completion or error; only pack and
tar are retained after a successful export. Collection stops before exceeding
65,536 tree entries, 16 MiB of accumulated project-relative path bytes (4 KiB per
path), 256 submodules or 8,192 LFS references. Declared LFS bytes, including repeated
references, share a verification limit equal to the production budget; excess
references are rejected before hashing. Local LFS storage may resolve through
symlinks, but the opened object must be a regular file with the declared size.
FIFO or device substitution is rejected without waiting for a peer.

`prepare-project-source` verifies the signed project request, live membership,
settled namespace and current capabilities on every attempt, including historical
retries. The host also repeats the existing read-only provider qualification and
immutable image check. The worker's runtime/controller binding and the pinned SSH
connection identify the actual target; the worker does not independently attest
its provider image digest. Membership becomes `importing`, which records intent,
not completed import or permission to start sessions.

`import-project-source` repeats preparation under the allocation lock, then reads
a four-byte big-endian JSON request length, that exact request, the declared Git
pack and the auxiliary tar. The descriptor binds both lengths and SHA-256 hashes;
the aggregate wire limit is 4 GiB including the length prefix and exact request bytes. Each worker request shares a 600-second deadline
across input, capability probes and validation helpers; each helper also retains
its 120-second limit. The host permits a caller-selected timeout up to 1,260 seconds
for source preparation plus import, including source resumes. Shorter caller
timeouts remain binding and are polled between host verification/framing chunks. Local export is cancellable and has separate command
bounds; synchronous filesystem calls are not preempted. Full Git responses are
capped at 16 MiB during execution, except tree listings, whose 20 MiB bound includes
the host's path budget plus per-entry metadata. Attribute responses are streamed:
expected paths and keys are checked, while non-LFS values are discarded without
buffering their full contents. Pointer inspection reads at most 1,025 bytes.
The helper and each Git child retain the allocation
lock, fencing new mutations until the final writer exits even if its parent dies.
Invalid archives, links, traversal, duplicate members,
missing commits and mismatched committed submodule/LFS identities are rejected.
Import does not check out files, fetch remotes, transfer Git config or run hooks.

The completed store publishes exclusively at
`/workspace/projects/<project-UUID>/repository/source`. It contains a bare
`repository.git`, bare `module-N.git` repositories, verified `material/lfs` assets
and the retained transfer files. Source ownership, parent/staging inode identities,
descriptor and a complete file inventory digest are anchored in the allocation
journal outside the store. Published content is validated without mutation.

An exact retry checks any retained stream prefix, completes owned staging, or
reconciles a durable import/rename whose reply was lost. A conflicting prefix,
unknown directory, changed published store, or mkdir-to-inode-anchor interruption
is fenced; recovery never resets or adopts it. Partial fixed metadata and uploaded file prefixes
can be completed only when they match the expected bytes. Unknown Git locks or
temporary object files left by an interrupted Git object publication remain
fenced; they are not deleted or included in a published source store. Attribute
inspection uses a disposable external index, so its interrupted lock does not
block a later source retry. The tests inject publication boundaries and selected
mid-helper states; they do not qualify arbitrary physical power-loss behavior.
Startup accepts anchored pending
imports for explicit recovery. Cancellation requires completed source publication
and retains source and sibling data. Delayed source requests cannot revive a
cancelled project. Source import adds no worktrees, homes, credentials, sessions,
routes or public UI/CLI/MCP activation.

The `sources` scenario in `scripts/cloud-initialization-smoke.py` exercises three
committed repositories with history, LFS and submodules through real local SSH,
including host/worker restarts after lost replies and failed completion saves.
It uses synthetic provider ownership; live-provider and physical power-loss
qualification remain separate gates under #805.
