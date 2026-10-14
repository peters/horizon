# Synthetic tailnet native qualification fixture

This checked-in fixture prepares and cleans synthetic credentials through the real
Linux Secret Service adapter. Its observer runs in a normal host-launched **AGENT**
panel and calls the frozen candidate's public `--browser-mcp` route. Private host
stamps, queues and cached snapshots are never substituted for MCP responses.
The prerequisite is API storage commit `79d32b9e9716b741ae03104d857416cecc2f6851`.
Its product has no API-status UI, map or `tailnets` CLI route. This tooling adds no
product adapter, credential editor or public mutation operation.

The [native tooling procedure](../../docs/testing/procedures/tailnet-api-native-fixture.md) defines
required results, native Device-panel evidence, GIF and cleanup. This fixture's
`state: passed` proves its synthetic setup/cleanup only. Public replies and native
observations require separate assessment; RPC errors and skipped tests are not passes.

Requirements are Linux x86_64, Python 3, healthy Docker, a hydrated source checkout
and Rust. Install the OS runtime dependencies listed in the
[storage fixture](../tailnet-api-store-fixture/README.md), plus host executables
`/usr/bin/Xvfb`, `/usr/bin/xkbcomp`, `/usr/bin/openbox` and `/usr/bin/x11vnc`.
Xvfb invokes xkbcomp to compile its keyboard map. `/usr/share/X11` and its `xkb`
data directory are required. The runner resolves the host XKB directory, requires
it below `/usr/share` with an actual `rules/evdev` file inside that resolved tree,
and mounts the resolved directory read-only at `/fixture/xkb`. This handles a host
`xkb` symlink that points outside `/usr/share/X11`. Xvfb uses explicit
`-xkbdir /fixture/xkb`; no writable `/var/lib/xkb` cache is added.
The runner checks every declared executable and required runtime directory before
freezing artifacts, building the test or creating a container. It mounts these
tools read-only with host runtime libraries and available font/Openbox/Vulkan data.
Inside the new container, the control creates `/tmp/.X11-unix` with mode `0700`
and refuses an existing directory or symlink. It starts Xvfb and requires the frozen
`horizon-device --target /fixture/state/target.json doctor` to report successful
X11 protocol readiness within a total five-second budget BEFORE constructing or
starting Secret Service or preparing credentials. A socket file alone is not ready.
The receipt validates the actual CLI ok/result envelope, exact target ID, geometry
revision/surface, dimensions and screenshot/pointer/keyboard capabilities. Nonsecret
`state/x11-readiness.json` and doctor logs stay private. Failure stops retained
owned Xvfb and expires the exact target without creating synthetic credentials.
The directory is private container tmpfs, never a host X11 mount. Check-only runs
use the same host preflight but start only Secret Service, without Xvfb or doctor.
The OS-only runtime image is the same pinned image as F05:

```sh
docker pull ghcr.io/peters/horizon-worker-base@sha256:6550aacb6c60187cd346a795beb3c8bffd998fe9b95ec74f5fb844cf78870bdc
```

Build the real production executables in the task checkout and own target:

```sh
cargo build --locked -p horizon-ui --features cloud-workspaces
cargo build --locked -p horizon-core --features cloud-workspaces --example cloud_deploy
cargo build --locked -p horizon-device --features cli --bin horizon-device
```

Freeze them outside the checkout. Record compiler/config/source inputs and hashes
in the root build manifest. Create a private JSON artifact manifest with exactly
`horizon`, `cloud_deploy` and `horizon_device` objects. Each has `path` (absolute)
and `sha256`. Link it to the root build manifest in the run evidence. A hash alone
does not establish that an executable came from the candidate; the operator must
compare the consumed source inputs before qualifying or reusing an artifact.

From the task checkout, use fresh private paths outside the repository:

```sh
python3 scripts/tailnet-api-status-fixture/run.py start --evidence-dir /tmp/horizon-api-native-first --manifest /tmp/horizon-api-native-artifacts.json --target-dir /tmp/horizon-api-native-target
```

The artifact manifest and build command are operator-owned inputs. Review the
candidate before building: this fixture does not sandbox arbitrary Rust build
scripts. The host treats container-writable state paths as untrusted. Bounded
no-follow reads and exclusive new output creation refuse candidate-planted
symlinks, hardlinks and nonregular entries without truncating another host file.
Source hashing/copying accepts only declared checkout inputs and rejects symlinks.
Docker operations are bounded; a launch timeout retains its unique name, labels
and daemon for exact reconciliation before reporting failure. Preserve unresolved
state rather than inferring absence.

The runner automatically builds/freezes the current ignored library test with
`cargo test -p horizon-cloud --lib --no-run --locked --message-format=json`.
It adds the chosen target argument, hashes the source before/after build and
refuses changes. It freezes the controls and executable copies, records exact
container/daemon/labels and starts one owned container. No provider configuration,
credentials, host home, runtime, X11 or bus socket is mounted. Docker has no network,
no capabilities, `no-new-privileges`, a read-only root, private state/tmp/run and a
30-minute fixture lifetime. A private non-activating bus and retained keyring serve
only this run. The existing `SecretService Fixture` is imported, not duplicated.

First qualify setup/cleanup separately with the same command plus `--check-only`
and another new evidence path. It explicitly executes `native-prepare`, `native-check` and `native-cleanup`
without starting X11 or Horizon. The check inserts an unknown synthetic catalog
ID and proves cleanup refuses before deleting any owned binding. It restores the
owned catalog, removes a known row and renames another before ledger cleanup. An ordinary ignored-test skip is
not this check. The native ledger permits names to change and known IDs to disappear;
it rejects unknown catalog IDs before deleting anything. Partial owned preparation
stays recorded; uncertain cleanup fails and preserves evidence.

