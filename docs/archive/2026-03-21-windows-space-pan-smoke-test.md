# Windows Smoke Test: `Space + Left-Drag` Canvas Pan

This is a temporary validation artifact for the fix that prevents `Space + left-click drag` from inserting a space into the focused terminal on Windows.

Delete this file after the Windows validation pass is complete unless it is explicitly needed longer.

## Target

- Repository: `peters/horizon`
- Branch/PR: fix for Windows `Space + left-click drag` leaking `Space` into the focused terminal
- Platform: Windows 11 preferred, Windows 10 acceptable
- Tester: separate agent or machine, not this Linux workstation

## Build Or Install

Choose one:

1. Build from source on the PR branch
   - Install Rust stable `>= 1.88`
   - Clone the PR branch
   - Run `cargo run --release`
2. Use the PR CI artifact if one is attached
   - Download the Windows build artifact for the PR
   - Launch `horizon.exe`

## Test Setup

1. Start Horizon with a clean session if practical.
2. Create one workspace and one terminal panel.
3. In the terminal, run a command that makes inserted spaces obvious, for example:
   - `python -q`
   - `cmd`
   - `powershell`
4. Click inside the terminal so it is definitely the focused input target.

## Baseline Checks

1. Press `Space` once without dragging.
   - Expected: one space is inserted into the terminal.
2. Type `a`, `Space`, `b`.
   - Expected: the terminal receives `a b` in that order.
3. Hold `Space`, release it without clicking.
   - Expected: one space is inserted, with no stuck input state afterward.

## Primary Regression Checks

1. Focus the terminal.
2. Hold `Space`.
3. Press and hold left mouse.
4. Drag the canvas several hundred pixels.
5. Release left mouse, then release `Space`.
   - Expected: the canvas pans.
   - Expected: no space is inserted into the terminal.
   - Expected: no extra key-up escape sequence or visible artifact appears in the terminal.
6. Repeat the same flow starting the drag from:
   - terminal body
   - panel titlebar
   - empty canvas

## Edge Cases

1. Hold `Space`, then press another key such as `A` before releasing `Space`.
   - Expected: if no drag occurred, input order remains sane and Horizon does not get stuck in pan mode.
2. Hold `Space`, click without moving, then release.
   - Expected: no accidental panel drag or resize.
   - Expected: the terminal does not receive stray spaces from a click-only gesture.
3. Hold middle mouse and drag.
   - Expected: canvas panning still works.
4. Use `Ctrl+click` or `Cmd+click` style terminal link interactions if available on the platform.
   - Expected: the fix does not break existing modifier-based interactions.
5. With multiple panels open, focus one terminal and perform the regression check.
   - Expected: no other terminal receives the leaked space.

## Persistence And Relaunch

1. Pan the canvas after the fix.
2. Close Horizon cleanly.
3. Relaunch Horizon.
   - Expected: normal persisted layout behavior remains unchanged.
   - Expected: the fix does not introduce stuck pan state after relaunch.

## Visual Regression Checks

1. Capture a screenshot right after launch.
2. Capture a screenshot after a successful `Space + left-drag` pan.
3. Resize the main window and repeat the pan gesture once.
   - Expected: panel chrome, workspace labels, and terminal rendering remain stable.
   - Expected: no jitter, snap-back, or unexpected window movement occurs.

## Report Back

Include the following in the PR comment or test report:

- Windows version
- How Horizon was launched
- Pass/fail for each primary regression check
- Any additional bugs found during the gesture test
- Launch screenshot and post-pan screenshot
