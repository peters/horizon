# Device view controls — smoke test plan

Validates that every native Device panel **View control** changes the local
image, including after the VNC worker has disconnected. These controls never
resize the target desktop or change VNC compression.

Public evidence must use only the synthetic Horizon fixture. Do not capture or
attach third-party application frames.

Build from this worktree with its own Cargo target directory, freeze the
binaries, then launch fixtures from those copies:

```sh
export CARGO_TARGET_DIR="$PWD/target/device-smoke"
cargo build -p horizon-device --features cli
cargo build -p horizon-ui --bin horizon
smoke_bin=$(mktemp -d /tmp/horizon-smoke-bin.XXXXXX)
cp "$CARGO_TARGET_DIR/debug/horizon" "$smoke_bin/horizon"
cp "$CARGO_TARGET_DIR/debug/horizon-device" "$smoke_bin/horizon-device"
sha256sum "$smoke_bin/horizon" "$smoke_bin/horizon-device" > "$smoke_bin/SHA256SUMS"
```

Use a task-owned isolated desktop and a **visible native Device panel** in the
calling agent's current workspace (`scripts/device-smoke/README.md`). Do not
use noVNC or drive the developer's desktop.

## Lane A — unit (no live viewer)

```sh
cargo test -p horizon-ui --bin horizon device_widget -- --test-threads=1
cargo test -p horizon-core device::view -- --test-threads=1
```

Must include:

- `image_limits_and_viewport_apply_without_a_live_session` — image limits and
  viewport crop change the presented texture while status is Disconnected.
- `fit_one_to_one_and_whole_desktop_clicks_work_without_a_session` — Fit, 1:1
  and Whole desktop clicks take effect on a retained desktop.
- `apply_viewport_and_reconnect_keep_the_last_desktop` — Apply viewport crops
  the last frame; Reconnect starts a worker without dropping that image.
- `fps_only_change_does_not_resample_the_retained_desktop` — Maximum fps does
  not recrop or rescale the last desktop.
- `discarded_stale_worker_frame_represents_the_latest_desktop` — a worker frame
  produced with older crop/limits still updates the retained desktop.
- `viewport_scaling_samples_the_selected_source_pixels` — crop/scale samples
  the selected source pixels.
- `an_active_viewport_can_be_cleared_without_desktop_geometry` — Whole desktop
  remains available after the desktop size is unknown.
- `shrink_with_an_outside_crop_preserves_the_session` — a later desktop shrink
  does not tear down the worker; the UI clears an invalid crop.

## Lane B — nested native viewer

Keep two isolated desktops. The source fixture is a synthetic Horizon. The
viewer fixture is a second Horizon whose Device panel points at the source
VNC. Watch the viewer fixture live through `device_panel` in the current
workspace.

```sh
python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
  --native-view --state /tmp/horizon-device-view-source

device_vnc_address=$(python3 -c 'import json; print(json.load(open("/tmp/horizon-device-view-source/lab.json"))["vnc_address"])')

python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
  --native-view --device-address "$device_vnc_address" --state /tmp/horizon-device-view-viewer
```

Create a visible native Device panel from the **viewer** `lab.json`
`vnc_address`. Inspect until `connection` is `connected`, `image_received` and
`image_displayed` are true, and `frame_sequence` advances while the source
heartbeat updates.

Verify `/proc/<horizon-pid>/exe` in each fixture matches `SHA256SUMS`. Drive
the **viewer** desktop with `horizon-device` and that fixture's `target.json`.
Take a fresh screenshot before every click. Map cropped screenshot pixels back
to original coordinates before `device_act`.

Record a short clip from the viewer display (not from the outer Device panel)
covering the control sequence below.

### B1. Expand View controls

Click **View controls**. The collapsing body stays open and shows Maximum fps,
Image limits, Fit, 1:1, Whole desktop, the desktop/image size label, viewport
fields, and Apply viewport.

### B2. Fit and 1:1

1. Note the presented image size label (`Desktop W×H · Image w×h`).
2. Click **1:1**. The image uses one presented pixel per UI point; scrollbars
   appear when the image is larger than the remaining panel body. Target
   desktop geometry is unchanged (`device_screenshot` of the **source**
   fixture still reports the original size).
3. Click **Fit**. The image letterboxes into the remaining panel area; the
   size label does not change.

### B3. Image limits

Lower **Image limits** below the current desktop (for a 1600×1000 source, set
width to 800). The `Image` size in the label shrinks while preserving aspect
ratio; `Desktop` stays 1600×1000. Source `device_screenshot` geometry is
unchanged. Restore 2048×2048 and confirm the image size returns to the desktop
size (or the active viewport size).

### B4. Viewport and Whole desktop

1. Set viewport to a nonempty rectangle inside the desktop (for example
   `x 0, y 0, w 800, h 500`) and click **Apply viewport**. The presented image
   crops to that rectangle; `Desktop` is still the full source size.
2. Click **Whole desktop**. The crop clears, the viewport fields return to the
   full desktop, and the image shows the whole source again.
3. Apply a rectangle that extends outside the desktop. The control shows the
   validation error and does not replace the current image.

### B5. Maximum fps

Set Maximum fps to 1, then 30. The source heartbeat must keep updating in both
cases. This control only throttles viewer refresh requests; it must not change
desktop resolution or the presented image size.

### B6. Reconnect and last-frame controls

1. Click **Reconnect** while the source is still up. Status becomes
   Connecting. The last image remains visible, but inspect must report
   `image_received: false`, `image_displayed: false`, and `frame_sequence: 0`
   until the new worker uploads a frame. Then Connected, and `frame_sequence`
   advances again.
2. Stop the source fixture (or its x11vnc) so the viewer reports Disconnected.
   The last desktop image must remain. Repeat B3 and B4 on that last frame:
   limits, Apply viewport, and Whole desktop must still change the **Image**
   size label. Fit/1:1 must still switch layout.
3. Restart the source and click **Reconnect**. The panel connects to the new
   server and shows a live image.

### B7. Read-only and teardown

Clicking the presented image must not type into or focus the source terminals.
Close only the task-owned Device panel (`device_panel` operation `close`).
Stop each owned fixture, confirm its children exited, and confirm `target.json`
expired. Keep only synthetic evidence.

## Lane C — restored panel

Restore a saved Device panel. It stays Stopped until **Reconnect**. View
options are session-local: recreating the panel restores defaults (20 fps,
2048×2048, Fit, no viewport).

## Pass criteria

| Control | Visible effect | Must not |
|---|---|---|
| Maximum fps | Refresh cadence changes; live source still updates | Change desktop size |
| Image limits | `Image` size in the label changes | Change `Desktop` size or VNC encoding |
| Fit | Image letterboxes into the panel body | Crop the source |
| 1:1 | Native presented pixels plus scroll when needed | Crop the source |
| Whole desktop | Clears crop; image returns to full desktop | Require a live worker when a last frame exists |
| Apply viewport | Crops to the requested rectangle | Accept out-of-bounds rectangles |
| Reconnect | Connecting, then a live or failed status | Drop the last image before a replacement |

Lane A is required. Lane B is required on Linux when Xvfb, Openbox, x11vnc and
a supporting host Device panel are available; otherwise report that lane
blocked. Lane C can share the viewer fixture after a normal close and restore.
