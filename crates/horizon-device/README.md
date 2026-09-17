# horizon-device

Small Rust library for device observations and bounded input. Linux X11 is the
only implemented backend. It has no dependency on Horizon or a GUI/model runtime.
The optional `cli` feature adds one binary with CLI and stdio MCP interfaces.
Building on Linux requires the libxkbcommon development library (for example
`libxkbcommon-dev` on Debian/Ubuntu). Live input also requires X11/XTest access.

```sh
cargo build -p horizon-device --features cli
horizon-device --target /private/lab/target.json doctor
horizon-device --target /private/lab/target.json screenshot /private/lab/before.png
horizon-device --target /private/lab/target.json act - < /private/lab/action.json
horizon-device --target /private/lab/target.json mcp
```

Create the target file in a private directory, choosing an explicitly authorized
local display. There is no ambient `DISPLAY` fallback:

```json
{"id":"lab","endpoint":{"kind":"local_x11","display":":99"}}
```

CLI results are `{ "ok": true, "result": ... }` or
`{ "ok": false, "error": { "code": ..., "message": ... } }`. Exit status is 0 for
success, 1 for a device failure, and 2 for invalid CLI arguments/output failure.
Screenshot without a path returns PNG base64; with a path it creates a new mode
0600 file and returns its path. Existing files are never overwritten.

MCP exposes `device_doctor`, `device_screenshot`, and `device_act`; screenshots
include an MCP image block and geometry metadata. CLI `act` and MCP `device_act`
accept the same JSON object:

```json
{"geometry":{"target_id":"lab","surface_id":"display","width":1280,"height":800,"revision":"COPY FROM YOUR SCREENSHOT"},"action":{"kind":"click","at":{"x":640,"y":610},"button":"left"}}
```

Other actions: `drag` with `from`, `to`, `duration_ms`; `scroll` with `at`,
`vertical_notches`, `horizontal_notches`; `type` with UTF-8 `text`; `key` with
`key` and `modifiers`. See the Rust enums or MCP tool schema for supported keys.
Input limits: drag 1–2000 ms, scroll ±100 notches per axis, text 256 Unicode
scalars and 4096 UTF-8 bytes without NUL, up to four modifiers. X11 text is paced
at 20 ms per character with a final 100 ms drain interval; split longer text into
bounded actions and observe the result. Coordinates must fall inside the screenshot.
Use stdin for entered text to avoid exposing it in process arguments/history.

Library consumers need no async runtime:

```rust,no_run
use horizon_device::{Device, Endpoint, Target};
let target = Target {
    id: "lab".into(),
    endpoint: Endpoint::LocalX11 { display: ":99".into() },
};
let device = Device::connect(&target)?;
let observation = device.screenshot()?;
assert_eq!(observation.mime_type, "image/png");
# Ok::<(), horizon_device::DeviceError>(())
```

## Contract and limits

Target identity and endpoint are distinct. Geometry must accompany actions;
stale geometry fails before injection. This checks target/display/size, not the
age or contents of the UI. Keep the lab alive and observe immediately before and
after acting. A `dispatched` receipt does not prove the app reached the desired
state. Never automatically replay an `indeterminate` action.

Coordinates are screenshot pixels. Wheel notches are explicit, not mobile
swipes. Touch and accessibility are unsupported. Capture requires little-endian
BGRX32, at most 8,294,400 pixels, and dimensions no greater than 32768 pixels. `doctor` probes capture and input readiness.
Wayland and non-Linux endpoints are not implemented.
Multi-screen X11 servers are rejected: the current input dependency cannot
guarantee that absolute pointer input targets the same root as the observation.
Multiple monitors represented by one X11 screen are subject to the same capture
size limit; they have not been separately qualified.

Run one controller per display. CLI/MCP cooperates through a persistent `.lock`
sibling of the target config; all clients must use the same config path in a
caller-owned private directory. Library callers serialize their own access.
The lock does not exclude unrelated desktop programs or a human user.
MCP cancellation/disconnection lets an active bounded input finish and release
held input. Abrupt process termination or a failing X server cannot guarantee
cleanup; reobserve before recovery. No autonomous retry or device reconnection.

## Integration and provenance

The bundled `skills/horizon-device/SKILL.md` describes agent usage. Register an
MCP server with the executable path and arguments `--target /private/lab/target.json
mcp`, or use the CLI directly. The target configuration must name an existing,
explicitly authorized display. The caller owns its startup and cleanup.
The crate contains no viewer server, application launcher, or lifecycle daemon.
A separate read-only viewer can observe the same isolated display.

[Enigo](https://github.com/enigo-rs/enigo) (MIT) handles input;
[x11rb](https://github.com/psychon/x11rb) (MIT/Apache-2.0) handles X11 capture.
XCap was evaluated, but its mandatory Linux PipeWire dependencies exceeded this
X11 MVP's needs. No upstream code was forked or copied. Live viewing is optional and is not bundled into this MIT crate.

The package has independent version/dependency metadata and can be packaged
outside the Horizon workspace. No publication is part of this MVP.