For a native run, start the owned RFB relay using the returned ownership path:

```sh
python3 scripts/tailnet-api-status-fixture/vnc_proxy.py --ownership /tmp/horizon-api-native-first/ownership.json
```

Keep this process in an owned terminal. It binds a numeric host-loopback port and
writes `relay/vnc-address.json`. Each connection rechecks exact container ownership
and relays actual x11vnc bytes through `docker exec`; it supplies no rendered pixels
or input. It has at most two clients, bounded 64-KiB queues in each guest direction
and a 30-minute lifetime. A public `device_panel` uses that endpoint for the live
view and video. The viewer is read-only. For input, run the frozen standalone
`horizon-device` in the exact owned container against `/fixture/state/target.json`;
use doctor, screenshot and bounded act with fresh geometry. Set `NATIVE_CONTAINER_ID`
to the exact immutable ID returned by start, then use:

```sh
docker exec "$NATIVE_CONTAINER_ID" /candidate/horizon-device --target /fixture/state/target.json doctor
docker exec "$NATIVE_CONTAINER_ID" /candidate/horizon-device --target /fixture/state/target.json screenshot /fixture/state/native-input.png
docker exec -i "$NATIVE_CONTAINER_ID" /candidate/horizon-device --target /fixture/state/target.json act -
```

The last command reads the bounded action JSON from stdin. Use the public device
contract and fresh screenshot geometry; do not guess coordinates or replay an
indeterminate action. The target refers only to this container's `:71` X11 display
and expires when the control exits.

The candidate config contains a `kind: codex` panel with the Python observer as a
custom command. Horizon supplies its actor and host instance through the ordinary
agent spawn. Do not set either value yourself or launch the observer from a shell.
The observer verifies the frozen Horizon hash, initializes MCP, lists tools and
reads `cloud_list(operation=list)` and `cloud_companions`. Baseline mode requires
three legacy tailnet items with exactly id/name fields matching the synthetic
preparation. It does not require or claim an API status map or new API rows. An
empty `cloud_list` is expected. Timestamped raw public responses stay in
`state/mcp-evidence/rpc.jsonl`; the bootstrap summary derives from those responses,
not private host metadata. A normal native close requires this bootstrap proof.

The runner also exposes exact-owned controls:

```sh
python3 scripts/tailnet-api-status-fixture/run.py keyring-off --ownership /tmp/horizon-api-native-first/ownership.json
python3 scripts/tailnet-api-status-fixture/run.py keyring-on --ownership /tmp/horizon-api-native-first/ownership.json
```

Off/on signals affect only the retained service in that container. Baseline mode
uses ordinary saved-tailnet rows and the legacy public catalog. It makes no API
unavailable/ready UI or map assertion. Keep fixture receipts separate from public
MCP and native outcomes. The existing cloud_deploy executable is built/frozen for
artifact completeness; baseline never dispatches its absent tailnets route.

For the later status implementation only, start a new run with
`--expect-api-status`. This is a qualification switch, not a product feature flag:

```sh
python3 scripts/tailnet-api-status-fixture/run.py start --expect-api-status --evidence-dir /tmp/horizon-api-status-first --manifest /tmp/horizon-api-status-artifacts.json --target-dir /tmp/horizon-api-status-target
python3 scripts/tailnet-api-status-fixture/run.py cli --ownership /tmp/horizon-api-status-first/ownership.json
```

That mode requires the actual public credential map with initial ready
missing/personal_token/oauth kinds. Stored kind does not validate remote authority.
It preserves pending/unavailable distinctions
and refuses malformed or unknown-ID observations. Only this mode permits the new
compiled CLI route. The later status PR must invoke this mode and document native
API rows, CLI agreement, open-Settings completion and unavailable/restore checks.
API79 cannot qualify that outcome; an absent map times out without a pass. The
same baseline fixture source therefore remains an independent prerequisite.

Close the owned candidate normally through its native UI. The control then runs
ledger-bound cleanup, checks the service empty and stops retained children. Close
the public viewer, then explicitly request the exact owned relay to stop:

```sh
python3 scripts/tailnet-api-status-fixture/vnc_proxy.py --ownership /tmp/horizon-api-native-first/ownership.json --stop
```

This validates the same healthy daemon, immutable container and labels before
creating a private exclusive stop request bound to that ownership and per-relay
nonce. A request for another container/daemon/nonce is refused. The actual relay
polls the request, stops and joins only its retained Docker-exec children, closes
its listener and removes `vnc-address.json`, then writes its actual stopped result.
The request receipt is not cleanup proof. Await the original relay execution's
normal exit code zero and its stopped result before container cleanup; allow its
bounded child joins to finish. Do not interrupt its shell wrapper, kill by a host
PID/process name, or substitute a private product queue. This is task-fixture
control only, not a Horizon operation. Then reconcile exact container and files:

```sh
python3 scripts/tailnet-api-status-fixture/run.py cleanup --ownership /tmp/horizon-api-native-first/ownership.json
```

Cleanup requires normal container exit, an empty owned service, stopped children
and stopped relay. It verifies the same healthy Docker daemon and exact container
absence before deleting only owned home/runtime/credential files and the password.
Frozen artifacts, logs, public replies and ownership records remain private.
On uncertainty preserve the record and state; no broad prune, process-name kill,
host-bus fallback or unrelated-container cleanup is permitted. Failed starts and
forced timeouts do not qualify; reconcile only the exact recorded resources.

Offline fixture checks require no Docker, socket or desktop:

```sh
python3 -B -m unittest discover -s scripts/tailnet-api-status-fixture/tests -v
```

These checks are not native, Secret Service, production compilation or public
host-routing qualification. Repeat the actual required lanes on the first and
final candidate with valid input comparisons for any reuse.
