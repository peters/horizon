# Linux UI development worker

Optional desktop-testing layer over the existing [remote worker](../remote-worker/README.md).
Build tools, agent tools, SSH, retained sessions and credentials come from that
base. This layer adds a private X11 desktop and a native Horizon smoke command.
The default build also installs Chromium with Debian's matching
ChromeDriver, Firefox ESR and
Mozilla geckodriver 0.37.1 (SHA-256-verified official release assets). Browser
versions follow the Debian security repository at build time; retain the final
image digest and build log for reproducibility. No browser profile or login is baked in.
The default image supports native UI and `horizon-browser` smoke testing.
A smaller `native` target is available when browser testing is explicitly out of scope.
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
The helper snapshots the executable before hashing and launching it, so a
concurrent build cannot change the tested candidate. Cleanup removes the snapshot.
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

For an explicitly native-only fixture, build the smaller target:

```bash
docker build --target native \
  --build-arg WORKER_IMAGE='<registry>/horizon-remote-worker@sha256:<digest>' \
  -t horizon-linux-ui-worker:native containers/linux-ui-worker
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

### Agent filesystem sandbox

Run `horizon-agent-sandbox-smoke` in a credential-free disposable container using
the selected image, runtime policy and agent UID before the first issue offload.
The helper uses the installed CLI's `:workspace` permission profile with an empty
private home. It needs neither login nor network access. It proves workspace
writes succeed, outside file creation and overwrite are denied, and outside bytes
remain unchanged. Normal writes to both locations are checked first so Unix file
permissions cannot produce a false sandbox pass. Each command is bounded to 30
seconds, and only probe-owned files and descendant processes are cleaned up.

```bash
docker run --rm --network none \
  --entrypoint horizon-agent-sandbox-smoke "$WORKER_IMAGE"
```

Require exit zero **and** JSON `passed: true`. CLI installation, version output,
an empty success exit or an SSH-ready worker are insufficient. The tested rootless
Docker daemon rejects the default-filter invocation because namespace creation is
blocked. On that daemon the per-container `--security-opt seccomp=unconfined`
setting used by the browser recipe permits the agent sandbox canary to pass,
without additional capabilities. This is a container security policy choice, not
an image feature or an authorization from repository YAML. It is not qualified for
rootful Docker or Azure by this local test; do not apply it silently to an existing
worker or change host sysctls. The provider adapters do not yet configure an agent
sandbox runtime policy automatically.

Run the probe again after changing the image, CLI, runtime policy or UID. A pass
qualifies only the built-in filesystem profile on a temporary workspace. It does
not qualify network isolation, custom agent profiles, real repository/toolchain
access, authentication, browser smoke or Azure/client-off behavior. Older published
images may lack the helper; build this image or copy the reviewed helper and
`processes.py` into a disposable qualification fixture, keeping credentials absent.

Run probe regressions with:

```bash
python3 -m unittest discover -s containers/linux-ui-worker -p 'test_agent_sandbox.py'
```

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
