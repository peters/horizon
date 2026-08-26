# Smoke-Test Plan — terminal OSC 8 hyperlinks and TUI mouse clicks (temporary)

> Validation artifact for the clickable-TUI fix. Delete after the UI
> validation pass is complete.

Horizon ignored OSC 8 hyperlinks unless the visible cell text itself was a
URL, so Grok labels such as `Terms` did nothing. Clicks in a mouse-reporting
TUI must reach the app. Shift+drag selects text. This plan checks both.

## Shared preconditions

- Build: `cargo build` (debug binary `target/debug/horizon` is sufficient).
- Isolated runtime: use a temp HOME and `--ephemeral` so the tester's real
  `~/.horizon` is untouched:
  ```bash
  SMOKE_HOME=$(mktemp -d)/home && mkdir -p "$SMOKE_HOME/.horizon"
  ```
- Unset `HORIZON` for the child process. Define an explicit disposable
  shell such as `/bin/bash --noprofile --norc`.
- When multiple Horizon processes may be running, scope every window
  inspection and kill to the exact PID under test, never by app name.
- On headless Linux: pick an unused display, `Xvfb :N -screen 0 1600x1000x24`,
  a lightweight WM, launch with `DISPLAY=:N`. Drive with `xdotool` scoped
  to that PID. Screenshot via `import -window <id>` or a PID-scoped capture.

Seed config (`$SMOKE_HOME/.horizon/config.yaml`):

```yaml
version: 9
terminal:
  shell: /bin/bash
  shell_args: ["--noprofile", "--norc"]
workspaces:
  - name: Click Smoke
    terminals:
      - name: clicks
        kind: shell
        position: [80, 80]
        size: [900, 520]
```

Launch:

```bash
HOME="$SMOKE_HOME" HORIZON= RUST_LOG=info target/debug/horizon --config "$SMOKE_HOME/.horizon/config.yaml" --ephemeral
```

## Baseline

1. Launch. Expect the window mapped, one workspace, one bash panel focused
   with a prompt. Screenshot after launch.
2. Type `echo hello` and press Enter. Expect the line to appear. This
   confirms the panel is a live PTY, not a snapshot.

## Lane A — OSC 8 labeled hyperlink (no visible URL)

3. In the panel, run:

   ```bash
   printf 'Read \033]8;;https://example.com/terms\007Terms\033]8;;\007 and \033]8;;https://example.com/privacy\007Privacy Policy\033]8;;\007\n'
   ```

   Expect `Terms` and `Privacy Policy` to render (often underlined). The
   visible text must **not** include `https://`.

4. Hover the pointer over `Terms` with no modifier keys. Expect a pointing
   hand cursor.

5. Left-click `Terms` (no Ctrl/Cmd). Expect the desktop handler to open
   `https://example.com/terms` (or an equivalent attempt: `xdg-open` /
   `open` in logs). The panel must not start a text selection on that click.

6. Hover `echo hello` from step 2 with no modifiers. Expect the normal
   text cursor, not a pointing hand.

7. Ctrl+click (Cmd+click on macOS) `Privacy Policy`. Expect
   `https://example.com/privacy` to open.

8. Shift+click-drag across `Terms`. Expect local text selection, and the
   browser must **not** open from that gesture.

## Lane B — mouse reporting (TUI buttons)

9. In the same panel, run a reporter that enables SGR mouse mode and
   prints received sequences:

   ```bash
   python3 - <<'PY'
   import sys, tty, termios, select
   fd = sys.stdin.fileno()
   old = termios.tcgetattr(fd)
   sys.stdout.write("\x1b[?1000h\x1b[?1002h\x1b[?1006h")
   sys.stdout.write("SGR mouse on. Click [Opt in] below, then press q.\n")
   sys.stdout.write("[Opt out]   [Opt in]\n")
   sys.stdout.flush()
   tty.setraw(fd)
   buf = b""
   try:
       while True:
           ready, _, _ = select.select([fd], [], [], 30)
           if not ready:
               sys.stdout.write("\r\nTIMEOUT\r\n")
               break
           buf += sys.stdin.buffer.read(1)
           if buf.endswith(b"q"):
               break
           if b"\x1b[<" in buf and buf.endswith((b"M", b"m")):
               sys.stdout.buffer.write(b"\r\nGOT " + buf.replace(b"\x1b", b"<ESC>") + b"\r\n")
               sys.stdout.flush()
               buf = b""
   finally:
       termios.tcsetattr(fd, termios.TCSADRAIN, old)
       sys.stdout.write("\x1b[?1000l\x1b[?1002l\x1b[?1006l\n")
       sys.stdout.flush()
   PY
   ```

   If `python3` is missing, use:

   ```bash
   printf '\033[?1000h\033[?1002h\033[?1006hSGR mouse on. Click, then Ctrl-C.\n[Opt out]   [Opt in]\n'
   cat -v
   ```

10. Left-click `[Opt in]`. Expect an SGR report such as
    `\x1b[<0;<col>;<row>M` followed by a release `m`. The click must **not**
    leave a Horizon selection highlight.

11. Shift+click-drag across `[Opt out]`. Expect local selection. The
    reporter must **not** receive that press.

12. Press `q` or Ctrl-C to restore the terminal. Confirm mouse reporting is
    off (`printf '\033[?1000l\033[?1002l\033[?1006l'` if needed).

## Lane C — Grok banner (if `grok` is on PATH)

13. In a **new** terminal panel (or replace the shell with `grok`), launch
    Grok Build so the "Help improve Grok" banner is visible, including
    `Terms`, `Privacy Policy`, `[Opt out]`, and `[Opt in]`.

14. Hover `Terms`. Expect a pointing hand.

15. Left-click `Terms`. Expect the Terms page to open in the default
    browser. Nothing happening is a fail.

16. Left-click `[Opt in]` (or `[Opt out]`). Expect Grok to accept the
    choice (banner dismisses or settings update). A no-op click is a fail.

17. Shift+click-drag across banner text. Expect local selection.

## Lane D — regressions

18. In a normal shell (mouse reporting off), click-drag to select `echo hello`.
    Expect the usual selection highlight and Ctrl+Shift+C / primary copy.

19. Middle-click paste on Linux still pastes PRIMARY when mouse reporting
    is off.

20. After all of the above, screenshot the panel. Close the window through
    the window-manager close path. Confirm the smoke PID exited. Remove
    `$SMOKE_HOME` and any screenshot/video proof files.

## Pass criteria

- Lanes A and B must pass on the candidate binary.
- Lane C is required when `grok` is installed; otherwise mark skipped.
- Lane D must not regress selection or middle-click paste.
