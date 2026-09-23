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

For the required native workflow, prerequisites are Xvfb, Openbox, x11vnc,
bubblewrap (`bwrap`), `dbus-daemon` and Python 3. `xinput` is needed for cancellation checks.
The device library build needs libxkbcommon. The scripts do not install
prerequisites. An optional `--tools ROOT` accepts unpacked Debian tools under
`ROOT/usr`.

## Horizon inside a native VNC Device panel

Start an isolated target desktop in a terminal and leave the fixture running:

```sh
python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
  --native-view --state /tmp/horizon-device-target
```

Use a new state path if it already exists. The harness atomically allocates an
unused 1600×1000 Xvfb display, disables MIT-SHM and starts its own window manager.
It launches an ephemeral Horizon with a heartbeat terminal and a plain Bash input
terminal. A filesystem namespace masks the developer's home with private state
for this child; it does not change the real HOME value or existing sessions.
The same namespace gives namespaced processes a private writable `/tmp` and a
session bus at the standard `/run/user/<uid>/bus` path, backed by fixture
runtime, so native applications and `dbus-run-session` can bind sockets. Host
`/tmp/.X11-unix` is re-bound read-only when bwrap starts, after Xvfb has created
it, so the fixture X server stays reachable even if that directory was missing
when the harness prepared the namespace. `--horizon` and `--tools` paths are
re-bound after that `/tmp` overlay so a frozen binary under
`/tmp/horizon-smoke-bin.*` remains executable. When AppArmor's query file
exists, only that file is bound writable so policy checks reach the host LSM;
policy load and remove stay read-only, host enforcement is unchanged, and the
developer's session bus is not used. The fixture starts and stops its own
`dbus-daemon`. `LIBGL_ALWAYS_SOFTWARE` does not disable Vulkan, so the debug
app may use the available GPU.

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

With `--native-view`, the printed manifest and `lab.json` contain `vnc_address`,
the display and owned process IDs; `viewer_url` is null. This mode starts no
noVNC assets, websockify or browser. The harness still supports its legacy web
mode, which additionally requires noVNC assets and Python websockify, but always
pass `--native-view` for interactive testing under `AGENTS.md`. Its legacy help
and heartbeat labels still mention noVNC even in native mode; those labels do
not establish which viewer is running. Verify the flag, manifest and owned
processes when collecting evidence.

Use the public `device_panel` tool exposed by Horizon's MCP server to create a
**visible native Device panel in the calling agent's current workspace**. The
agent must be launched inside a supporting Horizon host. Read the endpoint from
this fixture's `lab.json`; the following port is only an example:

```json
{"operation":"list"}
{"operation":"create","endpoint":"127.0.0.1:5900"}
{"operation":"inspect","panel_id":"<returned id>"}
```

