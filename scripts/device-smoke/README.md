# Local device smoke

The public fixture demonstrates Horizon using only disposable terminals and
synthetic text. Keep application-specific scenarios, screenshots, recordings,
identifiers and operational details outside the repository and public review.
This workflow is Linux-only; native macOS/Windows qualification is deferred.

Build from the intended worktree using its own Cargo target directory. Do not
build different worktrees into the same target directory during qualification:
stale local dependency artifacts can make a candidate differ from its source.
Finish both builds, then freeze the executables before starting a fixture:

```sh
export CARGO_TARGET_DIR="$PWD/target/device-smoke"
cargo build -p horizon-device --features cli
cargo build -p horizon-ui --bin horizon
smoke_bin=$(mktemp -d /tmp/horizon-smoke-bin.XXXXXX)
cp "$CARGO_TARGET_DIR/debug/horizon" "$smoke_bin/horizon"
cp "$CARGO_TARGET_DIR/debug/horizon-device" "$smoke_bin/horizon-device"
sha256sum "$smoke_bin/horizon" "$smoke_bin/horizon-device" > "$smoke_bin/SHA256SUMS"
```

Reuse the same `smoke_bin` directory in every terminal used for this run.
Do not overwrite the frozen copies while their fixtures are alive.

Prerequisites are Xvfb, Openbox, x11vnc, bubblewrap (`bwrap`), `dbus-daemon`,
noVNC and Python 3 with websockify. `xinput` is needed for cancellation checks.
The device library build needs libxkbcommon. The scripts do not install
prerequisites. An optional `--tools ROOT` accepts unpacked Debian tools under
`ROOT/usr`.

## Horizon inside a noVNC browser panel

Start an isolated target desktop in a terminal and leave the fixture running:

```sh
python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
  --state /tmp/horizon-device-target
```

Use a new state path if it already exists. The harness atomically allocates an
unused 1600×1000 Xvfb display, disables MIT-SHM and starts its own window manager.
It launches an ephemeral Horizon with a heartbeat terminal and a plain Bash input
terminal. A filesystem namespace masks the developer's home with private state
for this child; it does not change the real HOME value or existing sessions.
The same namespace gives namespaced processes a private writable `/tmp` and a
session bus at the standard `/run/user/<uid>/bus` path, backed by fixture
runtime, so native applications and `dbus-run-session` can bind sockets. Host
`/tmp/.X11-unix` is re-bound read-only so the fixture X server stays reachable.
`--horizon` and `--tools` paths are re-bound after that `/tmp` overlay so a
frozen binary under `/tmp/horizon-smoke-bin.*` remains executable.
When AppArmor's query file exists, only that file is bound writable so policy
checks reach the host LSM; policy load and remove stay read-only, host
enforcement is unchanged, and the developer's session bus is not used. The
fixture starts and stops its own `dbus-daemon`. `LIBGL_ALWAYS_SOFTWARE` does
not disable Vulkan, so the debug app may use the available GPU.

Before accepting smoke results, find the actual Horizon child within this
fixture's owned process tree. The recorded launcher PID may refer to `bwrap`.
On Linux, verify `/proc/<actual-horizon-pid>/exe` points at the frozen executable
and compare `sha256sum /proc/<actual-horizon-pid>/exe` with `SHA256SUMS`. Record
the child PID and hash with the evidence. Do not infer the running version from
the current build output path, process name, or successful build alone.

Independent application fixtures may run in parallel when requested. Give each
its own state directory, private config/home, display, device target and viewer
ports; keep an explicit owner and PID-to-target mapping. This harness allocates
its own display and ports. It launches Horizon only; other applications require
an equivalent isolated launcher. Never share targets or geometry between agents.

The printed manifest and `lab.json` contain `viewer_url`, `vnc_address`, the
display and owned process IDs. Open `viewer_url` in a separate Horizon browser
panel through the horizon-browser skill and its public MCP tools. The outer
browser is only the viewer; device CLI/MCP controls the isolated native desktop.
Never automate or record the developer's desktop as a fallback.

Use the target configuration to observe the disposable desktop:

```sh
python3 scripts/device-smoke/client.py --binary "$smoke_bin/horizon-device" \
  --target /tmp/horizon-device-target/target.json --mode cli \
  --output /tmp/horizon-device-observation
```

Inspect `00.png`, identify the input terminal and focus it with an action using
fresh screenshot geometry. Run the same synthetic flow through CLI and MCP:
enter `echo DeviceSmoke_ÆØÅ`, press Enter, and verify every character and the
printed output. `client.py` accepts either `--actions` containing a JSON action
array or `--actions-file FILE`; it captures fresh geometry before each action,
then saves screenshots and `report.json`. Reobserve after moving or resizing a
window instead of reusing old coordinates.

