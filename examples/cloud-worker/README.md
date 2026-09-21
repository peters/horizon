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
control service. Device control/ownership works without a browser executable or
browser MCP registration. Agent configuration contains only enabled tool servers.
Disabled agent requests are rejected before writing session or worktree state.

Horizon supplies `HORIZON_AGENTS`, `HORIZON_BROWSERS` and `HORIZON_DESKTOP` Docker
build arguments from the profile. The standalone Dockerfile defaults to a minimal
image; pass these arguments explicitly when building outside Horizon. The image
records available features in `/etc/horizon-worker/capabilities.json`. Before any
allocation, the image checker verifies the requested subset and actual executable
runtimes, with networking disabled. Requested features are supplied through the
nonsecret `HORIZON_WORKER_CAPABILITIES` environment binding, preserving legacy
checker arguments. Capability-aware images still validate the entire requested
selection, including the full default selection, and run their readiness checks.
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
`horizon-capabilities-contract=1`. Source transfer carries verified LFS objects and
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

Worker readiness checks the selected active capability set and the control service;
profiles with desktop also require a responding VNC server. Application readiness
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
