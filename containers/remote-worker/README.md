# Horizon remote worker image

This directory defines the provider-neutral image contract for one interactive
Horizon coding worker. The image is intentionally separate from provider
lifecycle code: a provider prepares compute and starts this image with runtime
credentials. Without an explicit expiry, the worker keeps running independently
of SSH clients and Horizon. Stop/kill and cleanup are explicit management actions,
not consequences of closing a panel, exiting Horizon, or powering off the
client PC.

The image contains the Rust toolchain, Horizon's Linux build dependencies,
Git/Git LFS, GitHub CLI, rsync, tar, tmux, SSH, CA certificates, and the
supported coding-agent CLIs. Their versions and both base-image digests are
pinned in `Dockerfile`.

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

Only workspace manifests and the lockfile enter the dependency-cache build
stage. Horizon source, local configuration, SSH material, tokens, and registry
credentials are excluded from the build context and final image.

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

## Retained SSH host identity

`host-identity.py` owns `/workspace/.horizon-worker/ssh.claim` and the private
`ssh/` directory next to it. The initialization claim is durably recorded before
key generation. A complete versioned marker is published only after both key
files are synchronized. Repeated startup validates the original access-key digest,
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
checks the same host identity through pinned SSH, and rejects a different access
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
