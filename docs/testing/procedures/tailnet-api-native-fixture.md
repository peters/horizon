---
procedure: tailnet-api-native-fixture
feature: synthetic tailnet native qualification tooling prerequisite
platforms: [linux]
cost: none
destructive: yes       # exact synthetic credential, metadata and container cleanup
secrets: [synthetic auth keys and API credentials in an isolated Secret Service]
owner: peters
---

# Tailnet native fixture test procedure

## 1. Purpose

This procedure tests reproducible native tooling on the API storage foundation.
It tests the real synthetic OS store, native transport and existing public MCP interfaces.

## 2. Applicability

- Base: API storage `79d32b9e9716b741ae03104d857416cecc2f6851`.
- Linux x86_64 with a task-owned container, private Secret Service and frozen production executables.
- Baseline mode has no status product overlay or new API-status rows/map.
- Baseline never invokes the unshipped `cloud_deploy tailnets` route.
- The future status candidate must invoke `--expect-api-status` under its own feature procedure.
- Baseline proof does not qualify that future feature, another platform, enrollment or a provider.

## 3. Safety

> **CAUTION:** USE ONLY TASK-OWNED SYNTHETIC CREDENTIALS AND STATE.
> The operator's desktop keychain and provider credentials are outside this run.

> **CAUTION:** REMOVE ONLY THE EXACT RESOURCES RECORDED BY THIS FIXTURE.
> Broad cleanup can remove another application's data or stop its processes.

The fixture mounts no host home, runtime, X11 or bus socket.
It has no provider configuration or external network. No real credential is read.
The artifact manifest and build invocation belong to the operator. This fixture
is not a sandbox for arbitrary Rust build scripts. Review the candidate before
building it. Source hashing and control copying accept only declared checkout
inputs and refuse symlinks. The running container can write its state directory;
the host treats those entries as untrusted. Host control reads use bounded regular
file descriptors with no-follow checks. Host output creation is exclusive and
does not truncate pre-existing candidate entries. Docker operations have explicit
timeouts. A timed-out launch preserves its unique name, labels and daemon for
exact reconciliation; uncertainty never authorizes broad removal.

## 4. Equipment and preconditions

- A hydrated checkout, Rust toolchain, Python 3 and healthy Docker on Linux x86_64.
- OS runtime dependencies and the exact image from the
  [checked-in README](../../../scripts/tailnet-api-status-fixture/README.md), including executable
  `xkbcomp` and the X11 keyboard-map data. All declared runtime prerequisites pass
  before artifact freezing, test build or container creation.
- Frozen production Horizon, existing cloud_deploy and standalone horizon-device with its CLI feature.
- A private artifact manifest and root build evidence for their consumed source inputs and hashes.
- A new private evidence directory outside the checkout.
- A public Device-panel viewer in the caller's workspace and the exact owned X11 input target.

## 5. Setup

1. Complete the build and artifact freeze from the README.

   Result: The executables belong to this candidate. Hashes alone do not establish source provenance.

2. Record the base, source hashes, toolchain, flags and OS runtime dependencies.

   Result: First/final comparisons can identify all consumed inputs.

## 6. Tasks

### 6.1 T01 — Offline control checks

1. Run the offline fixture checks and repository statics.

   ```sh
   python3 -B -m unittest discover -s scripts/tailnet-api-status-fixture/tests -v
   cargo fmt --all -- --check
   ./scripts/check-maintainability.sh
   python3 scripts/check-horizon-mcp-skills.py
   git diff --check
   ```

   Result: Mode guards, public-response parsing and ownership checks pass without Docker, sockets or native processes.
   Missing or unexecutable native helpers and missing keyboard-map data refuse before
   artifacts, build or container work. The isolated X11 directory has mode `0700`;
   existing directories and symlinks cannot be reused. Resolved XKB/evdev bounds,
   actual CLI readiness envelope/target validation, pre-credential ordering and
   daemon/CID/nonce-bound stop requests also pass offline. Candidate-planted file
   and directory symlinks, hardlinks, nonregular entries and existing outputs
   refuse without changing an outside sentinel. Launch timeout tests cover exact
   label reconciliation, daemon changes and uncertainty preservation. These tests do not
   qualify actual X11, Secret Service, VNC or public host routing.
   Product and bundled skill bytes stay unchanged.

### 6.2 T02 — Real synthetic store and exact ledger

1. Start a baseline setup/cleanup check with a new private path.

   ```sh
   python3 scripts/tailnet-api-status-fixture/run.py start --check-only --evidence-dir /tmp/horizon-tailnet-tooling-check --manifest /tmp/horizon-tailnet-tooling-artifacts.json --target-dir /tmp/horizon-tailnet-tooling-target
   ```

   Result: The runner builds and freezes the exact ignored Linux test executable.
   Native prepare/check/cleanup phases use a private real Secret Service with no GUI.
   Three synthetic bindings have missing, personal-token and OAuth-client kinds in the shared store.
   An unknown catalog ID refuses cleanup before any owned binding changes.
   Catalog restoration, known-ID removal and rename preserve the remaining stored kind.
   These direct store results are fixture inputs, not MCP observations.

2. Run exact cleanup after the check container exits.

   ```sh
   python3 scripts/tailnet-api-status-fixture/run.py cleanup --ownership /tmp/horizon-tailnet-tooling-check/ownership.json
   ```

   Result: The owned service is empty, retained processes stop and the exact container is absent from the same healthy daemon.
   Only then are the synthetic credential files and password removed. Ordinary ignored-test skips do not qualify T02.

