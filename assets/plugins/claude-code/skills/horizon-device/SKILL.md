---
name: horizon-device
description: Manage Horizon native VNC Device panels and drive isolated local desktops for simulators and native application tests through device_panel and the horizon-device CLI/MCP. Browser pages use horizon-browser.
---

# Horizon native VNC and device control

Use this skill for Horizon VNC Device panels, simulators, and isolated native
application tests. Browser pages use the `horizon-browser` skill and `browser_*`
tools. Never drive a browser with device input.

## Native VNC Device panel

Always observe interactive native tests live through a **Horizon native VNC
Device panel in the user's current workspace**. Use a task-owned isolated
desktop and private application state. Do not use noVNC or a browser viewer, or
substitute screenshots or recordings for the live panel.

Use the public `device_panel` MCP tool for viewer lifecycle. Call
`operation: "list"` to discover panels in the caller's workspace. Create a
task-owned viewer with `operation: "create"` and the fixture's numeric loopback
`endpoint` (IP and nonzero port); retain its returned `panel_id`. This requires
a Horizon-launched agent and a supporting host. The tool does not provision a
desktop or forward input. The source checkout's `scripts/device-smoke/README.md`
describes the isolated fixture; launch it with `--native-view` and use its
`vnc_address`, not a browser URL.

A VNC server on another machine's loopback is reached with optional `ssh` in
create: `{ "host": "lab", "user": "deploy", "port": 2222 }` (user and port
optional). `endpoint` is then the address as seen from that SSH host, typically
`127.0.0.1:5900`. Horizon runs `ssh -W` with its own SSH configuration and
keys, with strict host-key checking pinned regardless of `ssh_config`, so the
host must already be trusted in `known_hosts` (open it over SSH once); never
pass credentials, key paths or ssh options. Labels may only contain
letters, digits, `.`, `_`, `-` (and `:` in an IPv6 host), at most 253
characters for the host and 64 for the user, and are refused otherwise, so
nothing reaches ssh as an option or a shell fragment. Inspect and list report `ssh` for
tunnelled viewers, and `connection_error` carries ssh's last lines when the
tunnel fails.

When known, include optional `identity` in create: `machine_name`, `hostname`,
`ip_addresses` (numeric IPv4/IPv6 list), and `tailscale_name`. These are labels
supplied by the session creator, not verified identity. Use details for the
actual target machine; never substitute the local tunnel endpoint or the
viewer's own hostname. Omit unknown fields. Inspect/list return these labels.

Creation returns immediately. Use `operation: "inspect"` and the returned id to
check `connection: "connected"`. For a visible, on-screen viewer, also verify
`image_received`, `image_displayed` and an advancing `frame_sequence` while target
output changes. `visible` is only a presentation setting; an image can be off
canvas or clipped. For hidden or off-canvas viewers, use an advancing
`received_frame_sequence` while target output changes to verify reception;
uploads and display may remain absent or unchanged. This counter tracks received
image updates independently of the uploaded-image `frame_sequence`. Background
reception does not satisfy the live-view requirement for interactive testing.
Both counters reset on reconnect and neither is a heartbeat: a stationary desktop
is not a connection failure. Older hosts may omit reception progress; do not
interpret a missing/default-zero counter as a failure. Set
`operation: "visibility", visible: true` for a hidden owned viewer, then verify
actual presentation. Never claim that a separate isolated viewer is visible to
the user merely because its screenshot is available.

Check this automatically; never ask a human to confirm visibility or advancing
frames. Save at least three timestamped public inspections, two seconds apart.
The current `frame_sequence` counts uploaded images, not independent transport
heartbeats. An unchanged sequence on a static or unknown target does not prove a
stalled connection. A displayed static image proves presentation only; changing
target output and sequence advancement are needed for live-motion evidence.

Recover only the task-owned viewer, with at most one visibility request when
hidden and one reconnect when stopped or disconnected. Inspect again after each
request; do not reconnect a healthy connection merely because its image is not
displayed. Do not close/recreate viewers in a loop. Retain the attempt budget
across retries for the same incident. A connected, visible, unpresented viewer
requires `operation: "reveal"` on hosts advertising it. Reveal an owned viewer
at most once; it changes the viewport without reconnecting and answers once the
host has drawn the viewer, or after at most three seconds with the presentation
reason and host exclusion that kept it off screen (a host running no UI frames
cannot draw it and answers when that bound expires). In the reveal answer,
`image_displayed` is true only for a draw after the reveal applied. A drawn image is not live-motion proof. When diagnostics are present, record connection
generation, decoded-frame sequence and age, sampling pause, last displayed age
and presentation reason. Current hosts keep reception active while hidden or off
canvas; older hosts may pause it. Neither `not_rendered` nor a legacy sampling
pause proves a transport failure; `awaiting_frame` differs from `clipped`. Decoded pixels alone do not
prove display. Older hosts may lack reveal or diagnostics: record
`presentation_unverified` and the missing capability instead of asking the user
to move or watch the panel. Continue non-interactive checks. Never restart the
user's Horizon to upgrade these capabilities without authorization.

`operation: "reconnect"` explicitly connects and acquires an unowned/restored
viewer; do not take another owner's panel. Restored viewers stay stopped until
reconnected. `operation: "close"` closes an owned viewer without terminating its
target. On `host_timeout`, list before retrying a mutation because it may have
completed. If the tool, supporting host or visible native viewer is unavailable,
report the blocked lane. Do not fall back to noVNC, edit private runtime files,
restart active sessions, or automate the developer's desktop.

