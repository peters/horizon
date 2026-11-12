# Local device smoke

The public fixture demonstrates Horizon using only disposable terminals and
synthetic text. Keep application-specific scenarios, screenshots, recordings,
identifiers and operational details outside the repository and public review.
This workflow is Linux-only; native macOS qualification is deferred.

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

Use a new state path if it already exists. Pass `--reuse-state` to start a run
in the private home that the earlier run at that state path left, and to keep
that home when the run stops. Use it only for a state path that this task owns.

The isolated desktop is 1600 by 1000 pixels. `--screen WIDTHxHEIGHT`, such as
`--screen 2800x1000`, gives it another size, for a procedure that needs a wide
or a tall screen.

The fixture runs the candidate in a `bwrap` sandbox. The sandbox binds the
private home over the home of the person, so a checkout under that home does not
exist inside it, and it binds the state folder at its own path. A program that a
panel of the fixture runs therefore lies in the state folder or in a path that
the sandbox binds. The fixture copies each program of its own folder into the
state folder and never writes a link into the checkout. The programs are
`card_agent.py` of `--worktree-card`, `agent_attention_standin.py` of
`--attention-agent`, and each program of that folder that a `--panels` entry
names. The fixture replaces the path of the checkout in that entry with the path
of the copy, so a procedure writes the path of its checkout.

### Restart the candidate in the same state

A procedure that quits the candidate and starts it again needs `--reuse-state`
in both runs. The first run leaves its private home, so sessions
and saved records stay. The second run starts in that home and shows the boards
of the earlier run.

1. Start the fixture with `--saved-session`, `--reuse-state` and a state path that
   this task owns. An ephemeral run writes no session file, so a later run has no
   board to open.
2. Quit the candidate with the close button of the window manager.
   Result: The candidate exits. The harness also exits, and it keeps the private
   home.
3. Start the fixture again with the same state path, `--saved-session` and
   `--reuse-state`.
   Result: The harness uses a new display and viewer endpoint, and the candidate
   reads the private home of the earlier run.
4. Open the saved board through **Menu** › **Sessions** when the candidate starts
   in a new session.

> **CAUTION:** A reused private home holds the credentials, sessions and records
> of the earlier run. Delete the state path when the task ends.

Pass `--firefox-system-access` only
with the Firefox disclosure procedure. The flag sets `browser.backend` to
`firefox` and `browser.firefox_system_access` to true. It also opens the
Google sign-in page and the X login page. Other runs leave that field false.
Mozilla documents the matching geckodriver flag as full system access for
clients that reach the fixture driver port.

The harness atomically allocates an
unused Xvfb display of the `--screen` size, disables MIT-SHM and starts its own
window manager.
It launches an ephemeral Horizon with a heartbeat terminal and a plain Bash input
terminal. Pass `--saved-session` to keep the session of the fixture in its private
home instead.
The flag changes no file outside the fixture. A filesystem namespace masks the
developer's home with private state
for this child; it does not change the real HOME value or existing sessions.
The harness removes `HORIZON_HOME` from the child environment, so the fixture
keeps its state in `.horizon` of the masked home.
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

The harness starts its X server on the host, not inside that namespace, so the
host `/tmp` must accept writes: Xvfb keeps its keymap files there. A `/tmp` that
is a tmpfs with a per-user quota can refuse the write even when `df` reports
free space, because every process of the same user counts against one quota.
Before it creates its state, the harness writes and removes one probe file in
`/tmp` and stops with a message that names `/tmp` when that write fails. During
a run, keep the state directory and any frozen binary on a filesystem with
space. Remove the binary copies of earlier runs; the harness does not reuse
them.

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

`--presets FILE` sets the New Workspace presets of the fixture from a JSON list
with the fields of `presets:` in the configuration, for example two presets of
one agent with different `args`. Leave it out to keep the default menu of
Horizon. The fixture replaces only the menu: it does not add a login, a
credential or another program.

The fixture checks the file before it starts Horizon. A preset without a `name`
or `kind` stops the fixture with an error. So does a `resume` value that Horizon
does not read: it accepts `fresh`, `last`, or the map form
`{"session": {"session_id": "<id>"}}`, and not the plain string `session`.
Horizon itself starts with its own defaults when a configuration does not load,
and an unchecked file would then show an empty board instead of the presets of
the test.

