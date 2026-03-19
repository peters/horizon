# Transcript Replay Restart Smoke Test

## Goal

Validate that Horizon can restore shell and command panel transcripts after a full app restart without injecting replay-time terminal replies into the newly spawned PTY.

The regression this plan targets is raw control-reply text appearing at the prompt after relaunch, for example:

- `^[[7;1R`
- `rgb:....`
- literal `^[` / `^[]11;...`
- unexpected prompt corruption immediately after the restored history

## Scope

Cover:

1. Basic transcript restore on restart.
2. Replay of control-query bytes that trigger PTY writebacks.
3. Shell and command panels.
4. Mixed history with alternate-screen programs.
5. Persistence of visible panel state after restart.
6. Visual confirmation that restored panels remain clean and aligned.

## Environment Setup

Use an isolated home directory so the test does not touch existing Horizon sessions or config.

```bash
cd /home/peters/github/horizon-transcript-replay
export HORIZON_SMOKE_HOME="$(mktemp -d /tmp/horizon-transcript-smoke.XXXXXX)"
export HOME="$HORIZON_SMOKE_HOME"
export SHELL="${SHELL:-/bin/bash}"
cargo run --release -p horizon-ui
```

After Horizon launches, keep using the same `HOME` value for every relaunch in this plan.

## Probe Commands

Run these inside a Horizon terminal panel. They intentionally emit terminal queries and consume the live replies so the query bytes land in the transcript while the live session stays clean.

### Cursor Position Probe

```bash
python3 - <<'PY'
import os, select, sys, termios, tty
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
try:
    tty.setcbreak(fd)
    os.write(sys.stdout.fileno(), b"\x1b[6n")
    if select.select([fd], [], [], 1)[0]:
        os.read(fd, 64)
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
PY
```

### Background Color Probe

```bash
python3 - <<'PY'
import os, select, sys, termios, tty
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
try:
    tty.setcbreak(fd)
    os.write(sys.stdout.fileno(), b"\x1b]11;?\a")
    if select.select([fd], [], [], 1)[0]:
        os.read(fd, 128)
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
PY
```

## Test Cases

### 1. Baseline Fresh Launch

1. Launch Horizon with the isolated `HOME`.
2. Create one shell panel.
3. Run:

```bash
printf 'baseline-one\n'
pwd
```

4. Confirm the prompt is clean before any restart.

Expected:

- No raw escape text is visible.
- The prompt is aligned on a normal line.
- Cursor placement is correct.

### 2. Simple Restart Restore

1. In the same shell panel, run:

```bash
printf 'simple-restore\n'
ls
```

2. Fully quit Horizon.
3. Relaunch Horizon with the same `HOME`.

Expected:

- The shell panel restores previous visible output.
- The prompt appears once at the bottom of restored history.
- No `^[[`, `rgb:`, `R`, or other control garbage appears.

### 3. Cursor Position Replay Regression

1. In a shell panel, run the Cursor Position Probe.
2. Then run:

```bash
printf 'after-dsr\n'
```

3. Quit Horizon.
4. Relaunch with the same `HOME`.

Expected:

- The restored panel does not show `^[[<row>;<col>R`.
- The prompt remains clean immediately after the replayed history.
- Typing a new command works normally and does not start with stray control bytes.

### 4. Background Color Replay Regression

1. In a shell panel, run the Background Color Probe.
2. Then run:

```bash
printf 'after-osc11\n'
```

3. Quit Horizon.
4. Relaunch with the same `HOME`.

Expected:

- The restored panel does not show `rgb:` data or literal OSC text.
- No prompt corruption appears after replay.
- New typed input is clean.

### 5. Mixed Query History

1. In a shell panel, run both probes back to back.
2. Then run:

```bash
printf 'mixed-probes-complete\n'
```

3. Quit Horizon.
4. Relaunch with the same `HOME`.

Expected:

- No cursor-report reply text.
- No color-report reply text.
- The prompt and scrollback remain readable.

### 6. Alternate-Screen Followed by Restart

1. In a shell panel, run:

```bash
man bash
```

2. Scroll a little inside `man`, then quit it.
3. Run the Cursor Position Probe.
4. Quit Horizon.
5. Relaunch with the same `HOME`.

Expected:

- The restored panel is not stuck in an alternate-screen view.
- The prompt is visible and usable.
- No leftover cursor-report text is injected at the prompt.

### 7. Command Panel Coverage

1. Create a command panel that runs:

```bash
bash -lc 'python3 - <<'"'"'PY'"'"'
import os, select, sys, termios, tty
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
try:
    tty.setcbreak(fd)
    os.write(sys.stdout.fileno(), b"\x1b[6n")
    if select.select([fd], [], [], 1)[0]:
        os.read(fd, 64)
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
PY
printf "command-panel-done\n"
exec "$SHELL" -l'
```

2. Wait until the command panel drops into the login shell.
3. Quit Horizon.
4. Relaunch with the same `HOME`.

Expected:

- The command panel restores without raw cursor-report text.
- The final prompt is clean.
- Restart behavior matches the shell-panel case.

### 8. Multiple Panels and Layout Persistence

1. Create two shell panels.
2. In panel A, run the Cursor Position Probe.
3. In panel B, run the Background Color Probe.
4. Move and resize both panels so the restored layout is easy to verify.
5. Quit Horizon.
6. Relaunch with the same `HOME`.

Expected:

- Both panels restore at the previous positions and sizes.
- Neither panel shows replay-injected control replies.
- No panel loses its cursor or prompt alignment.

### 9. Restart After Shell Exit

1. In a shell panel with visible history, run:

```bash
printf 'before-exit\n'
exit
```

2. Confirm the panel shows shell termination in the current run.
3. Quit Horizon.
4. Relaunch with the same `HOME`.

Expected:

- Restored history is visible.
- No stray control replies are appended after the final visible line.
- Any newly spawned replacement shell prompt is clean.

## Visual Evidence To Capture

Capture screenshots for:

1. First launch before any restart.
2. Relaunch after Test Case 3.
3. Relaunch after Test Case 4.
4. Relaunch after Test Case 8.

Each screenshot should clearly show the bottom of the restored panel and the active prompt.

## Failure Signals

Treat any of the following as a failure:

1. Raw text beginning with `^[[`.
2. Visible `rgb:` payloads.
3. A cursor-position reply like `1;1R`, `7;1R`, or similar appearing in the panel.
4. Prompt text merged into replayed output without a clean line break.
5. Keyboard input starting with unexpected characters after relaunch.
6. Alternate-screen content persisting after relaunch when the app had already exited.

## Optional Triage If A Failure Appears

Use the isolated `HOME` to inspect saved transcripts:

```bash
find "$HOME/.horizon/sessions" -path '*/transcripts/*' -type f -print
```

For a suspicious transcript file:

```bash
python3 - <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
data = path.read_bytes()
print(path)
print(data[-512:])
PY /path/to/transcript.bin
```

Look for query bytes such as `\x1b[6n` or `\x1b]11;?`.

## Exit Cleanup

After the smoke pass is complete:

```bash
rm -rf "$HORIZON_SMOKE_HOME"
```
