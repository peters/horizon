# Horizon remote worker image

This directory defines the provider-neutral image contract for one interactive
Horizon coding worker. The image is intentionally separate from provider
lifecycle code: a provider prepares compute, starts this image with runtime
credentials and a lease deadline, and destroys the compute after the lease.

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
- `HORIZON_TERMINATE_AFTER`: a future RFC 3339 timestamp no more than 30 days
  away.

The optional GitHub token must be mounted as the exact read-only file
`/run/secrets/github-token`, with `HORIZON_GITHUB_TOKEN_FILE` set to that path.
Writable mounts, other paths, symlinks, and direct `HORIZON_GITHUB_TOKEN`
injection are rejected. The standard `GITHUB_TOKEN` and `GH_TOKEN` environment
variables are rejected too, because container environment values and writable
host binds violate the secret boundary.

Example:

```bash
deadline=$(
  docker run --rm --entrypoint date horizon-remote-worker:0.1.0 \
    -u --date='+30 minutes' +%Y-%m-%dT%H:%M:%SZ
)
docker run --rm \
  --publish 127.0.0.1::22 \
  --env "HORIZON_SSH_PUBLIC_KEY=$(ssh-keygen -y -f /path/to/ephemeral-worker-key)" \
  --env "HORIZON_TERMINATE_AFTER=${deadline}" \
  --mount type=bind,src=/path/to/github-token,dst=/run/secrets/github-token,readonly \
  --env HORIZON_GITHUB_TOKEN_FILE=/run/secrets/github-token \
  horizon-remote-worker:0.1.0
```

At startup the entrypoint:

1. validates the lease, public key, and optional secret file;
2. creates a new SSH host identity in the container's writable layer;
3. installs only the supplied public key for root login;
4. copies the optional token to a root-only runtime file and configures shared
   Git authentication once, before SSH accepts concurrent sessions; and
5. starts a watchdog that terminates the worker at the lease deadline.

The in-container watchdog is defense in depth. The provider remains responsible
for enforcing the same deadline and deleting the underlying compute.

`/workspace/horizon` starts empty. The controller checks out the requested
repository and task after host-key verification. Remote commands should run
through `horizon-agent-session`, which exposes the Rust toolchain, marks the
session with `HORIZON=1`, and configures GitHub authentication only when the
runtime token file exists. tmux provides reconnectable interactive sessions.

Password authentication, keyboard-interactive authentication, SSH agent
forwarding, X11 forwarding, and user-controlled SSH environment files are
disabled. Root login is public-key-only.

## Local Docker provider

`horizon_core::cloud_run::local_docker` implements the interactive-worker
contract against the local Docker daemon. Its profile name must match the
worker target and its `docker_host` must explicitly name a local Unix socket or
Windows named pipe; ambient and remote Docker contexts are rejected. The target
image must be an immutable digest reference that already exists locally.
Creation uses `--pull=never`, restart policy `no`, and an ephemeral SSH port
bound only to `127.0.0.1`; registry pulls and credentials remain outside the
provider boundary.

One workflow/job pair maps to one deterministic container name. The provider
stores the complete target, workflow and job IDs, client public key, protocol
version, and lease deadline in labels, then verifies those values plus the
runtime environment and exact 64-character container ID before reuse,
inspection, or deletion. A mismatched or malformed resource fails closed. A
delete succeeds only after inspection proves that exact ID is absent.

Docker CLI calls have bounded output and a 30-second process deadline. The
provider reports `Ready` only after Docker exposes exactly one loopback SSH
binding and the container's Ed25519 host key can be read and validated.

## Local security smoke

Run the permanent smoke harness from the repository root:

```bash
./scripts/run-remote-worker-smoke.sh
```

It builds a uniquely tagged local image, starts two isolated workers, and
proves:

- missing or malformed runtime inputs fail closed;
- source, build credentials, host keys, and user authentication state are not
  baked into the image;
- both workers accept only the supplied client key;
- strict known-host verification works and the two runtime host keys differ;
- concurrent token-backed sessions do not race or mutate shared Git state;
- the optional token is copied with mode `0600` without entering image history
  or container environment values, and writable or incorrectly located mounts
  fail closed; and
- a short lease terminates its worker and emits the watchdog marker.

Use `--image <reference>` to test an existing image. By default, an image built
by the script and all task-owned containers and temporary files are removed on
exit; `--keep-image` preserves only the locally built image.
