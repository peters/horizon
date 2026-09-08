# Horizon remote worker image

This directory defines the provider-neutral image contract for one interactive
Horizon coding worker. The image is intentionally separate from provider
lifecycle code: a provider prepares compute and starts this image with runtime
credentials. Without an explicit expiry, the worker keeps running independently
of SSH clients and Horizon. Stop/kill and cleanup are explicit management actions,
not consequences of closing a panel, exiting Horizon, or powering off the
client PC.

The image contains the Rust toolchain, Horizon's Linux build dependencies,
Git/Git LFS, GitHub CLI, rsync, tar, tmux, SSH, CA certificates, the explicit
`horizon-repository` helper, and the
supported coding-agent CLIs. Their versions and both base-image digests are
pinned in `Dockerfile` and the checksum-verified `build-tmux.sh` helper. The
worker and its CI tests use tmux 3.7c built without utmp integration; older
distribution builds can lose child-exit notifications in the upstream
[utempter race](https://github.com/tmux/tmux/issues/4559). This does not change
the client machine's tmux installation or existing servers.

## Build

Build from the repository root:

```bash
docker build \
  --file containers/remote-worker/Dockerfile \
  --build-arg WORKER_IMAGE_VERSION=0.1.0 \
  --tag horizon-remote-worker:0.1.0 \
  .
```

The default Rust and Node base images are immutable digest references. A base
override must also include a SHA-256 digest:

```bash
docker build \
  --file containers/remote-worker/Dockerfile \
  --build-arg WORKER_BASE_IMAGE=registry.example/worker-base@sha256:<64-lowercase-hex-digits> \
  --build-arg WORKER_IMAGE_VERSION=0.1.0 \
  --tag horizon-remote-worker:0.1.0 \
  .
```

Version tags make development builds understandable, but provider profiles must
use a registry digest after publication. This slice does not publish an image
or change any provider configuration.

The helper is compiled with the pinned Rust toolchain in a separate build stage.
The context admits workspace manifests, the lockfile, reviewed worker scripts and
only Rust source under the repository, core, browser and browser-protocol crates.
Build from a trusted clean checkout: the source allowlist is not a secret scanner.
Unrelated application source, local configuration, SSH material and credentials
remain excluded. Only the executable crosses into the final runtime; the existing
dependency cache receives a manifest-only tree, never Horizon source in any layer.

## Explicit repository helper

The image installs `/usr/local/bin/horizon-repository`. Nothing invokes it during
startup or starts a task after it finishes. Explicitly authorized `materialize`,
`setup` and read-only `setup-status` commands accept bounded JSON on stdin.
See the [command protocol](../../docs/remote-repository-command.md) for the complete
request, receipts, exit codes and retained-state rules.

Supply stable, authorized Git objects and an existing verified bundle store,
plus a separately prepared private scratch parent on retained storage. Publication
requires Linux journaled ext4 with barriers and readable kernel storage metadata;
container overlay storage is not sufficient. Unsupported storage fails closed and
may leave an unpublished checkout. No permission repair, overwrite or cleanup is
implicit. Missing replies are uncertain outcomes, not permission to retry.

Retained `setup` uses one immutable claim and a fixed private setup child; repeated
requests observe rather than replay. Status never creates state or proves liveness.
The commands are synchronous and do not provide independent setup supervision.
The separate `horizon-setup-launch` command can submit setup independently of its
request channel; it does not turn submission or a claim into proof of liveness.
See [independent setup submission](../../docs/remote-repository-command.md#independent-setup-submission)
for its bounded handoff, observation and recording limitations.
The same helper can explicitly receive a canonical overlay bundle into an existing
private bundle store and inspect a lost acknowledgement without writing. See
[worker overlay receipt](../../docs/remote-overlay-transfer.md) for the framed
input and storage limits. No local files are captured/exported automatically.
This packaging does not add Git-object transport, client setup, worker-loss recovery,
checkpointing or task admission. Existing workers are not
upgraded by rebuilding an image. Neither the helper nor a locally retained volume
proves cloud durability or PC-off operation.

After building, run the synthetic image smoke against an explicit local Docker
socket and a trusted journaled ext4 fixture parent:

```bash
python3 -B containers/remote-worker/test_repository_image.py \
  --docker-host unix:///path/to/docker.sock --image horizon-remote-worker:0.1.0
python3 -B containers/remote-worker/test_repository_packaging.py \
  --docker-host unix:///path/to/docker.sock
python3 -B containers/remote-worker/test_setup_launch.py -v
```

The first checks large packed objects, raw index/worktree semantics, no-overwrite
publication, retained data observed by a fresh container and overlay rejection.
It also checks retained setup/status, intent conflicts, unknown claims without
replay, unsafe result preflight and status after output loss. The claim-only case
is a synthetic state fixture, not proof of surviving an actual interruption.
It uses no network or credentials and removes only its owned containers/fixtures.
The second checks the real Docker context filter with positive source controls and
excluded synthetic files. Add `--image` and `--expected-binary-sha256` from a separate
`repository-builder` target to audit every final image layer and executable provenance.
These tests retain images and fail, rather than claim success, on unsupported storage.

To exercise independent setup through local SSH with a fresh synthetic client key,
use local rootless Docker or a root host caller. The SSH worker runs as container
root and must own the private host-created fixtures; the harness checks this
precondition without changing ownership:

```bash
python3 -B containers/remote-worker/test_setup_launch_image.py \
  --docker-host unix:///path/to/docker.sock --image horizon-remote-worker:0.1.0
```

This uses the real setup helper with a synthetic Git gate to keep admitted setup
in progress while its request/output channel disappears. It then releases the
gate, verifies the exact repository through a separate status channel, checks no
claim replay and completed-child reaping, and runs an ungated submission. It binds
only loopback SSH and removes its own containers, fixture and generated keypair.
The gate is explicit test instrumentation, not cloud/PC-off or crash-durability proof.

To check framed overlay receipt and fresh-process observation with synthetic data:

```bash
python3 -B containers/remote-worker/test_overlay_receive_image.py \
  --docker-host unix:///path/to/docker.sock --image horizon-remote-worker:0.1.0
```

This covers exact small and 65 MiB-plus multi-file bundles, immutable retries,
malformed frames, conflicting/unsafe records, missing roots and acknowledgement
loss. It uses no network or credentials, changes only its private input stores,
and removes only its own containers/fixtures. It is not cloud/checkpoint proof.

## Runtime contract

Each worker requires:

- `HORIZON_SSH_PUBLIC_KEY`: exactly one valid OpenSSH Ed25519 public key, with
  a 16 KiB limit.

`HORIZON_TERMINATE_AFTER` is optional. Leave it **unset** for a persistent worker;
no termination watchdog is started and no client heartbeat/renewal is required.
For an explicitly time-limited worker, supply a future RFC 3339 timestamp no more
than 30 days away. A supplied empty, malformed, past, or out-of-range value is a
configuration error, not an instruction to run forever. Existing providers that
supply deadlines retain their bounded behavior; provider API/profile support for
persistent lifetime is a separate integration step.

The optional GitHub token must be mounted as the exact read-only file
`/run/secrets/github-token`, with `HORIZON_GITHUB_TOKEN_FILE` set to that path.
Writable mounts, other paths, symlinks, and direct `HORIZON_GITHUB_TOKEN`
injection are rejected. The standard `GITHUB_TOKEN` and `GH_TOKEN` environment
variables are rejected too, because container environment values and writable
host binds violate the secret boundary.

Persistent example (stop and remove this exact container manually when finished):

```bash
docker run --detach --name horizon-development \
  --publish 127.0.0.1::22 \
  --mount type=volume,src=horizon-development-data,dst=/workspace \
  --env "HORIZON_SSH_PUBLIC_KEY=$(ssh-keygen -y -f /path/to/ephemeral-worker-key)" \
  --mount type=bind,src=/path/to/github-token,dst=/run/secrets/github-token,readonly \
  --env HORIZON_GITHUB_TOKEN_FILE=/run/secrets/github-token \
  horizon-remote-worker:0.1.0
```

For a temporary worker, add `--env HORIZON_TERMINATE_AFTER=<RFC-3339-deadline>`.
An explicit deadline is independent of whether a client is attached. It is not
the default product lifetime or a replacement for manual management.

At startup the entrypoint:

1. validates any supplied expiry, the public key, and optional secret file;
2. installs only the supplied public key for root login;
3. copies the optional token to a root-only runtime file and configures shared
   Git authentication once, before SSH accepts concurrent sessions;
4. prepares or strictly recovers the workspace-retained Ed25519 host identity
   and materializes its verified runtime files before SSH starts; and
5. starts a termination watchdog only when an explicit expiry was supplied.

For explicitly time-limited workers, the in-container watchdog is defense in
depth; the selected provider policy owns compute expiry and cleanup. Persistent
workers may continue incurring cost until explicitly stopped. Local connection
loss must not be interpreted as permission to stop or delete them.
Retain the exact container/provider handle outside the client session until the
management UI is available. The SSH key remains authorized while the worker is
running; losing the client reference neither revokes access nor stops compute.

`/workspace/horizon` starts empty. The controller checks out the requested
repository and task after host-key verification. Remote commands should run
through `horizon-agent-session`, which exposes the Rust toolchain, marks the
session with `HORIZON=1`, and configures GitHub authentication only when the
runtime token file exists. tmux provides reconnectable interactive sessions.

The provider or operator must supply retained storage at `/workspace`; the image
cannot prove that the backing storage is durable. The named volume in the example
retains that directory across container removal, but not volume deletion or host
loss. Remote backup/checkpointing, agent-state durability, provider restart
coordination and the Remote Environments overview remain separate requirements.
The full remote workspace must use the separately validated storage design. A container
on the client PC also cannot keep running when that same PC powers off: local
Docker smoke demonstrates disconnection semantics, not the real cloud PC-off gate.

Password authentication, keyboard-interactive authentication, SSH agent
forwarding, X11 forwarding, and user-controlled SSH environment files are
disabled. Root login is public-key-only.

## Retained panel sessions

After repository setup, `horizon-panel-session` provides separate explicit
start and non-creating reconnect operations. Supply the exact runtime generation
UUID and stable panel ID (letters, digits, underscores or hyphens, at most 128
characters). For example, inside the worker:

```bash
horizon-panel-session start c8203298-3169-48d6-84fd-882d8d49a7b4 terminal_a . -- /bin/bash
horizon-panel-session status c8203298-3169-48d6-84fd-882d8d49a7b4 terminal_a
horizon-panel-session attach c8203298-3169-48d6-84fd-882d8d49a7b4 terminal_a
```

Attach requires a terminal, such as a verified SSH connection with PTY allocation.
Working directories are relative to `/workspace/horizon`; traversal and symlinks
escaping that repository are rejected. Arguments are passed directly through
`horizon-agent-session`, never reconstructed as shell text. Use an explicit shell
program only when shell evaluation is actually intended.

Controllers can invoke the fixed command `horizon-panel-session request` and
send one UTF-8 JSON object on stdin, then close stdin. This keeps task arguments
out of a reconstructed remote shell command. Requests are bounded to 512 KiB,
require version 1 and reject duplicate/unknown fields or invalid field types.
For an explicit start:

```json
{"version":1,"operation":"start","runtime":"c8203298-3169-48d6-84fd-882d8d49a7b4","panel":"terminal_a","directory":".","argv":["/bin/bash"]}
```

A status request contains only `version`, `operation`, `runtime` and `panel`,
with `operation` set to `status`. Both return the existing JSON status shape.
Status never creates a session; repeating start retains the existing one-shot
intent rules. Interactive attachment still uses the separate `attach` command
and PTY. Malformed request errors never echo stdin or task content. This protocol
does not itself provide authenticated transport or authorize a new task.

A `verify` request uses the same fields as `start`, but only compares the supplied
directory and literal argv with the retained task's launch digest before returning
its status. It never creates a marker, tmux server, session or replacement task.
A changed intent is rejected; a missing task stays absent and a completed task
stays retained. Controllers must use the saved launch intent instead of silently
attaching a panel identity whose command has since changed. This check alone does
not verify repository contents, remote durability or permission to attach. Intent
comparison is independent of current filesystem contents, so renaming or removing
the original directory does not hide a retained running or completed task. Fresh
task startup still resolves and confines its working directory to the repository.

Before starting anything, the helper durably publishes one private no-overwrite
marker for the runtime/panel pair. Repeating the same start can only inspect that
task; a changed launch intent is rejected. Each runtime has a dedicated tmux
socket, and reconnect verifies a retained instance nonce before attaching. A lost
start reply or missing server never grants permission to execute a replacement.
An unavailable result needs an explicit recovery decision; do not remove the
marker to make a retry work.

Closing the last client does not destroy the server, session or task. Completed
panes remain available without reexecution. Status reports `running`, `exited`
or `unavailable`, with an exit status only when tmux has one; an unknown result,
including some signal terminations, is not reported as success.

Markers live under `/workspace/.horizon-worker/panels` and contain an intent
digest, not command arguments or credentials. Startup prepares this private root;
task requests never recreate it if it disappears. Sockets remain under
`/run/horizon/panels` and are not retained across runtime filesystem replacement.
These locations rely on the worker's private root-owned directory boundary;
tasks within one worker share that trust domain. The marker is not a process
checkpoint: container/server loss cannot preserve a running process. With the
workspace volume retained, both running and completed task identities become
`unavailable` after server loss; even a repeated start must not execute them again.
Durable backing storage, backup and explicit recovery remain separate requirements.
This helper does not yet connect local panels, stop tasks or delete workspaces.

With `bison`, `libevent-dev`, `libncurses-dev`, a C compiler and Make installed, build a
disposable test binary under a new prefix, then run the worker regressions:

```bash
test_root=$(mktemp -d /tmp/horizon-panel-tests.XXXXXX)
bash containers/remote-worker/build-tmux.sh "$test_root/tools"
PATH="$test_root/tools/bin:$PATH" python3 -B containers/remote-worker/test_panel_sessions.py -v
```

Coverage includes repeated PTY disconnects, retained completion, concurrent
starts, literal semicolon/format arguments, locale-independent status and no
recreation after server loss. Structured-request coverage includes literal argv,
non-creating status, disconnected progress, bounded/invalid input and redaction.
Verification covers changed intent, absent/lost/completed tasks, exact literal
arguments and unchanged ownership records while disconnected tasks keep running.
Retention coverage replaces the runtime/socket filesystem and rejects missing,
linked, insecure or foreign-owned state without replaying old task identities.
Tests own only their private temporary directories
and dedicated sockets. Keep the disposable tool prefix for repeated validation,
then remove only that exact task-owned prefix when finished.

## Retained SSH host identity

`host-identity.py` owns `/workspace/.horizon-worker/ssh.claim` and the private
`ssh/` directory next to it. The initialization claim is durably recorded before
key generation. Version 2 readiness also requires the private sibling `panels/`
directory: it is created and synchronized before readiness is published, together
with both synchronized key files. Repeated startup validates the original access-key digest,
marker and real private/public key pair; it does not generate another identity.
The verified runtime copies remain at `/etc/ssh/ssh_host_ed25519_key{,.pub}` so
trusted provider host-key inspection keeps its existing path.

This requires a trusted Linux POSIX-permission filesystem. Workspace ancestors
must have trusted ownership and no unprotected group/world write access. Identity
directories and files must be owned by the worker user with no group/world
permission bits. New directories and files use modes `0700` and `0600`;
existing entries may use other owner-only modes. Symlinks, partial initialization,
missing files, corruption, changed access
keys and conflicting runtime keys fail before SSH starts. An interrupted first
initialization may require explicit recovery; it is never silently retried as
permission to replace an identity. Existing unretained keys are not automatically
migrated. Key rotation, old-volume migration and whole-volume loss require a
separate verified recovery operation. Never resolve a host-key mismatch by
disabling client verification.

Version 1 storage kept panel records on the runtime filesystem. It is rejected
without automatic migration, key rotation or record replacement: missing old
records cannot prove that a task was never started. Version 2 also rejects a
missing/insecure retained panel root instead of creating an empty replacement.
Do not edit the version or remove records to force startup. Existing running
workers are not upgraded or stopped by this source change.

Concurrent starters wait up to 30 seconds for readiness: two ten-second key
operation budgets plus ten seconds of filesystem synchronization grace. This is
a bounded readiness wait, not a filesystem I/O deadline. A stalled or slower
initialization fails closed for waiting starters; a later startup can validate
the completed identity, but a timeout never authorizes replacement.

Back up the complete private `.horizon-worker` directory with the repository;
protect it as credential-bearing data. Host keys are unencrypted on disk and
worker tasks share the root-owned trust domain. This is not a per-task sandbox,
a backup service, or proof that interrupted processes resumed.

Validate using synthetic fixtures and a task-owned local volume:

```bash
python3 -B containers/remote-worker/test_host_identity.py -v
bash scripts/run-remote-worker-host-identity-smoke.sh --image horizon-remote-worker:0.1.0
```

The smoke replaces the container filesystem while retaining its workspace volume,
checks the same host identity through pinned SSH, retains task records without
replaying completed or interrupted work, and rejects a different access
key without modifying the original identity or workspace data. It removes only
its own containers and volume; it neither publishes an image nor uses cloud resources.

## Local Docker provider

`horizon_core::cloud_run::local_docker` implements the interactive-worker
contract against the local Docker daemon. Its profile name must match the
worker target and its `docker_host` must explicitly name a local Unix socket or
Windows named pipe; explicit remote endpoints are rejected and ambient context
selection is ignored. The target image must be an immutable digest reference
that already exists locally.
Creation uses `--pull=never`, restart policy `no`, and an ephemeral SSH port
bound only to `127.0.0.1`; registry pulls and credentials remain outside the
provider boundary.

One workflow/job pair maps to one deterministic container name. The provider
stores the complete target, workflow and job IDs, canonical client public key,
protocol version, and lease deadline in labels, then verifies those values plus
the runtime environment and exact 64-character container ID before reuse,
inspection, or deletion. A mismatched or malformed resource fails closed. A
delete succeeds only after inspection proves that exact ID is absent.

Docker CLI calls have bounded output, a conservative Windows-compatible
argument budget, and a 30-second process deadline. A container disappearing
during host-key discovery is reported as absent or reconciled before reuse.
The provider reports `Ready` only after Docker exposes exactly one loopback SSH
binding and the container's Ed25519 host key can be read and validated.

## Local security smoke

Run the permanent smoke harness from the repository root:

```bash
./scripts/run-remote-worker-smoke.sh
```

It builds a uniquely tagged local image, starts isolated leased and persistent
workers, and proves:

- missing keys and explicitly supplied invalid expiry values fail closed;
- source, build credentials, host keys, and user authentication state are not
  baked into the image;
- both workers accept only the supplied client key;
- strict known-host verification works and the two runtime host keys differ;
- concurrent token-backed sessions do not race or mutate shared Git state;
- the optional token is copied with mode `0600` without entering image history
  or container environment values, and writable or incorrectly located mounts
  fail closed;
- an explicit short lease terminates its worker and emits the watchdog marker;
- a no-expiry worker's task progresses with all its SSH clients disconnected,
  then reconnects with the same container, tmux session, process, and pinned
  host key;
- manually stopping that exact persistent test worker stops execution without
  removing the container or its last observed workspace progress.

Use `--image <reference>` to test an existing image. By default, an image built
by the script and all task-owned containers and temporary files are removed on
exit; `--keep-image` preserves only the locally built image.
Interrupt, termination, and hangup signals use the same exact-resource cleanup.
SIGKILL or host loss can bypass cleanup and leave the no-expiry test worker
running. Retain the printed task-owned name prefix, inspect the
exact container IDs from that run, and manually remove only those verified test
resources. The harness does not sweep shared smoke labels or other runs.
