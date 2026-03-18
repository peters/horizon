---
name: horizon-notify
description: Notify the Horizon workspace user about completed work, findings, or needed decisions. Only works when HORIZON env var is set.
user-invocable: false
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

Related OSC API for terminal titles:

- Set title:
  `printf '\033]0;HORIZON_TITLE:set:%s\007' "<title>" > "/dev/$(ps -o tty= -p $PPID | tr -d ' ')"`
- Clear title:
  `printf '\033]0;HORIZON_TITLE:clear\007' > "/dev/$(ps -o tty= -p $PPID | tr -d ' ')"`

Keep titles under 80 chars and avoid newlines.