### 6.3 T03 — Real native transport and baseline public MCP

1. Start a new baseline native fixture.

   ```sh
   python3 scripts/tailnet-api-status-fixture/run.py start --evidence-dir /tmp/horizon-tailnet-tooling-native --manifest /tmp/horizon-tailnet-tooling-artifacts.json --target-dir /tmp/horizon-tailnet-tooling-target
   ```

   Result: The actual frozen candidate starts in an owned, network-disabled container with private X11 and bus state.
   A new `/tmp/.X11-unix` directory is created with mode `0700` before credentials
   or Xvfb start. The resolved XKB tree is mounted read-only at `/fixture/xkb` and
   Xvfb uses `-xkbdir /fixture/xkb`, with no writable keyboard cache or host socket.
   Before Secret Service construction or credential preparation, the exact frozen
   horizon-device doctor validates the X11 protocol within a total five seconds.
   `state/x11-readiness.json` records its nonsecret result; socket existence alone
   is not readiness. A failure stops owned Xvfb and expires the target without credentials.
   Its running executable hash matches the frozen artifact.

2. Start the owned real VNC relay in a separate task terminal.

   ```sh
   python3 scripts/tailnet-api-status-fixture/vnc_proxy.py --ownership /tmp/horizon-tailnet-tooling-native/ownership.json
   ```

   Result: `state/vnc-address.json` gives the numeric loopback endpoint.
   The relay rechecks exact ownership and transports actual x11vnc bytes with bounded clients, queues and lifetime.

3. Create a public Device panel at that endpoint.

   Result: Three timestamped inspections show connected, displayed and advancing frames while target output changes.
   The viewer supplies presentation evidence and video only; it has no input operation.

4. Examine the observer's timestamped public responses.

   Result: A normal **AGENT** panel launches the observer. Horizon supplies its actor and host instance.
   Public initialize, tools/list, cloud_list and cloud_companions succeed while Settings stays closed.
   The initial legacy catalog has the three prepared IDs/names, each with exactly id/name fields.
   An empty cloud list is valid. Baseline does not require or qualify an API status map.
   Raw responses are in `state/mcp-evidence/rpc.jsonl`; the summary derives from those replies.
   Private host queues, stamps and caches are not substitutes.

5. Open **Settings > Tailnets** with the exact owned standalone input target.

   Result: The README's doctor/screenshot/act commands bind only this container's `/fixture/state/target.json` and display `:71`.
   The existing three saved names appear. No new API-status row or credential editor is expected.
   Native input receipts and application results remain separate.

6. Capture a short native video through the public Device panel.

   Result: Actual candidate frames, viewer inspections and public MCP timestamps have separate evidence.
   A sanitized GIF can demonstrate the tooling; it does not claim a new product layout.
   Keep generated media outside the checkout. Publish through the approved GitHub
   attachment command when needed; never commit the GIF.

## 7. Pass criteria

- T01 through T03 pass on the first reviewable and final candidate inputs.
- The real store, ledger refusal/cleanup, live native transport and baseline public host routes have separate evidence.
- The public three-tailnet id/name catalog matches preparation without private application-state proof.
- No baseline result claims a future API status map, rows or CLI route.
- Future status qualification needs its explicit mode and separate feature acceptance tests.
- Exact owned cleanup completes; uncertainty is preserved instead of treated as absence.

## 8. Cleanup

1. Close the owned candidate normally through native input.

   Result: Ledger-bound cleanup removes only known remaining bindings, proves the service empty and stops retained children.
   Normal close also requires the recorded public bootstrap. The exact X11 target expires.

2. Close the owned public viewer.

   Result: The Device panel closes without stopping another target.

3. Request the exact owned VNC relay to stop.

   ```sh
   python3 scripts/tailnet-api-status-fixture/vnc_proxy.py --ownership /tmp/horizon-tailnet-tooling-native/ownership.json --stop
   ```

   Result: The same daemon, immutable container, labels and per-relay nonce bind
   the private exclusive stop request. The actual relay stops and joins its retained
   Docker-exec children, closes the listener and removes the endpoint before writing
   its actual stopped result. Wait for the original relay execution to exit normally
   with code zero. A request receipt alone is not cleanup proof. Do not interrupt
   the shell wrapper, kill by host PID/name, or use a Horizon product queue.

4. Reconcile the exact container and synthetic files.

   ```sh
   python3 scripts/tailnet-api-status-fixture/run.py cleanup --ownership /tmp/horizon-tailnet-tooling-native/ownership.json
   ```

   Result: The same healthy Docker daemon proves the exact container absent before owned file/password removal.
   Artifacts, logs and ownership records remain private.
   On failure preserve the state for exact reconciliation; no broad prune or host-bus fallback is permitted.

## 9. Record of results

Record commands, hashes, modes and cleanup separately from native and public-interface observations.
Record the relay's exact container and nonce with its stopped receipt. Before cleanup,
substitute a different synthetic nonce in that receipt and verify that cleanup refuses
before removing the container or credential files. Restore the original receipt and
complete exact cleanup. The stop request alone is not a stopped receipt.
Reuse requires the repository's consumed-input comparison. An earlier private helper or status-overlay result does not qualify this baseline.
