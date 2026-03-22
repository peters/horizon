# Linux Primary Selection Smoke Test

Branch: `fix/linux-primary-selection`  
PR: `#97`  
Issue: `#95`

## Goal

Verify that Linux primary-selection behavior works without regressing standard canvas pan and terminal mouse-mode behavior.

## Environments

Run this plan on at least one of:

- X11 session
- Wayland session

If both are available, run the full plan on both.

## Preconditions

- Build the PR branch:

```bash
cargo run --release
```

- Start Horizon with a clean config state if possible:

```bash
HOME="$(mktemp -d)" cargo run --release
```

- Create one normal terminal panel.
- Keep another app available for cross-app copy/paste checks, for example a browser, text editor, or terminal emulator outside Horizon.

## Baseline Launch

1. Launch Horizon.
2. Confirm the main window appears normally.
3. Resize the window larger and smaller.
4. Confirm the canvas and terminal still render correctly.

Expected:

- Horizon launches without errors.
- Resize does not break layout or input.

## Standard Selection To PRIMARY

1. In a Horizon terminal, print distinct text:

```bash
printf 'alpha primary test\nbeta primary test\ngamma primary test\n'
```

2. Drag-select `beta primary test` with the mouse.
3. In another Linux app, middle-click paste.

Expected:

- The selected Horizon text is pasted through Linux PRIMARY.
- No explicit Ctrl+C is required.

## Standard PRIMARY Paste Back Into Horizon

1. In another Linux app, select a unique string with the mouse, for example:

```text
outside-primary-source-123
```

2. Focus the Horizon terminal.
3. Middle-click inside the terminal body.

Expected:

- The PRIMARY selection is pasted into the Horizon terminal.
- Paste happens on middle-click without freezing the UI.

## Clipboard Shortcut Behavior Still Works

1. In Horizon, select text in the terminal.
2. Use the normal copy shortcut for the platform.
3. Paste into another app with the normal clipboard shortcut.
4. Also middle-click paste in another app.

Expected:

- Normal clipboard copy still works.
- PRIMARY is also updated from terminal selection/copy behavior on Linux.

## Empty Canvas Middle-Click Pan

1. Move the pointer to empty canvas, outside any terminal body.
2. Press and hold middle mouse.
3. Drag the canvas.

Expected:

- The canvas pans.
- No paste is triggered.

## Terminal Body Middle-Click Should Prefer PRIMARY Paste

1. Put a known PRIMARY selection in another app.
2. Middle-click inside the Horizon terminal body.

Expected:

- PRIMARY paste happens.
- The canvas does not pan.

## Forced Pan Override

1. Hold `Ctrl`.
2. Middle-click and drag inside the Horizon terminal body.

Expected:

- The canvas pans even over the terminal body.
- PRIMARY paste does not trigger.

## Mouse-Mode Terminal Regression Check

1. In a Horizon terminal, run a mouse-reporting app:

```bash
vim
```

or

```bash
htop
```

2. Without modifiers, middle-click inside the terminal body.
3. Hold `Ctrl`, then middle-click-drag inside the terminal body.

Expected:

- Plain middle-click still follows the Linux PRIMARY behavior for Horizon, not stray canvas pan.
- `Ctrl` + middle-click-drag pans the canvas.
- The mouse-reporting app does not receive leaked button-2 drag input during forced canvas pan.

## Non-Terminal Area Regression Check

1. Middle-click on panel chrome, sidebar, and empty canvas areas.
2. Try normal left-click focus and drag interactions afterward.

Expected:

- Non-terminal UI remains responsive.
- No stuck pan state.
- No unexpected paste outside terminal bodies.

## Large Selection / Responsiveness

1. Print a large block in the Horizon terminal:

```bash
python - <<'PY'
for i in range(2000):
    print(f'line-{i:04d}')
PY
```

2. Select a large region.
3. Middle-click paste from another app into Horizon.
4. Middle-click paste Horizon-selected text into another app.

Expected:

- Horizon remains responsive.
- No visible UI stall during PRIMARY read/write.

## Wayland-Specific Notes

On Wayland, compositor support for PRIMARY can vary.

Record:

- compositor / desktop environment
- whether PRIMARY paste worked
- whether behavior differed from X11

If PRIMARY is unsupported by the compositor, Horizon should remain responsive and fail gracefully rather than hanging.

## Pass Criteria

- Launch and resize are normal.
- Mouse selection in Horizon populates Linux PRIMARY.
- Middle-click paste into Horizon reads Linux PRIMARY.
- Empty-canvas middle-click still pans.
- `Ctrl` + middle-click forces pan over terminal bodies.
- Mouse-mode terminals do not receive leaked button-2 drag events during forced pan.
- No visible UI freeze during middle-click paste.

## Report Template

Include:

- distro
- X11 or Wayland
- desktop environment / compositor
- whether each section above passed or failed
- any freeze, delay, or unexpected mouse behavior