## Isolated desktop input

Drive the isolated target with explicitly configured device CLI/MCP tools: use
`device_doctor`, then `device_screenshot`, then bounded `device_act` input with
fresh geometry. Observe the result after each action. A `dispatched` receipt
confirms input delivery, not application success. On stale geometry take another
screenshot. On indeterminate input observe before deciding whether another
action is appropriate; never replay blindly. The native Device panel is
read-only for agents: a person may turn its Interact toggle on in the UI, but
no tool operation can.

The local CLI has the same contract:
`horizon-device --target <private-target.json> doctor|screenshot|act|resize <JSON>`.
For screenshots an optional output path writes a new private file instead of
base64 JSON. Optional `--options JSON` (or `--options -` for stdin) accepts
`region: {x,y,width,height}`, `output: {width,height}`, `format: png|jpeg`, and
JPEG-only `quality: 1..100`. MCP screenshot accepts the same options directly.
Omitting options preserves full-resolution PNG; JPEG defaults to quality 85.
`act -` reads JSON from stdin. Use this to avoid putting entered text in shell
history. The MCP server uses `--target <file> mcp` and stays bound to that
configured target. Read `--help` if the executable/target was not supplied.

Action kinds: `click` (at, button), `drag` (from, to, duration_ms), `scroll` (at,
vertical_notches, horizontal_notches), `type` (text), `key` (key, modifiers).
Coordinates are original surface pixels. When a screenshot is cropped/scaled,
map image pixels through `source_region` and `image_dimensions` before input:
`origin + floor((pixel + 0.5) * source_size / image_size)` per axis. Keep the
returned original geometry unchanged. Touch and accessibility are not implemented.
Each `type` action accepts at most 256 Unicode scalars and 4096 UTF-8 bytes,
without NUL. Text input is paced to let the application consume X11 key mappings;
split longer text into bounded actions and verify the displayed result.
Only control the explicitly authorized display; no implicit desktop fallback.

The caller owns the isolated display, application startup and cleanup.
Independent fixtures need separate unused displays, private configuration/home,
expiring targets, loopback VNC ports and owned process trees. Bind each agent
to its exact target; do not share screenshot geometry across fixtures. Never
control the developer's desktop, change production configuration, or stop a
pre-existing application. When a target expires, stop; do not recreate it from
a saved display number.

Finish the intended build, copy executables to a new task-owned directory, and
record their hashes before launch. Keep build caches separate across source
checkouts during qualification. Verify the running application's executable/hash
against that frozen copy, following the actual child rather than a sandbox
launcher such as `bwrap`. Close only the owned candidate normally when replacing
it; a rebuild does not change a running process. Native View controls change
local rendering, not target desktop geometry or VNC compression. Device
screenshot crop/output options are separate controls.

For nested Device-panel tests, view the isolated Horizon containing that panel
through a native panel in the user's workspace; keep each target and its
geometry distinct.

For feature evidence, record directly from the task-owned isolated desktop using
a recorder explicitly scoped to its display. Native panels have no video API;
`browser_video` is for browser pages. Start before the flow, stop afterward and
inspect decoded frames. If recording is unavailable or stalls, report the blocked
recording lane; still images do not replace motion evidence. Client-side scaling
does not reduce VNC wire bandwidth. Keep application-specific workflows and
evidence private; public demonstrations use generic fixtures and synthetic
content only.

Close only task-owned viewers and application/display fixtures, then verify
children exited and target configuration expired. Horizon injects `device_panel`
on the browser MCP; this skill does not start a viewer or register the input
server. Callers must still supply the `horizon-device` executable and target for
input, or register `--target <file> mcp`.

## Desktop resizing

Owner permission is separate from server support. Enable
`desktop_resize.policy.enabled` only for the owned container session, with
`max_width`, `max_height` and `max_pixels` limits. An optional adapter uses the
explicit `desktop_resize.vnc_address`; existing X11 users need no VNC dependency.

Check `doctor`, then call `resize '{"width":1920,"height":1080}'` or MCP
`device_resize` with the same dimensions. A confirmed result includes requested
and applied dimensions and current surface geometry. Capture a fresh screenshot
before input. Pre-resize coordinates remain stale after a grow/shrink round
trip. Screenshot crop/output dimensions and viewer Fit do not resize the desktop.

The CLI/MCP runner serializes commands and writes a `target.json.resize-pending`
journal before dispatch. After a timeout, disconnect or process exit during
mutation, the owner must reconcile the same session before removing that journal.
`device_resize` cannot enable permission or clear uncertainty. A separate
`target.json.resize-observe` marker blocks input until a successful fresh screenshot.
A bounded operation already in progress finishes even if its MCP caller cancels.

Target filenames must not end in `.lock`, `.resize-pending` or `.resize-observe`.

CLI/MCP errors include `resize_uncertain`: true means a resize may have been
applied and requires owner reconciliation before retrying.

To change permission during a running session, call `device_set_resize_enabled`
with `{"enabled":true}` or `{"enabled":false}`, or use
`horizon-device --target <file> --resize-enabled true|false` from another CLI.
The next operation reloads the saved setting; no MCP restart is needed. Limits,
endpoint and pending journals are preserved. A busy result means retry the
permission change after the current operation finishes; disabling cannot cancel
an already dispatched resize or reconcile uncertainty.