`--resume-session` starts the candidate with no session argument, so it loads the
board that an earlier run saved in the private home. Give it with `--reuse-state`
and without `--saved-session`; it is how a procedure quits the candidate and starts
it again on the same board. The fixture refuses the combination that names both.

`--plain-start` gives the binary no argument at all, so it starts in the private
home of the state folder and reads and writes the default paths of that home:
`<private home>/.horizon/settings.sqlite`, its sessions and its secrets file. Give
it with `--reuse-state`, so a first run with the binary of an older release makes
the home and a second run with the candidate starts on that same home. That is how
the upgrade procedure shows an upgrade from a release. A plain start refuses every
option that writes the settings or the session of the fixture, such as
`--presets`, `--panels`, `--theme`, `--browser-host`, `--server-target`,
`--named-secret` and `--saved-session`, because the home of the fixture holds its
own settings and its own session in that run.

`--worktree-card` makes one throwaway repository at `<state>/data/repo`, writes
the presets `Task shell` (a shell) and `Page` (a browser page), and starts the
scripted card agent with the scenario `worktree`. The agent makes one task
worktree beside the repository, opens the preset panel there with `open_panel`,
and draws one card for the worktrees of that repository: a row for each worktree,
the changes of the selected one, and the buttons `Refresh`, `Show changes`,
`Merge` and `Remove`. A merge and a remove need a second press, and they work on
real git and need no model and no login. A plain start refuses it. The procedure
is [the worktree card procedure](../../docs/testing/procedures/worktree-card.md).

`--panels FILE` opens extra workspace panels at start from a JSON list of
terminal entries, so a procedure that needs an agent panel, a scripted client or
another command keeps the same viewer and needs no click. An entry without a
`position` is placed in a column to the right of every panel the fixture places,
including the board that `--firefox-system-access` replaces. The fixture checks
the file before it starts Horizon, the same way `TerminalConfig` reads it: a
panel needs a string `name`, a `kind` from the panel kinds, string `command`,
`cwd` and `args` items, two numbers for `position` and `size`, and a `resume`
value Horizon reads. Any other value stops the fixture with an error.

### An agent that shows the attention states

`scripts/device-smoke/agent_attention_standin.py` is the program of an agent
panel that needs no model and no login. Horizon starts it as the agent of a
Claude panel and attaches the work ledger of that panel, and the stand-in sends
the lifecycle events of a turn through the same entry point that the installed
hooks use. The sidebar then shows the four attention states: `Needs you`, `Done`,
`Working` and `Background`.

```bash
scripts/device-smoke/serve.py --horizon target/debug/horizon \
  --state /var/tmp/attention-1 --native-view --attention-agent
```

`--attention-agent [SECONDS]` puts a `claude` program that is the stand-in itself
first on the `PATH` of the candidate, and adds the panel entry of
`scripts/device-smoke/attention_panel.json` to the board. The entry asks for work
continuation, which is what makes Horizon attach the ledger of the panel. The
stand-in sends the events of its sequence every SECONDS, which is six seconds
when the option names none. Type `start`, `ask`, `answer`, `stop`, `background` or
`failed` in the panel and press Enter to send one event, also while the timer
runs, or give `--attention-agent 0` to wait for the typed words alone. Type
`after SECONDS WORD` to send one event after that delay, at most 60 seconds, so
that the person can give the focus of the desktop to another window before the
event arrives. `--attention-agent-count N` opens N panels of the stand-in, 1 to
4, in one row above the panel of the notifications, so that a change of two
panels at the same time shows the count of a notification. `--attention-agent-command`
adds one panel of the second shape of the entry,
`scripts/device-smoke/attention_panel_command.json`: a preset whose own command
is the stand-in and whose work continuation is off. Horizon attaches the ledger
of that panel too, so the sidebar shows its state, and the panel offers no resume
and shows no card. Both options open at most four panels together. The panels
share their workspace with the panels of the default board and cover the first
one. The procedure is
[the agent attention procedure](../../docs/testing/procedures/agent-attention.md).

### A desktop notification that a person can see

The fixture records the desktop notifications of the candidate, so a procedure
proves the line that a person sees without a notification on the desktop of the
developer.

```bash
scripts/device-smoke/serve.py --horizon target/debug/horizon \
  --state /var/tmp/notifications-1 --native-view --attention-agent 0 \
  --notifications-log --plain-window
```