Check `connection: "connected"`, `image_received`, `image_displayed`, and an
advancing `frame_sequence` during changing target output. Creation returns
immediately; `visible` alone does not prove that an image is on screen. On
`host_timeout`, list before retrying a mutation. Restored viewers need an explicit
`reconnect`; only operate on task-owned panels. See the
[native lifecycle contract](../../crates/horizon-browser-mcp/README.md#native-device-viewer-lifecycle).

If the tool, host support or visible native panel is unavailable, report the
blocked lane. Do not open noVNC, modify private runtime files, restart an active
session or automate the developer's desktop. Screenshots and recordings are
supporting evidence, not a replacement for the user's live panel. Device CLI/MCP
controls the isolated target; the panel is a read-only viewer.

### Automatic viewer health check

The agent performs this procedure through the public `device_panel` tool. Never
ask a person whether the panel is visible or updating. Preserve the observations,
decisions and attempted operations with the task's private smoke evidence.

1. Inspect the exact task-owned viewer at least three times, two seconds apart.
   Record UTC timestamps, panel identity, connection, visibility, ownership,
   `image_received`, `image_displayed` and `frame_sequence`. Record whether target
   output is independently known to be changing, static or unknown. A fixture's
   existing heartbeat is suitable; do not drive an unverified interactive UI just
   to manufacture changing content.
2. If hidden, request `visibility(true)` once. If stopped or disconnected,
   reconnect once, provided the task owns the viewer. An unowned/restored viewer
   needs an explicit task ownership record before acquisition. Never mutate a
   viewer owned by another agent. If a mutation times out, list/inspect before
   deciding whether any further action is safe.
3. Repeat the bounded observations after recovery. Never compare frame counters
   across reconnects. A connected, visible panel that remains unpresented is a
   presentation problem, not evidence that another reconnect will help. On hosts
   advertising `reveal`, call it once for the owned viewer. Reveal changes
   canvas presentation, preserves keyboard focus and does not reconnect; new
   hosts answer once the viewer was drawn after the reveal, or after at most
   three seconds with `presentation` and `diagnostics.host.exclusion` naming the
   blocked reason. A host running no UI frames answers when that bound expires.
   Inspect again for advancing frames. Older hosts may lack this
   operation; record `presentation_unverified` and the unsupported capability.
   Do not enter a reconnect/recreate loop or request manual confirmation.
   New hosts report `diagnostics`: the connection generation, decoded-frame
   counter and age, sampling pause and a presentation reason. `not_rendered`
   plus `sampling_paused` explains an offscreen viewer; it is not a transport
   failure. `awaiting_frame` and `clipped` identify different presentation gaps.
   A decoded frame is not an uploaded or displayed image. Missing diagnostics
   mean an older host, not a healthy or failed connection.
   `last_uploaded_age_millis` independently dates the last received-frame texture
   submission, matching `frame_sequence`; it is not a GPU completion timestamp.
   Repainting or cropping retained pixels does not refresh this age. Reconnect
   clears it even if an old texture remains visible. Older hosts can omit it.
   When `diagnostics.host` is available, retain its viewport, exclusion,
   render-time `canvas`, `canvas_after_pass`, view revision and Reveal counters.
   A changed camera can explain why the next pass differs without attributing
   navigation to an actor. An unapplied Reveal request is distinct from a later
   camera change. Host evidence is a UI-pass observation: reject discarded
   passes as presentation proof, and do not compare viewport-local pass numbers
   across windows. Missing detached canvas is intentionally unknown, not root
   geometry. Continue to require actual displayed advancing frames.
4. Report the strongest evidence actually observed. Connected plus received plus
   displayed with an advancing sequence during known changing output establishes
   live presentation for that observation window. Displayed static content
   establishes presentation only. Unchanged counters with static/unknown output
   are inconclusive about transport freshness. A known-changing target without
   advancement is `freshness_unverified`; a failed connection is
   `connection_unavailable`; unavailable MCP/host support is `host_unavailable`.
5. Keep a single recovery budget for the incident, including resumed agent turns:
   at most one visibility request, one reveal and one justified reconnect, followed by a
   recorded outcome. Further recovery requires new diagnostic evidence. A
   blocked native lane does not block headless checks, and it never authorizes a
   restart of the user's Horizon or automation of their desktop.

`image_received` and `frame_sequence` retain their original texture-upload
meaning for compatibility. The separate decoded-frame counter is available only
when the host returns diagnostics. `image_displayed` refers to the last completed
UI frame. Validate the running host's advertised capabilities; a new client or
updated instructions do not upgrade an older running host.

### Exercise the isolated target

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
  --native-view --device-address "$device_vnc_address" --state /tmp/horizon-device-viewer
```

Create a native Device panel in the current user's Horizon workspace pointing
at the second fixture's `vnc_address`, and verify its live image. The test chain is:

`current Horizon → native Device panel → isolated Horizon → native Device panel → isolated Horizon target`

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

Both fixtures use `--native-view`. x11vnc remains loopback-only and read-only.
The viewer fixture must itself be watched live in the current user's native
Device panel; inspecting a screenshot of that fixture is not enough.

## Video evidence and cleanup

For visible feature additions or behavior changes, record a short clip directly
from the task-owned isolated desktop. The native panel does not expose video
recording, and `browser_video` records browser pages only. Do not start noVNC to
obtain a recording.

1. Select a native recorder explicitly scoped to the fixture's display and
   private evidence directory. Verify it can capture that display before relying
   on it. This fixture disables MIT-SHM; do not assume any particular X11 recorder
   works, or that process liveness proves it is capturing frames.
2. Start before the interaction, perform the synthetic flow through device
   CLI/MCP, verify the application result, then stop and finalize the recording.
3. Play back or decode representative frames before, during and after the flow.
   Check that changes and movement are visible. Record the candidate hash,
   scenario, duration and available frame/drop statistics with the evidence.
4. If recording is unavailable, stalls or has no usable frames, report the
   recording lane as blocked. Screenshots are still required after launch and
   resize/fit but cannot replace motion evidence.

Keep evidence private until publication is authorized. Only generic Horizon
fixtures and synthetic content are eligible for a public demonstration; inspect
the actual frames before any separately authorized publication.

Close the test application normally and confirm it exited, then close its
owned viewer with `device_panel` operation `close`. Closing the viewer only
releases the VNC connection; it does not stop the fixture. Ctrl-C stops only the
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
