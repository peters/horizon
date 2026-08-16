# Smoke Test: HOME/END keyboard shortcuts in kitty-keyboard-protocol panels

Temporary validation plan for the fix that makes Horizon emit spec-compliant
kitty sequences for unmodified HOME/END (`CSI H` / `CSI F` instead of the
non-standard `CSI 1 H` / `CSI 1 F`). pi (and other spec-conformant parsers)
ignore `CSI 1 H`/`CSI 1 F` entirely, so HOME/END appeared dead inside a pi
panel while arrows and modified chords worked.

**Scope:** terminal keyboard input path only. No rendering changes, so the
visual checks below are launch regressions, not layout checks.

**Target OS:** any (X11/Wayland/macOS). The steps below use X11 + xdotool;
on Wayland use `ydotool` or manual key presses, on macOS use AppleScript or
manual key presses.

## Prerequisites

- A display session (X11 or Wayland) with a window manager.
- `cargo` (Rust stable >= 1.88) and `xdotool` (X11 only).
- The pi CLI installed and on `PATH` (for scenario 4). Optional but recommended.

## Setup (once)

```bash
cargo build          # debug binary: target/debug/horizon
```

Run Horizon in an **isolated runtime state** so you never touch a real
session. On Linux/X11:

```bash
mkdir -p /tmp/hz-smoke-home
env -i HOME=/tmp/hz-smoke-home PATH="$PATH" SHELL=/bin/bash \
  DISPLAY="$DISPLAY" TERM=xterm-256color \
  target/debug/horizon > /tmp/hz-smoke.log 2>&1 &
HZ_PID=$!
```

Identify the new window by PID (never by title — other Horizon instances may
be running):

```bash
for w in $(xwininfo -root -tree | grep -oE '0x[0-9a-f]+ "Horizon"' | awk '{print $1}'); do
  [ "$(xprop -id $w _NET_WM_PID | sed 's/.*= //')" = "$HZ_PID" ] && echo "WINDOW: $w"
done
```

All `xdotool` key events below target the active window, so activate the test
window first: `xdotool windowactivate --sync $WIN` (click the window if the
WM refuses to move focus).

## Scenario 1 — byte-level check in a kitty-protocol panel (the pi path)

pi enables the kitty keyboard protocol with flags 7 (disambiguate + event
types + alternate keys). A panel whose process does the same and hexdumps
stdin reproduces the bug exactly.

Create the probe:

```bash
mkdir -p /tmp/hz-smoke
cat > /tmp/hz-smoke/keyspy.py <<'PY'
import os, sys, termios, time
out = sys.stdout
out.write("\x1b[2J\x1b[Hkeyspy ready\n")
out.write("\x1b[>7u")          # kitty keyboard protocol, same flags as pi
out.flush()
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
tty = termios.tcgetattr(fd)
tty[3] = tty[3] & ~(termios.ECHO | termios.ICANON | termios.ISIG | termios.IEXTEN)
termios.tcsetattr(fd, termios.TCSANOW, tty)
try:
    deadline = time.monotonic() + 600
    while time.monotonic() < deadline:
        try:
            data = os.read(fd, 1024)
        except OSError:
            time.sleep(0.05)
            continue
        if not data:
            break
        out.write("KEY: " + data.hex(" ") + "\n")
        out.flush()
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
PY
```