Exercise click, drag, scroll and modifier release against this disposable desktop:

```sh
HORIZON_DEVICE_TEST_TARGET=/tmp/horizon-device-target/target.json \
  cargo test -p horizon-device --features cli --test x11_live -- --ignored
python3 scripts/device-smoke/edges.py --binary "$smoke_bin/horizon-device" \
  --target /tmp/horizon-device-target/target.json
```

The native test requires a suitable disposable input window and changes its
focus/input state. Follow its preconditions; do not point it at an active desktop.

## Native Device panel inside the isolated fixture

The product viewer is Rust TCP → patched `vnc-rs` → egui. It does not need noVNC,
websockify or a browser engine to display VNC. The target still needs a VNC server.
This MVP accepts explicit numeric loopback addresses and a nonzero port, with no
credentials. It does not create tunnels or manage remote hosts.

Keep the target fixture running. Start a second isolated Horizon whose Device
panel points at that target's direct VNC endpoint:

```sh
device_vnc_address=$(python3 -c 'import json; print(json.load(open("/tmp/horizon-device-target/lab.json"))["vnc_address"])')
python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
  --device-address "$device_vnc_address" --state /tmp/horizon-device-viewer
```

Open the second fixture's `viewer_url` through public Horizon browser tools.
The test chain is:

`current Horizon → browser/noVNC → isolated Horizon → native Device panel → isolated Horizon target`

The generated private config uses `kind: device` and the existing `command` field
for the VNC endpoint. It does not modify default presets or the developer's
configuration. Restored Device panels stay stopped until **Reconnect** is
selected, because a saved local port may have been reused by another server.

Use the viewer fixture's `target.json` for Fit, panel resize, detach, fullscreen
and Reconnect actions. Use the target fixture's `target.json` for terminal input.
Each desktop has its own coordinates and geometry revision. Clicking or typing
on the native Device image must not forward input to its target.

Acceptance checks:

- Observe the heartbeat and synthetic terminal input in the native panel.
- Resize/Fit, detach/reattach and fullscreen the viewer; confirm target geometry
  is unchanged and repaint continues in the correct viewport.
- Confirm the image remains read-only using both target pixels and pointer state.
- Exercise manual reconnect, task-owned VNC failure/recovery and stopped restore.
- Close the Device panel and verify its connection closes while the target stays
  alive. Close the isolated Horizon normally and verify its harness succeeds.
- Stop each remaining owned fixture, verify its children exited and its target
  config expired, and retain only approved evidence.

`--native-view` is available when a target fixture only needs its direct VNC
server. It sets `viewer_url` to null and skips noVNC assets/websockify. Do not use
it for the outer Horizon desktop that must be observed through noVNC. x11vnc is
loopback-only and read-only in both modes.

## Video evidence and cleanup

For visible feature additions or behavior changes, record a short clip through
Horizon's public `browser_video` MCP tool:

1. Check `video_capture.supported` on the outer noVNC panel.
2. Start with its `panel_id`, `operation: "start"`, `fps: 5`,
   `compression_level: 0`, `quality: 70`, and `max_width: 1280`. Increase resolution
   when fine text is an acceptance criterion.
3. Perform the synthetic feature flow through device CLI/MCP. Wait for the
   verified result, then stop the recording.
4. Copy the finalized WebM into the private evidence directory **before closing
   the browser panel**; closing removes its profile and capture exports.
5. Play back or decode representative frames before and after the interaction.
   Retain the candidate binary hash, scenario, duration and encoded/dropped/repeated
   frame counts. A recording's existence is not proof that it captured the flow.

No additional recording service is needed. Recording may drop frames and is not
a VNC frame-rate benchmark. Only generic Horizon fixtures and synthetic content
are eligible for a public demonstration; review the actual frames before any
separately authorized publication.

Closing/reloading the browser leaves the fixture running. Ctrl-C stops only the
harness's owned children, expires `target.json` and removes private application
state. Logs and evidence remain. Normal application exit also completes the
harness; unexpected child failure fails it and triggers cleanup.

## Agent registration

```sh
python3 scripts/device-smoke/register.py --binary "$smoke_bin/horizon-device" \
  --lab /tmp/horizon-device-viewer --project /tmp/horizon-device-agent
```

Registration writes new project-local CLI/MCP configurations and the device skill;
it refuses to replace existing files. `--target FILE` also supports control
without a viewer. Start a fresh agent in the project; existing agents are never
restarted or hot-reloaded. Registration neither creates a native panel nor grants
additional control authorization. The target expires when its fixture stops.