`--notifications-log` puts the folder of a stand-in `notify-send` first on the
`PATH` of the candidate. It appends the arguments of one call as one line of JSON
to `notifications.log` of the state folder, and it is a copy in that folder: a
program that a panel of the fixture runs lies in the state folder or in a path
that the sandbox binds. The panel `Notifications` follows that file with
`tail -f`, and it lies below the stand-in panels. `--plain-window` opens a second window of the isolated desktop, which
takes the focus of the desktop away from Horizon: `xterm` when this computer has
it, and `xmessage` or a window of Tk otherwise. The procedure is
[the desktop notification procedure](../../docs/testing/procedures/desktop-notifications.md).

### A scripted card agent

A procedure that needs an agent which draws cards, answers the clicks of the
person and quits needs no AI model. `scripts/device-smoke/card_agent.py` is that
agent: it starts `<candidate> --browser-mcp` as its child, draws the cards of one
scenario and answers the next click of the person. Its own tests run with the
other tests of this fixture.

Write this list to a file and give it with `--panels`, so the fixture starts an
agent panel that runs the script:

```json
[
  {"name": "Scripted card agent", "kind": "claude", "command": "/bin/sh",
   "args": ["-c", "exec /usr/bin/python3 -u /<repo>/scripts/device-smoke/card_agent.py \\\n  --mcp-command \"<candidate> --browser-mcp\" --scenario all+tall \"$@\"",
            "card-agent"],
   "cwd": "<work>", "position": [40, 620], "size": [330, 440]}
]
```

The wrapper is necessary. Horizon starts an agent panel with its own arguments
around the arguments of the panel: `--session-id <id>` in front of them and
`--plugin-dir <path>` after them. The script ignores every argument that it does
not know, but `python3` refuses `--session-id`. `/bin/sh -c` reads the script
from its first argument and gives every argument of Horizon to the script as
`"$@"`, and `exec` hands the terminal to the script itself. Put a variable that
one panel needs, such as `CARD_AGENT_SSH_HOST`, in front of `exec` in the same
wrapper. The fixture copies the script into the state folder and gives the
copy to the panel, so the folder of the checkout needs no bind.

`--scenario` selects the cards: `machine`, `choose`, `browser`, `tall`, `all`
(the first three), `all+tall` (all four) and `intruder`, which draws no card and
tries to change the cards of another agent. The scenario `worktree` reads
`CARD_AGENT_REPO`, `CARD_AGENT_STATE` and `CARD_AGENT_PRESET`, starts one task
worktree in that repository, opens the preset panel there, prints the three
refusals of `open_panel`, and draws the card of the worktrees. Every git command
of that scenario runs with `-C` and an argument list, never through a shell, and
it removes only a folder under `CARD_AGENT_STATE`. `--listen continuous` keeps one wait
on every card; `--listen once` reads the wake line first. `--click-timeout` sets
the seconds of one wait. The Connect button of the machine card opens an SSH
terminal for the host in `CARD_AGENT_SSH_HOST` and the one line in
`CARD_AGENT_SSH_COMMAND`.

### A scripted client of the device panel tool

A procedure that needs a viewer of a frame source program of the fixture cannot
use the `device_panel` tool of the reader's own Horizon: that tool belongs to the
Horizon of the reader, not to the candidate in the fixture. `--frame-source-agent`
adds the panel of a scripted client of the candidate:

```sh
python3 scripts/device-smoke/serve.py --horizon target/debug/horizon \
  --state "${TMPDIR:-/var/tmp}/frame-source-run" --native-view --frame-source-agent \
  --frame-source "standin=/usr/bin/python3 {standin} --width 640 --height 360 --rate 10"
```

The panel runs `scripts/device-smoke/frame_source_agent.py`, which starts
`<candidate> --browser-mcp` as its child and calls the one tool `device_panel`.
Type one word on the terminal of the panel, one operation per line:

| Word | Operation |
|------|-----------|
| `create [name] [target]` | create a viewer of that program of the settings |
| `inspect` | inspect the viewer of this run |
| `watch [n]` | inspect once a second, `n` times, default three |
| `hide` / `show` | change the visibility of the viewer |
| `close` | close the viewer |
| `list` | list the viewers of this workspace |

Each answer is one line that starts with `frame-source-agent: `, with the kind of
the source and the evidence members of the answer: `connection`, `image_received`,
`image_displayed`, `frame_sequence`, `received_frame_sequence`, `desktop_size`,
`frames_dropped` and `age_ms`. A refusal is one line with its code and its
message. The fixture copies the script into the state folder, as it does with
every program of a panel.