Seed a session panel that runs the probe. Stop the Horizon instance, then in
`$HOME/.horizon/sessions/<sid>/runtime.yaml` set the first workspace's
`panels` to a single shell panel with `command: python3` and
`args: [/tmp/hz-smoke/keyspy.py]` (copy the panel schema from any existing
session's `runtime.yaml`; keep `kind: shell`, `resume: fresh`). Restart
Horizon the same way as in Setup and wait for the panel to show `keyspy
ready`. **Important:** seed the file *after* every Horizon instance for that
HOME has exited — a running instance overwrites the file on exit.

Activate the test window, click inside the panel body, then:

```bash
xdotool key Home
xdotool key End
xdotool key shift+Home
```

**Expected (fixed):** the panel shows lines like

```
KEY: 1b 5b 48          # \x1b[H   Home press  (regression: was 1b 5b 31 48 = \x1b[1H)
KEY: 1b 5b 31 3b 31 3a 33 48   # \x1b[1;1:3H  Home release (unchanged by fix)
KEY: 1b 5b 46          # \x1b[F   End press   (regression: was 1b 5b 31 46 = \x1b[1F)
KEY: 1b 5b 31 3b 31 3a 33 46   # \x1b[1;1:3F  End release (unchanged by fix)
KEY: 1b 5b 31 3b 32 48  # \x1b[1;2H  Shift+Home (unchanged by fix)
```

**Failure signature (the bug):** Home press shows `1b 5b 31 48` (`\x1b[1H`)
and/or End press shows `1b 5b 31 46` (`\x1b[1F`).

## Scenario 2 — arrow-key and text regressions (same panel)

In the same keyspy panel:

```bash
xdotool key Left
xdotool key shift+Right
xdotool type xy
xdotool key BackSpace
```

**Expected:**

```
KEY: 1b 5b 44          # \x1b[D   Left press (unchanged by fix)
KEY: 1b 5b 31 3b 32 43 # \x1b[1;2C Shift+Right press (unchanged by fix)
KEY: 78                # 'x' — plain text stays on the raw UTF-8 path
KEY: 79                # 'y'
KEY: 1b 5b 31 32 37 75 # \x1b[127u kitty Backspace press (unchanged by fix)
```

**Failure signature:** arrows or letters gain a `1` prefix, or letters arrive
as `CSI u` sequences.

## Scenario 3 — legacy (non-kitty) shell panel

Create a second panel running a plain shell (default `$SHELL`) in the same
workspace. In it, run:

```sh
stty raw -echo; cat -v
```

Then press Home, Shift+Home, End, and press `Ctrl+C` followed by
`stty sane; echo` to restore.

**Expected (screen output, `^ [` = ESC):**

```
^[H     # Home (unchanged by fix — legacy never used the "1" prefix)
^[1;2H  # Shift+Home (unchanged)
^[F     # End (unchanged)
```

**Failure signature:** `^[1H` / `^[1F` appearing in a *non*-kitty shell.

## Scenario 4 — pi panel, user-visible cursor movement

Create a panel running `pi` (agent kind "pi", or shell panel with
`command: pi`). Wait for the pi prompt, click the panel, type a visible
string such as `hello world`, then:

1. Press **Home** — the cursor (block/beam in the pi input line) jumps to the
   start of `hello world`.
2. Press **End** — it jumps to the end.
3. Press **Shift+Home** — the pi input selects/extends to the start.

**Expected:** the cursor moves as described. With the bug present, Home/End
do nothing (pi's parser returns undefined for `CSI 1 H`/`CSI 1 F`) while
arrows still work.

Take a screenshot after step 2 as PR evidence.

## Scenario 5 — app-cursor mode (vim)

In a shell panel run `vim /tmp/hz-smoke/scratch.txt`, enter insert mode
(`i`), type `abcdef`, then press Home and End.

**Expected:** Home moves to column 1, End to the end of the line (app-cursor
path `SS3 H`/`SS3 F`, unchanged by the fix).

## Scenario 6 — launch visual regression

At Horizon launch (before any key input): one workspace renders, the panel
shows its terminal content, no crash in `/tmp/hz-smoke.log`
(`grep -i "panic\|error"` should show nothing new). Screenshot for the PR.

## Cleanup

```bash
kill $HZ_PID        # or close the window
rm -rf /tmp/hz-smoke /tmp/hz-smoke-home /tmp/hz-smoke.log
```

## Pass criteria

- Scenario 1: Home/End press bytes are `1b 5b 48` / `1b 5b 46` (no `1`
  prefix), releases and Shift+Home unchanged.
- Scenario 2: arrows/text/Backspace unchanged.
- Scenario 3: legacy sequences unchanged.
- Scenario 4: pi cursor moves with Home/End.
- Scenario 5: vim Home/End unchanged.
- Scenario 6: clean launch.
