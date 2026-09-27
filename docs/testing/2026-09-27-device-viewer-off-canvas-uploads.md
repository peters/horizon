# Native Device viewer: off-canvas uploads smoke (issue #1032)

A connected Device viewer that is not drawn (off canvas, hidden, behind a
fullscreen panel) now keeps uploading the latest received frame about once a
second instead of freezing `frame_sequence`. Presentation stays truthful:
`image_displayed` is false and `presentation` reports `not_rendered` with the
host exclusion until the viewer is drawn again. Nothing moves the person's
camera, and the host is not repainted at the stream rate.

## Lane A: deterministic

- `device_widget::tests::undrawn_viewer_uploads_in_the_background_without_claiming_display`:
  a pass that never draws the viewer uploads the pending frame
  (`frame_sequence` 1, `image_received`, `last_uploaded_age_millis` present,
  `image_displayed` false, `not_rendered`); a newer frame inside the upload
  interval waits, schedules a delayed repaint of at most the interval so a
  stream that then goes static cannot strand it, and uploads once the interval
  has elapsed; drawing presents the already uploaded frame.
- `device_widget::session::tests::hidden_viewer_receives_first_image_and_retains_only_latest_with_delayed_repaints`:
  hidden frames request only delayed repaints (no immediate repaint); a
  visible update requests an immediate one.
- `app::device_tests::off_canvas_viewer_keeps_uploading_without_being_presented`:
  a viewer positioned outside the canvas is not rendered, reports
  `outside_canvas`, and still reports `frame_sequence` 1 with
  `image_displayed` false.

Status: **PASS** (2026-09-27, Linux x64; `cargo test -p horizon-ui device`,
111 tests).

## Lane B: live, isolated desktops

Setup: target desktop on Xvfb `:60` (1024x768, openbox, `xclock -digital
-update 1`, `xeyes` with a scripted pointer so content changes at several
frames per second) served by loopback `x11vnc` on `127.0.0.1:5960`; a candidate
Horizon (`--config <private yaml> --ephemeral`, private `HOME`) on Xvfb `:61`
with one `kind: codex` panel whose command is a Python probe that records the
Horizon-injected identity. `device_panel` was driven through
`horizon --browser-mcp` with that identity. The `:61` desktop was itself viewed
live through a native Device panel in the developer's current workspace
(`connected`, `displayed`, `frame_sequence` advancing 284 -> 523 across the
run) and recorded with `ffmpeg` (`x11grab`, 10 fps; every recording decoded
end to end).

Candidate: release build of this branch after the review round that makes a
waiting frame schedule its own wake, SHA-256 prefix `8cdee1ba5e1e0214`. The
recording of this run is 60 s at 10 fps.

1. Create, reveal: `displayed`, `frame_sequence` 23 with
   `last_uploaded_age_millis` 153.
2. Wheel the canvas until the viewer is completely off canvas (screenshot:
   empty canvas, viewer only in the minimap). Target content changing several
   times a second (scripted pointer over `xeyes`). Eight inspections over eight
   seconds: `presentation: not_rendered`, `exclusion: outside_canvas`,
   `image_displayed: false`, `frame_sequence` 28 -> 36 while
   `received_frame_sequence` went 37 -> 96, `last_uploaded_age_millis` never
   above 983.
3. Stop the pointer script so only the 1 Hz clock changes (a slow stream, the
   case where a frame arriving just after an upload used to wait for an
   unrelated repaint). Six inspections over nine seconds:
   `received_frame_sequence` 105 -> 113 and `frame_sequence` 37 -> 45, so every
   received frame was uploaded within the interval.
4. Wheel back: the first inspection after the pan reads `displayed`.
5. Host cost while off canvas: `ui_pass` advanced 148 -> 233 over eighteen
   seconds (under five passes a second, including the inspections), against
   276 -> 279 in under a second right after panning back.

The first candidate of this branch (SHA-256 prefix `b4ab4a17bb2c794d`, before
the review round) gave the same shape with a continuously changing target:
`frame_sequence` 37 -> 45 while off canvas, `ui_pass` 1362 -> 1383 in ten
seconds against 1387 -> 1437 in four seconds displayed.

Status: **PASS** (2026-09-27, Linux x64).

## Baseline on main (same lab, main build from 2026-09-26)

Completely off canvas: `frame_sequence` froze immediately (26 for the whole
off-canvas period) while `received_frame_sequence` advanced 28 -> 40; a
partially visible viewer kept uploading. This matches the developer-host
incident recorded on #867 and #1032 (`frame_sequence` 799 against 10,218
received frames after the person focused a first-row agent panel and the
second-row viewer left the canvas).

## Not covered

- macOS and Windows use the same code path; only Linux was run live.
- Detached viewports were not exercised live; the background upload runs from
  the root pass and the detached pass re-registers its viewport when it draws
  the viewer again.