`--frame-source` writes one program of the `frame_sources` section of the
settings and never starts it. A line that holds `{standin}` runs the copy of
`scripts/device-smoke/frame_source_standin.py` in the state folder, because the
sandbox binds the private home over the home of the person and a checkout under
that home is not inside it.

To see the same board again after a restart, start the fixture with
`--saved-session --reuse-state`, and start it again with `--reuse-state
--resume-session`, as [the restart recipe](#restart-the-candidate-in-the-same-state)
describes. The restored agent panel starts the script again, which draws a second
set of cards beside the panel; the cards of the earlier run stay on the board.

`--ssh-config FILE` and `--ssh-identity FILE` give the app in the fixture an
SSH configuration and an identity, so a feature that names a host over SSH can
reach it. Give the two flags together, or give neither one:

```sh
python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
  --native-view --state /tmp/horizon-device-target \
  --ssh-config /tmp/fixture-ssh/config --ssh-identity /tmp/fixture-ssh/fixture_key
```

The app reads `ssh` from its private home, and that home starts empty: it holds
no alias, no identity and no known host. The fixture copies the configuration to
`~/.ssh/config` and the identity to `~/.ssh/<file name of the identity>` of that
home. The folder mode is 0700 and the file mode is 0600, because `ssh` refuses a
configuration that another user can read. The identity keeps its own file name,
so the configuration names it:

```
Host browser-vm
  HostName 127.0.0.1
  Port 2225
  User runner
  IdentityFile ~/.ssh/fixture_key
  StrictHostKeyChecking accept-new
```

The fixture stops with an error before it creates its state when it gets one of
the two flags alone, when a path is not a regular file, or when the identity is
named `config`. Without the two flags the fixture seeds nothing, and a board
that needs no SSH host stays as it is. `ssh` writes `known_hosts` into the same
private home, and the fixture removes that file with the rest of its state.

The fixture never reads `~/.ssh` of the developer. Give an explicit path for
each file, and keep the source copy in the evidence directory of the task. A
run must not pass because a configuration of this computer happened to hold the
host. The app log states the words of `ssh` when a host is not reachable, so a
refused host names its reason instead of an empty one.

`--browser-host TARGET` names a browser host in the fixture settings, so a
browser panel that the fixture creates opens on that host instead of the local
browser. `--browser-host-profile PROFILE` adds the profile of that host, and it
needs `--browser-host`. The target is one name that `ssh` resolves, such as an
SSH config alias; the fixture does not start the host. Leave both out to keep
the local browser.

`--server-target TARGET` names the Horizon server that the machine list of the
fixture follows, so two fixtures prove one machine list on two computers.
`--server-command COMMAND` adds the command of the link on that server, when the
default `horizon --server-link` does not fit, and it needs `--server-target`.
The fixture starts no server: start one with `horizon --server` in the server
home that the target reaches.

Two fixtures run at the same time. Each one chooses its own display through
Xvfb, binds every port at zero and keeps its own state, so give each run its own
`--state` folder and give the two runs different server homes or targets.

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
   advertising `reveal`, call it once only if the owned viewer has never
   presented an image. Hosts return `navigation_preserved` after the first
   non-discarded displayed image, even after reconnect or ownership transfer.
   Restored viewers also preserve saved navigation when past display history
   is unavailable; reconnect only acquires the transport.
   Continue testing and inspection; never close/recreate the viewer to bypass
   the guard. The first Reveal changes
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
   restart of the user's Horizon or automation of their desktop. A viewer that
   was displayed earlier in the connection (`last_displayed_age_millis` present
   under the same `connection_generation`) and now reports `not_rendered` with
   `outside_canvas` while uploads or reception keep advancing was navigated away
   from by the person: keep the interactive test and its recording running, do
   not pause the lane, and do not spend the reveal budget just to advance
   counters. A person can return to the viewer through the UI.

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
state. A run with `--reuse-state` keeps that state instead, so a later run can
start in it. Logs and evidence remain: the first run writes `<name>.log`, and a
later run in the same state writes `<name>.2.log`, `<name>.3.log` and so on, so no
run truncates the log of another. Normal application exit also completes
the harness; unexpected child failure fails it and triggers cleanup.

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
