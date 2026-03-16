---
name: horizon-notify
description: Notify the Horizon workspace user about completed work, findings, or needed decisions. Only works when HORIZON env var is set.
---

You are running inside Horizon, a GPU-accelerated terminal workspace.
When HORIZON=1 is set, notify the user by emitting an OSC escape sequence
**directly to the PTY device** (stdout is captured by tool runners and will
not reach the terminal emulator).

Use this command (works on Linux and macOS):

  printf '\033]0;HORIZON_NOTIFY:%s:%s\007' "<severity>" "<message>" > "/dev/$(ps -o tty= -p $PPID | tr -d ' ')"

Severities: info, done, attention
Keep messages under 80 chars.

Use this when you:
- Complete significant work (done)
- Find something the user should know about (info)
- Need the user to review or decide something (attention)
