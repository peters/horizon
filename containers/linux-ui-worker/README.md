# Linux UI development worker

Optional desktop-testing layer over the existing [remote worker](../remote-worker/README.md).
Build tools, agent tools, SSH, retained sessions and credentials come from that
base. This layer adds a private X11 desktop and a native Horizon smoke command.
It also installs Chromium with Debian's matching ChromeDriver, Firefox ESR and
Mozilla geckodriver 0.37.1 (SHA-256-verified official release assets). Browser
versions follow the Debian security repository at build time; retain the final
image digest and build log for reproducibility. No browser profile or login is baked in.
It does not publish an image or create cloud resources.

## Build

Use the digest of a tested base image, with full agent tools for development or
the compact Shell image for a test-only fixture:

```bash
docker build --build-arg WORKER_IMAGE='<registry>/horizon-remote-worker@sha256:<digest>' \
  -t horizon-linux-ui-worker:local containers/linux-ui-worker
```

Local rehearsals may use an existing local base tag instead; record its image ID
with `docker image inspect` before and after building. Published images must use
a registry digest. A local image ID is not necessarily a registry manifest digest.

The default entrypoint remains the existing worker's SSH service. Prepare the
repository and credentials using its existing contract. Build the candidate
inside the worker so its shared-library requirements match the environment:

```bash
cargo build --locked -p horizon-ui
horizon-linux-ui-smoke --binary target/debug/horizon --artifacts /tmp/ui-proof-001
```

The artifacts directory must not already exist; its parent must exist. It is
created privately and contains launch/resized PNGs, process logs, a synthetic
terminal fixture, an isolated home/config and `result.json` with the binary hash.
Inspect both images before reporting visual acceptance. Exit zero requires
window/terminal startup, terminal input before and after a measured window resize,
normal window-manager close and cleanup. The default active timeout is 90 seconds
(30–600 configurable); cleanup has a separate ten-second bound. The standalone
helper uses Linux subreaper ownership to collect orphaned PTY children after a
candidate crash. It requires Linux procfs, pidfds and Python 3.11 or newer.
Failures preserve evidence and return nonzero. No current desktop, agent login,
Horizon home or display environment is inherited by the candidate.

For a local container rehearsal, bind only the candidate and a task-owned output
directory; use no host display socket or privileged mode. Override the entrypoint
with `horizon-linux-ui-smoke`, pass `--binary` and `--artifacts` container paths,
and use `--network none`. Retain the exact container identity for failure cleanup.

From the source checkout, this complete local rehearsal uses only temporary
build/proof directories and the image built above:

```bash
smoke_root=$(mktemp -d /tmp/horizon-linux-ui.XXXXXX)
mkdir "$smoke_root/build" "$smoke_root/proof"
docker run --rm \
  --mount "type=bind,src=$PWD,dst=/source,readonly" \
  --mount "type=bind,src=$smoke_root/build,dst=/build" \
  --env CARGO_TARGET_DIR=/build --workdir /source \
  --entrypoint /bin/bash horizon-linux-ui-worker:local \
  -lc 'cargo build --locked -p horizon-ui'
docker run --rm --network none \
  --mount "type=bind,src=$smoke_root/build/debug/horizon,dst=/candidate/horizon,readonly" \
  --mount "type=bind,src=$smoke_root/proof,dst=/proof" \
  --entrypoint horizon-linux-ui-smoke horizon-linux-ui-worker:local \
  --binary /candidate/horizon --artifacts /proof/run
```

Keep `smoke_root` for image inspection and debugging. With rootful Docker, files
may be owned by container root; rootless Docker maps that user to the caller.
This example proves native UI. Run the browser lane separately for each engine:

```bash
# The task-owned proof parent must be writable by container user 1000.
docker run --rm --network none --user 1000:1000 \
  --security-opt seccomp=unconfined \
  --mount "type=bind,src=$PWD,dst=/source,readonly" \
  --mount "type=bind,src=$smoke_root/build/debug/horizon,dst=/candidate/horizon,readonly" \
  --mount "type=bind,src=$smoke_root/proof,dst=/proof" \
  --entrypoint horizon-linux-ui-smoke horizon-linux-ui-worker:local \
  --binary /candidate/horizon --repository /source \
  --browser chromium --artifacts /proof/chromium --timeout-seconds 150
# Repeat with --browser firefox and a new --artifacts directory.
```

With rootless Docker, prepare the proof directory ownership from a task container
(`chown 1000:1000 /proof`) and restore it to container root afterward if needed.
The per-container seccomp setting allows user namespaces on the tested rootless
runtime. Chromium's own sandbox stays enabled; no privileged mode, host display,
external network, browser debug-port access or real credentials are needed.

The helper creates a synthetic agent panel inside the isolated candidate and uses
that candidate's public `horizon --browser-mcp` transport. It reuses the repository's
MCP client from `scripts/browser-smoke/mcp_gate.py`, so `--repository` must point to
the matching checkout. Each lane creates an embedded browser panel, loads a
loopback fixture, fills and submits a form, checks its result, captures the native
Horizon window after resize and closes the exact candidate normally. The receipt
includes the public-MCP lane; `browser-result.json` records its assertions.

## Development validation

Run the complete pre-push matrix in [AGENTS.md](../../AGENTS.md) in the exact
candidate checkout. This smoke supplements that matrix; it is not a substitute
for Rust tests, review or issue-specific UI assertions. Keep build output and
Cargo caches in task-owned retained directories, outside the image and source.

Software rendering is suitable for launch, layout and input regressions. It does
not prove hardware-specific GPU behavior, real audio capture, or cloud/client-off
durability. The baseline uses a synthetic local Shell panel and no browser.
Browser interaction must use the Horizon browser skill and its public MCP tools;
the image alone does not establish their availability. Never use the installed
drivers directly as an alternate agent browser controller. Chromium's sandbox
also needs a suitable unprivileged runtime; packaging does not authorize disabling
it or exposing driver ports. Persistence/restore and motion-sensitive issues
require their own smoke scenarios and evidence.
