# Device desktop resize adapter

This optional executable adds VNC resize negotiation to the device CLI/MCP
contract. Screenshots and input still use local X11. The standalone
`horizon-device` crate does not depend on this adapter or the VNC decoder.

Build with `cargo build -p horizon-device-vnc`. Run inside the dedicated Linux
container, using a private target file that binds its X11 display and loopback
VNC server to the same desktop:

```json
{
  "id": "container-desktop",
  "endpoint": {"kind": "local_x11", "display": ":99"},
  "desktop_resize": {
    "vnc_address": "127.0.0.1:5900",
    "policy": {
      "enabled": true,
      "max_width": 1920,
      "max_height": 1080,
      "max_pixels": 2073600
    }
  }
}
```

The container owner sets permission and limits; omitted permission is disabled.
Only numeric loopback endpoints with a nonzero port are accepted. The server
must permit shared, unauthenticated loopback connections and advertise
ExtendedDesktopSize. Password authentication and general remote management are
outside this adapter.

```sh
horizon-device-vnc --target /private/session/target.json doctor
horizon-device-vnc --target /private/session/target.json resize '{"width":1920,"height":1080}'
horizon-device-vnc --target /private/session/target.json screenshot /private/session/fresh.png
horizon-device-vnc --target /private/session/target.json mcp
```

MCP exposes `device_doctor`, `device_resize`, `device_screenshot` and `device_act`.
Check support and permission separately. Resize returns requested and confirmed
applied dimensions with current geometry. Capture a fresh screenshot before
input; old geometry is rejected even after returning to the original size.
Screenshot crop/output size and viewer Fit leave the desktop unchanged.

Connection/negotiation and resize confirmation have bounded waits. A timeout or
disconnect after dispatch leaves a journal in the private target directory.
Do not retry blindly: the owner must reconcile the same session before removing
`target.json.resize-pending`. A cancelled MCP request does not cancel an already
running bounded mutation. Input resumes only after a successful new screenshot.

The cancellation/concurrency smoke uses a fake VNC server and an explicitly
owned X11 target; it does not resize that desktop:

```sh
python3 scripts/device-smoke/resize_lifecycle.py \
  --binary target/debug/horizon-device-vnc --target /private/session/target.json
```
