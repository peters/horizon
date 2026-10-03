# Panel resize handle: temporary native smoke plan

Candidate: branch `fix/panel-resize-trackpad`, based on `08415419`.
Outcome: a visible 32-screen-point bottom-right panel grip that works with
trackpad-sized targeting errors, even when the canvas is zoomed out. Input on
the grip must resize rather than select terminal text or reach a browser/device.

## Environment and evidence

1. Build `cargo build -p horizon-ui --bin horizon` and
   `cargo build -p horizon-device --features cli` in this worktree's separate
   Cargo target directory. Freeze both executables in a new owned directory;
   record SHA-256, source commit, fixture manifest and actual Horizon child PID.
2. Start `scripts/device-smoke/serve.py --native-view` on a newly allocated
   display and state directory, per `scripts/device-smoke/README.md`.
   This Linux host may need
   `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`
   for Xvfb. Do not remove isolation if bwrap cannot create a namespace.
3. Create a task-owned public `device_panel` viewer in the calling workspace.
   Record at least three timestamped inspections, two seconds apart, proving
   connected, received and displayed images with advancing heartbeat frames.
   Use only the bounded viewer health recovery procedure in the fixture README.
4. Run device doctor and take a screenshot before input; use the explicit
   fixture target and fresh geometry for every action. Never operate on the
   developer's desktop. Start a recorder scoped to the fixture display before
   the feature flow; decode representative frames before claiming motion proof.

## Baseline and primary flows

1. At launch, capture the full desktop and inspect both panel corners. Each
   has a visible square grip, three diagonal strokes and a quiet border.
2. Hover inside the upper-left of the grip, 28 screen points from the corner
   on both axes. Confirm accent feedback, diagonal resize cursor and tooltip.
3. Drag from that position outward slowly. Confirm smooth growth, stable top
   left, terminal rows/columns committed on release, and no text selection.
4. Drag inward to shrink, then grow again. Test long drags outside the original
   grip and panel; the gesture stays captured until release.
5. Fit the workspace and capture a screenshot. Repeat targeting and resizing
   at canvas zoom 25%, 50%, 100% and 200%; the grip is 32 screen points unless
   the whole panel is smaller. A small panel must keep its grip inside its bounds.
6. Repeat on focused and unfocused panels; resizing an unfocused panel focuses
   it. Titlebar move, rename, close, microphone and context menu still work.

## Edge cases and interaction boundaries

1. Start text selection just outside the grip, at least 40 screen points from
   the corner on both axes. Verify normal terminal selection and no resize.
2. Resize a browser panel and a native Device panel in the isolated Horizon.
   Grip input must not reach web content or Device interaction. Use a second
   task-owned synthetic desktop for the Device target and its separate geometry.
3. Test adjacent panels, panel partially clipped by the canvas, and arranged
   workspace collision/minimum-size constraints. No unrelated panel moves or
   overlapping control acquires the gesture.
4. Open a host dialog or session picker and attempt a grip drag: resize remains
   disabled. Canvas pan and keyboard navigation keep their existing behavior.
5. Detach a workspace, repeat resize there, reattach and fit. Check screenshots
   after launch and resize/fit. Repeat the visible flows in dark and light themes.

## Persistence and regression checks

1. With task-owned private persisted state (omit `--ephemeral` in an equivalent
   isolated fixture), resize, close normally, relaunch the frozen binary and
   confirm saved panel size and usable corner. No configuration migration is
   required; existing saved panel geometry must load unchanged.
2. Repeat idle and pointer movement over blank canvas, panel body and grip.
   Hover must not add continuous animation or repaint when stationary.
3. Record an end-to-end synthetic clip: hover, grow, shrink, zoom out, resize,
   fit. Convert to a two-pass palette GIF around 6 fps/1000 px, under 10 MB.
   Inspect frames for movement, final candidate identity and synthetic content.
   Keep private until authorized; a UI PR requires this GIF at the top.
4. Close only owned windows/viewer/fixtures normally, verify child exit and
   target expiry. Remove this temporary plan only after completed UI validation.

## Current blocked lane

Native baseline could not start: Xvfb's NVIDIA EGL crash was avoided with the
Mesa vendor setting, but bwrap then failed with “No permissions to create a new
namespace.” No live viewer, interactive assertions, video or GIF can be claimed
from this run. Headless tests do not substitute for this lane.
