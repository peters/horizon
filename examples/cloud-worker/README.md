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
runtimes, with networking disabled. Old images can satisfy the legacy selection;
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
