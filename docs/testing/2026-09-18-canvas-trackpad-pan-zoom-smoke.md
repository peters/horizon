# Smoke Test: Canvas Trackpad Pan And Zoom

Temporary validation for two-finger canvas pan over focused panels, Ctrl+scroll zoom, and Ctrl+Plus/Minus on a non-US layout.

Delete this file after the UI validation pass is complete unless it is explicitly needed longer.

## Target

- Repository: `peters/horizon`
- Branch: `fix/canvas-zoom-touchpad-layout`
- Platform: Linux laptop with a multitouch clickpad (System76 Bonobo WS / `bonw16` is the original report). A Norwegian or other non-US keyboard layout is required for the Ctrl+Plus lane.

## Build

Use `target/debug/horizon` from this branch. Launch with an isolated config if another Horizon is already running.

## Setup

1. Open a workspace with at least one terminal panel and some empty canvas around it.
2. Click the terminal so it is focused. Leave the pointer over the terminal body.

## Baseline

1. Two-finger scroll on empty canvas pans the board. Expected: canvas moves, terminal does not scroll.
2. Click the terminal. Type a character. Expected: the character appears in the shell.

## Primary flows

1. Pointer over the focused terminal, two-finger scroll.
   - Expected: the **canvas** pans. Terminal scrollback does not move.
2. Pointer over the focused terminal, **Shift** + two-finger scroll.
   - Expected: terminal scrollback moves. Canvas does not pan.
3. Pointer over empty canvas, two-finger scroll.
   - Expected: canvas pans, same as before.
4. Hold **Ctrl** and two-finger scroll over empty canvas and over a panel.
   - Expected: canvas zoom changes in both cases. Terminal does not receive the wheel.
5. Command palette **Reset Zoom** / **Zoom In** / **Zoom Out** still work. Keyboard Ctrl+Plus/Minus on non-US layouts is a follow-up (layout-unmodified shortcut matching).
6. **Space** + one-finger drag over a panel.
   - Expected: canvas pans. No space is inserted into the terminal.

## Edge cases

1. Start a text selection in the terminal, keep the button down, then two-finger scroll.
   - Expected: the terminal still scrolls with the selection (primary-button gesture keeps the wheel).
2. Two-finger scroll over the panel titlebar (not the body).
   - Expected: canvas pans.
3. Browser panel: unmodified two-finger pans the canvas; Shift+two-finger scrolls the page.
4. Fit workspace, then Ctrl+scroll and two-finger pan. Expected: zoom/pan still apply.
5. Fullscreen a terminal (panel fullscreen). Unmodified two-finger scroll. Expected: the terminal scrollback moves; the canvas is not visible so it must not steal the wheel.
6. Browser panel with a long native `<select>` list open. Unmodified two-finger over the menu. Expected: the menu scrolls; the canvas does not pan.

## Persistence

Close and reopen Horizon. Pan/zoom restore from the session as before.

## Visual

Screenshot after launch and after a two-finger pan over a focused terminal.

This is a motion-sensitive check. Record a short video from the isolated test desktop of two-finger pan over a focused terminal so the board moves while terminal content stays put. Start the recorder before the gesture and stop after. If recording is unavailable or stalls, this lane is **blocked** — do not sign off from screenshots alone.
