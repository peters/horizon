# Legacy C0 Keyboard Smoke Test

## Goal

Validate the legacy keyboard-input parity fix for control-style keys that do
not rely on text events:

- `Enter`
- `Escape`
- `Backspace`
- `Tab`

The regression family was that modified variants of these keys could be
dropped or mis-encoded before Horizon forwarded them to the PTY when the
foreground program had not enabled kitty keyboard reporting.

## Expected Result Summary

Pass criteria:

1. Modified legacy C0 keys reach the PTY in a normal shell panel even when the
   foreground program is using legacy keyboard input.
2. The raw bytes match the expected legacy encodings listed below.
3. Plain shell behavior remains correct after the fix:
   - `Enter` still submits commands
   - `Backspace` still deletes normally
   - `Shift+Tab` still reports backtab
4. No visual regression appears in panel focus, rendering, resize, or restore
   flows.

## Test Environment

- Run from the exact worktree/branch that contains the fix.
- Prefer X11 with `DISPLAY=:1`.
- Use a temporary `HOME` and isolated config path.
- Use a single shell panel so the key path is unambiguous.
- If the window manager reserves `Alt+Tab` or `Alt+Shift+Tab`, record that as
  an environment limitation and treat those cases as automation-only.

## Seeded Config

Use this config so the run starts from a deterministic single-panel layout:

```yaml
window:
  width: 1200
  height: 820
  x: 120
  y: 80
workspaces:
  - name: Smoke
    position: [0, 40]
    terminals:
      - name: Shell
        kind: shell
        position: [40, 80]
        size: [520, 320]
```

## Launch Procedure

1. Create a temporary directory for the smoke run.
2. Write the seeded config into that directory.
3. Launch Horizon from the issue worktree with the temporary `HOME`.
4. Confirm a single shell panel is visible and focused after clicking it.

Example:

```bash
TMPDIR="$(mktemp -d /tmp/horizon-legacy-c0-smoke-XXXXXX)"
mkdir -p "$TMPDIR/home"
cat > "$TMPDIR/config.yaml" <<'EOF'
window:
  width: 1200
  height: 820
  x: 120
  y: 80
workspaces:
  - name: Smoke
    position: [0, 40]
    terminals:
      - name: Shell
        kind: shell
        position: [40, 80]
        size: [520, 320]
EOF

cd /absolute/path/to/the/worktree
HOME="$TMPDIR/home" DISPLAY=:1 cargo run -p horizon-ui -- --new-session --config "$TMPDIR/config.yaml"
```

## Evidence To Capture

Capture:

- screenshot immediately after launch
- screenshot while running the raw-byte helper
- screenshot after the resize pass
- screenshot after relaunch
- written notes for any mismatch including:
  - key combo
  - observed bytes
  - expected bytes
  - whether the combo was manual or automation-injected
  - whether the window manager intercepted it

## Raw Byte Capture Helper

Inside the Horizon shell panel, define this helper:

```bash
capture_key() {
python3 - <<'PY'
import os
import sys
import termios
import tty

fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
print("Press the target key combo now...", flush=True)
try:
    tty.setraw(fd)
    data = os.read(fd, 8)
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)

print("bytes:", " ".join(f"{byte:02x}" for byte in data), flush=True)
PY
}
```

Use `capture_key` before each manual key check below.

## Expected Legacy Bytes

Verify these exact byte sequences in the shell panel:

| Key combo | Expected bytes |
|-----------|----------------|
| `Enter` | `0d` |
| `Shift+Enter` | `0d` |
| `Alt+Enter` | `1b 0d` |
| `Escape` | `1b` |
| `Shift+Escape` | `1b` |
| `Ctrl+Escape` | `1b` |
| `Alt+Escape` | `1b 1b` |
| `Backspace` | `7f` |
| `Shift+Backspace` | `7f` |
| `Ctrl+Backspace` | `08` |
| `Alt+Backspace` | `1b 7f` |
| `Tab` | `09` |
| `Ctrl+Tab` | `09` |
| `Shift+Tab` | `1b 5b 5a` |
| `Ctrl+Shift+Tab` | `1b 5b 5a` |
| `Alt+Shift+Tab` | `1b 1b 5b 5a` |

