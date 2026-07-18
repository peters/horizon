# Canvas Scroll Pan Smoke Test

## Goal

Verify that wheel and trackpad scrolling pans the canvas exactly once per input
event. Before this fix the pan applied `smooth_scroll_delta + raw_scroll_delta`,
which doubled every trackpad gesture and gave wheel notches an immediate jump
plus a duplicated smoothed tail.

## Fix Under Test

`crates/horizon-ui/src/app/actions/interaction.rs` now feeds only
`smooth_scroll_delta` into canvas panning (`wheel_pan_scroll_input`).

## Setup

- Build the debug binary: `cargo build -p horizon-ui`.
- Launch `target/debug/horizon` with an isolated `HOME` and isolated Horizon
  runtime state.
- Create one workspace with at least one terminal panel and leave visible empty
  canvas around it.
- On macOS test both a trackpad and, if available, an external notched mouse
  wheel; the two input paths behave differently in egui.

## Baseline

- Confirm the app launches and the workspace renders normally.
- Screenshot the initial canvas position for comparison.

## Primary Flow

- Place the pointer over empty canvas (not over any panel).
- Make one small deliberate trackpad swipe. The canvas must track the gesture
  1:1: content under the pointer should move with the finger travel, without
  moving roughly twice as far as the gesture.
- With a notched wheel, click one notch. The canvas must move once, smoothly,
  with no immediate jump followed by a continued drift of a second full notch.
- Repeat both in a detached workspace window (right-click workspace label →
  Detach Workspace); the same pan path runs there.

## Interaction Edge Cases

- Hold Shift and scroll vertically: the canvas must pan horizontally by the
  same single amount.
- Scroll with the pointer over a terminal panel: the terminal scrollback must
  scroll; the canvas must not pan.
- Hold Ctrl (Cmd on macOS) and scroll over empty canvas: zoom must engage
  instead of pan, anchored at the pointer, at its usual rate (zoom uses
  `zoom_delta`, which is unaffected by this change).
- Pinch-zoom on trackpad must behave as before.
- Middle-drag and Space+drag panning must behave as before (pointer-delta
  driven, unaffected).
- Start a fast fling-style trackpad scroll and stop abruptly: the pan must not
  continue further than the gesture's momentum events.

## Persistence

- Pan somewhere distinctive, quit, relaunch with the same runtime state, and
  confirm the canvas restores to the persisted view without an extra offset.

## Visual and Performance Regression Checks

- During and after scrolling, confirm no visual artifacts in workspace
  backgrounds, panels, or the minimap.
- Confirm idle CPU returns to baseline after the smoothed wheel tail settles
  (a wheel notch may repaint for a few frames while egui drains smoothing;
  it must stop).

## Cleanup

- Stop only the Horizon PID launched for this test and remove the isolated
  home. Delete this temporary smoke-test plan after the validation pass.
