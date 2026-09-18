---
name: horizon-device
description: Observe and control an explicitly configured local device through device MCP tools or the horizon-device CLI. Use for native application smoke tests; browser tasks use horizon-browser.
---

Use `device_doctor`, then `device_screenshot`. Send the returned geometry unchanged
with one bounded `device_act` action. Observe the result before continuing; a
`dispatched` receipt confirms input delivery, not application success. On stale
geometry take another screenshot. On indeterminate input observe before deciding
whether another action is appropriate; never replay blindly.

The local CLI has the same contract:
`horizon-device --target <private-target.json> doctor|screenshot|act <JSON>`.
For screenshots an optional output path writes a new private file instead of
base64 JSON. Optional `--options JSON` (or `--options -` for stdin) accepts
`region: {x,y,width,height}`, `output: {width,height}`, `format: png|jpeg`, and
JPEG-only `quality: 1..100`. MCP screenshot accepts the same options directly.
Omitting options preserves full-resolution PNG; JPEG defaults to quality 85. `act -` reads JSON from stdin. Use this to avoid putting entered text
in shell history. The MCP server uses `--target <file> mcp` and stays bound to that
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
Never use device input to control a browser; use Horizon browser tools instead.

For native application smoke tests, the caller owns the isolated display,
application startup and cleanup. Independent applications can run in parallel
when requested: give each its own unused display, private configuration/home,
expiring target, viewer ports and process tree. Bind each agent to its exact
target; do not share screenshot geometry across fixtures. Never control the
developer's desktop, change production configuration, or stop a pre-existing
application. When a target expires, stop; do not recreate it from a saved display
number.

Finish the intended build, copy executables to a new task-owned directory, and
record their hashes before launch. Keep build caches separate across source
checkouts during qualification. Verify the running application's executable/hash
against that frozen copy, following the actual child rather than a sandbox
launcher such as `bwrap`. Close only the owned candidate normally when replacing
it; a rebuild does not change a running process.

A separate read-only viewer can observe the same display; it does not provide
device tools. In Horizon, use an outer noVNC browser panel through the
horizon-browser skill and public `browser_*` tools. Its native Device panel is a
read-only VNC viewer, not a browser. Keep native input on this CLI/MCP contract;
never use it to automate the outer browser. Each nested viewer/target desktop
needs its own target configuration.

For feature evidence, use the viewer's supported recording controls. With
Horizon `browser_video`, stop recording and copy its finalized WebM export before
`browser_close` deletes the profile and exports. Browser viewport/video settings
do not change native desktop dimensions. Use screenshot options for native
crop/quality, and existing browser_video quality/fps/max_width/max_file_bytes
for bounded evidence. Client-side scaling does not reduce VNC wire bandwidth. Inspect evidence before retaining it. Keep internal application names,
scenarios, images, video and operational details private; public demonstrations
use generic fixtures and synthetic content only.

Close only task-owned viewers and application/display fixtures, then verify
children exited and target configuration expired. This packaged skill does not
automatically register an MCP server or start a viewer: callers must supply the
CLI executable and target or register the explicit `--target <file> mcp` command
in their agent's supported configuration.

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

The CLI/MCP runner serializes commands and writes a `target.resize-pending`
journal before dispatch. After a timeout, disconnect or process exit during
mutation, the owner must reconcile the same session before removing that journal.
`device_resize` cannot enable permission or clear uncertainty. A separate
`target.resize-observe` marker blocks input until a successful fresh screenshot.
A bounded operation already in progress finishes even if its MCP caller cancels.
