# macOS Terminal Hover Cache Smoke Test

## Goal

Verify that a hovered terminal body bypasses stale grid-cache reuse on macOS, while non-hovered panels, chrome, and empty-canvas movement keep their normal behavior.

## Test Build

- Use the exact branch and commit that will be pushed for review.
- Build the debug app:
  ```bash
  cargo build --locked -p horizon-ui --bin horizon
  ```
- Run the focused regression check:
  ```bash
  cargo test --locked -p horizon-ui --bin horizon hovered_terminal_bypasses_grid_cache
  ```

## Isolated Runtime Setup

1. Create an isolated runtime root:
   ```bash
   export SMOKE_ROOT="$(mktemp -d /tmp/horizon-hover-smoke.XXXXXX)"
   mkdir -p "$SMOKE_ROOT/home"
   ```
2. Save the helper below as `$SMOKE_ROOT/horizon-hover-cache-helper.py`:
   ```python
   #!/usr/bin/env python3
   import os
   import re
   import sys
   import termios
   import tty

   MOUSE_REPORT = re.compile(rb"\x1b\[<(\d+);(\d+);(\d+)([Mm])")

   def paint(count: int, button: int, column: int, row: int, suffix: str) -> None:
       status = (
           f"\x1b[2;1H\x1b[2Kreports={count:05d}  button={button:02d}  "
           f"column={column:03d}  row={row:03d}  suffix={suffix}"
       )
       sys.stdout.write(status)
       sys.stdout.flush()

   def main() -> None:
       input_fd = sys.stdin.fileno()
       previous = termios.tcgetattr(input_fd)
       buffer = bytearray()
       count = 0

       try:
           tty.setcbreak(input_fd)
           sys.stdout.write(
               "\x1b[2J\x1b[H"
               "Horizon terminal-hover cache smoke (press q to exit)\n"
               "Move the pointer across this terminal body.\n"
               "\x1b[?1003h\x1b[?1006h\x1b[?25l"
           )
           sys.stdout.flush()

           while True:
               chunk = os.read(input_fd, 128)
               if chunk == b"q":
                   break
               buffer.extend(chunk)

               consumed = 0
               for match in MOUSE_REPORT.finditer(buffer):
                   consumed = match.end()
                   count += 1
                   paint(
                       count,
                       int(match.group(1)),
                       int(match.group(2)),
                       int(match.group(3)),
                       match.group(4).decode("ascii"),
                   )

               if consumed:
                   del buffer[:consumed]
               elif len(buffer) > 64:
                   del buffer[:-16]
       finally:
           sys.stdout.write("\x1b[?1003l\x1b[?1006l\x1b[?25h\x1b[4;1H\nmouse reporting disabled\n")
           sys.stdout.flush()
           termios.tcsetattr(input_fd, termios.TCSADRAIN, previous)

   if __name__ == "__main__":
       main()
   ```
3. Create the isolated config at `$SMOKE_ROOT/config.yaml`:
   ```yaml
   version: 8
   window:
     width: 1280
     height: 860
   appearance:
     theme: dark
   workspaces:
     - name: Hover Cache Smoke
       cwd: REPLACE_SMOKE_ROOT
       terminals:
         - name: Mouse Reporter
           kind: command
           command: /usr/bin/python3
           args:
             - REPLACE_SMOKE_ROOT/horizon-hover-cache-helper.py
           position: [40.0, 40.0]
           size: [920.0, 620.0]
   ```
4. Replace `REPLACE_SMOKE_ROOT` in the config with the real `$SMOKE_ROOT` path.

## Launch

Launch Horizon with the isolated home and config:

```bash
HOME="$SMOKE_ROOT/home" \
RUST_LOG=horizon=info,horizon_core=info \
target/debug/horizon --config "$SMOKE_ROOT/config.yaml"
```

## Evidence to Collect

- A launch screenshot after the `Mouse Reporter` panel appears.
- A short motion recording during pointer sweeps:
  ```bash
  screencapture -V 8 "$SMOKE_ROOT/hover-motion.mov"
  ```
- A post-hover screenshot.
- If the session is automated, also record the exact Horizon PID and scope any window inspection or motion tooling to that PID.

## Primary Validation

1. Confirm the `Mouse Reporter` panel is visible and shows the helper banner.
2. Move the pointer over empty canvas outside the terminal body.
   - Expected: terminal text remains stable and does not repaint to stale content.
3. Move into the terminal body and sweep horizontally across several cells.
   - Expected: `column=` updates continuously and ends at the final hovered cell.
4. Sweep vertically across several rows.
   - Expected: `row=` updates continuously and ends at the final hovered cell.
5. Pause inside the terminal body.
   - Expected: the last reported `row=` and `column=` remain visible and do not revert.
6. Leave the body, pause on empty canvas, then re-enter.
   - Expected: the first cell after re-entry appears immediately, not after a second hover.
7. Move rapidly, stop abruptly, and confirm the final reported cell is still the visible one.

## Regression Checks

### Selection

1. Drag to create a text selection inside the terminal.
2. Confirm the highlight follows the pointer without stale cells.
3. Clear the selection and move the pointer away.
4. Expected: no selection-colored cells remain stuck.

### Scrollbar and Chrome

1. Hover the scrollbar without entering the terminal body.
2. Drag the scrollbar.
3. Hover the titlebar and panel chrome.
4. Expected: only scrollbar/titlebar visuals change; these interactions should not look like stale terminal-body cache reuse.

### Streaming Output

1. Temporarily stop the helper and run a short streaming command such as:
   ```bash
   python3 - <<'PY'
   import time
   for i in range(20):
       print(f"line {i:02d}")
       time.sleep(0.1)
   PY
   ```
2. Check once with the pointer outside the body and once with it inside.
3. Expected: new output appears promptly in both cases.

### Multi-Panel Check

If practical, add a second panel and keep both visible.

- Hover one panel while the other emits output.
- Sweep the pointer across empty canvas with both panels idle.
- Expected: both panels stay visually correct; non-hovered panels do not flicker or regress.

## Expected Result

- Hovering the terminal body always shows live mouse-report updates.
- Empty-canvas movement does not disturb terminal rendering.
- Scrollbar-only and titlebar-only hover do not regress.
- Selection, scrolling, and streaming output remain correct.
- No crashes, panics, or rendering warnings appear during the pass.

## Cleanup

1. Exit the helper with `q` so mouse reporting is disabled.
2. Close only the Horizon instance launched for this smoke test.
3. Remove the isolated runtime root:
   ```bash
   rm -rf "$SMOKE_ROOT"
   ```