Notes:

- `Alt+Tab` and `Alt+Shift+Tab` may be reserved by the window manager when
  pressed manually. If so, record that and use automation if available.
- The fix does not change kitty/disambiguate behavior. This smoke pass is about
  legacy-mode forwarding in a normal shell.

## Core Manual Checks

1. Run `capture_key`, then press `Enter`.
2. Repeat for `Shift+Enter`.
3. Repeat for `Escape`.
4. Repeat for `Shift+Escape`.
5. Repeat for `Ctrl+Escape`.
6. Repeat for `Backspace`.
7. Repeat for `Shift+Backspace`.
8. Repeat for `Ctrl+Backspace`.
9. Repeat for `Tab`.
10. Repeat for `Ctrl+Tab`.
11. Repeat for `Shift+Tab`.
12. Repeat for `Ctrl+Shift+Tab`.

Expected result:

- Each combo prints the expected bytes from the table.
- No combo silently produces no output.

## Alt-Modified Checks

If the environment allows the window manager to pass Alt combinations through,
run `capture_key` and test:

- `Alt+Enter`
- `Alt+Escape`
- `Alt+Backspace`
- `Alt+Shift+Tab`

If the environment intercepts one of these:

1. Record the interception in the notes.
2. If available, use targeted automation such as `xdotool key --window`.
3. If automation is not available, rely on the automated test coverage for that
   combo and mark the live case as environment-blocked rather than failed.

## Shell Behavior Checks

At a normal prompt, verify user-facing behavior:

1. Type `printf 'enter-ok\n'` and press `Enter`.
2. Type the same command again and press `Shift+Enter`.
3. Type `abc`, then press `Backspace` once.
4. Type `abc`, then press `Shift+Backspace` once.
5. Run `bind -q backward-word >/dev/null 2>&1 || true` only to ensure the shell
   stays responsive after the modifier checks.

Expected result:

- Plain and shifted Enter both submit the command.
- Plain and shifted Backspace both delete one character.
- The shell remains interactive and stable.

## Resize And Focus Checks

1. Resize the Horizon window larger, then smaller.
2. Re-run these representative raw-byte checks:
   - `Shift+Enter`
   - `Ctrl+Escape`
   - `Ctrl+Backspace`
   - `Ctrl+Shift+Tab`
3. Click away and back inside the terminal panel.
4. Confirm the panel still accepts keyboard input immediately after refocus.

Expected result:

- Byte output does not change after resize.
- Refocus does not reintroduce dropped key events.
- No visual glitch appears in panel chrome or terminal rendering.

## Persistence Check

1. Close Horizon cleanly.
2. Relaunch with the same temporary `HOME` and config path.
3. Re-run these representative checks:
   - `Shift+Enter` => `0d`
   - `Alt+Escape` => `1b 1b` if the environment permits
   - `Ctrl+Backspace` => `08`
   - `Shift+Tab` => `1b 5b 5a`
4. Capture the relaunch screenshot.

Expected result:

- The restored session still forwards the fixed key sequences correctly.
- No startup or restore regression appears.

## Visual Review

The screenshots should confirm:

- the shell panel remains visible and readable
- the cursor is visible when expected
- resize does not corrupt panel chrome or terminal content
- no unexpected overlay or focus artifact appears during the test

## Failure Logging

For every failure, record:

- exact key combo
- expected bytes
- observed bytes
- whether Horizon dropped the event entirely
- whether the mismatch happened only after resize or relaunch
- whether the window manager intercepted the combo

## Cleanup

1. Quit Horizon.
2. Save screenshots and notes with the smoke result.
3. Keep the temporary config directory only if follow-up debugging is needed.
