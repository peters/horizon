# Shell Login Smoke Test Plan

Use this plan to validate the `PanelKind::Shell` login-shell change in the
branch under test. This is a correctness-sensitive smoke pass for the
macOS/BSD regression where explicit custom shell commands with no args could be
launched as `<command> -l`.

## Target Platform

Run the full pass on one of:

1. macOS
2. FreeBSD
3. OpenBSD
4. NetBSD
5. DragonFly BSD

Linux can be used as a control run, but it is not sufficient for final sign-off
because the login-shell injection path is disabled there.

## Evidence To Capture

Capture and keep:

1. One screenshot of the first launch with all smoke-test panels visible
2. One screenshot after creating an additional wrapper shell panel from the UI
3. One screenshot after quitting and relaunching with the same isolated home
4. The contents of every file under `$SMOKE_HOME/.horizon/smoke/`

## Environment Setup

Run the branch under test with an isolated `HOME` so the smoke pass does not
reuse an existing Horizon config or session:

```bash
export SMOKE_HOME=/tmp/horizon-login-shell-smoke
rm -rf "$SMOKE_HOME"
mkdir -p "$SMOKE_HOME/.horizon" "$SMOKE_HOME/bin"
```

Create a wrapper executable that logs its argv exactly as Horizon launches it:

```bash
cat > "$SMOKE_HOME/bin/argv-dump.sh" <<'EOF'
#!/bin/sh
set -eu

log_dir="$HOME/.horizon/smoke"
mkdir -p "$log_dir"
log_file=$(mktemp "$log_dir/launch.XXXXXX.log")

printf 'argv0=%s\n' "$0" > "$log_file"
printf 'argc=%s\n' "$#" >> "$log_file"

i=1
for arg in "$@"; do
  printf 'arg%s=%s\n' "$i" "$arg" >> "$log_file"
  i=$((i + 1))
done

printf 'cwd=%s\n' "$PWD" >> "$log_file"
printf 'WRAPPER_READY\n'

exec sleep 300
EOF
chmod +x "$SMOKE_HOME/bin/argv-dump.sh"
```

Create a dedicated smoke-test config:

```bash
cat > "$SMOKE_HOME/.horizon/config.yaml" <<EOF
presets:
  - name: Wrapper Shell
    alias: ws
    kind: shell
    command: $SMOKE_HOME/bin/argv-dump.sh
    resume: fresh
  - name: Shell
    alias: sh
    kind: shell
    resume: fresh
workspaces:
  - name: Login Shell Smoke
    terminals:
      - name: Default Shell
        kind: shell
        position: [120.0, 120.0]
        size: [520.0, 340.0]
      - name: Custom Wrapper
        kind: shell
        command: $SMOKE_HOME/bin/argv-dump.sh
        position: [700.0, 120.0]
        size: [520.0, 340.0]
      - name: Custom Wrapper With Arg
        kind: shell
        command: $SMOKE_HOME/bin/argv-dump.sh
        args: ["--sentinel"]
        position: [1280.0, 120.0]
        size: [520.0, 340.0]
EOF
```

Launch Horizon from the branch under test:

```bash
cd /path/to/horizon
HOME="$SMOKE_HOME" cargo run --release
```

## Baseline

1. Confirm Horizon opens one workspace named `Login Shell Smoke`.
2. Confirm three panels are visible on first launch:
   `Default Shell`, `Custom Wrapper`, and `Custom Wrapper With Arg`.
3. Confirm neither wrapper panel exits immediately or shows a launch failure.
4. Capture the first-launch screenshot.
5. In another terminal, inspect the wrapper logs:

```bash
ls -1 "$SMOKE_HOME/.horizon/smoke"
for file in "$SMOKE_HOME"/.horizon/smoke/*; do
  echo "== $file =="
  cat "$file"
done
```

## Primary Checks

### Default Shell Still Uses Login-Shell Startup

1. Focus the `Default Shell` panel.
2. Run:

```sh
printf 'argv0=%s\n' "$0"
case "$0" in
  -*) echo 'LOGIN_SHELL=yes' ;;
  *) echo 'LOGIN_SHELL=no' ;;
esac
```

3. Expected on macOS/BSD: `LOGIN_SHELL=yes`.
4. Record the visible output in the test notes.

### Custom Shell With No Args Must Not Receive `-l`

1. Find the wrapper log whose contents include `argc=0`.
2. Confirm the file does not contain any `arg1=-l` line.
3. Confirm the `Custom Wrapper` panel stayed alive and printed `WRAPPER_READY`.

### Custom Shell With Explicit Args Must Preserve Only Its Own Args

1. Find the wrapper log whose contents include `argc=1`.
2. Confirm it contains exactly `arg1=--sentinel`.
3. Confirm it does not contain `arg2=-l` or any other injected argument.

## UI Creation Flow

This verifies the same behavior through the preset-to-panel path instead of only
the startup config path.

1. With Horizon still running, press `Ctrl+N`.
2. Confirm a new `Wrapper Shell` panel appears.
3. Re-run the log inspection command in another terminal.
4. Confirm at least one new wrapper log shows `argc=0`.
5. Confirm none of the wrapper logs contain `arg1=-l`.
6. Capture a screenshot with the newly created wrapper panel visible.

## Edge Cases

1. Use `Ctrl+double-click` on empty canvas to open the preset picker.
2. Choose `Shell`.
3. In the newly created default shell panel, run the same login-shell command
   from the primary checks section and confirm `LOGIN_SHELL=yes`.
4. Resize the Horizon window noticeably narrower and wider.
5. Confirm all wrapper panels remain visible and do not relaunch, crash, or
   render blank after resize.

## Persistence And Runtime Restore

1. With the original three panels plus the newly created panel(s) still open,
   close Horizon normally.
2. Relaunch with the same command:

```bash
cd /path/to/horizon
HOME="$SMOKE_HOME" cargo run --release
```

3. Confirm the same panel set returns after relaunch.
4. Confirm the restored custom wrapper panel(s) still stay alive.
5. Re-run the log inspection command and confirm new wrapper log files were
   created for the relaunch.
6. Confirm every new wrapper log still omits `-l` for the zero-arg wrapper
   launch.
7. Confirm the `Default Shell` panel still reports `LOGIN_SHELL=yes`.
8. Capture the relaunch screenshot.

## Visual Regression Checks

1. Confirm panel titles remain readable and no panel body is blank on launch or
   relaunch.
2. Confirm no wrapper panel is replaced by a short-lived crashed panel or an
   empty placeholder.
3. Confirm the layout remains stable after window resize and relaunch.
4. Compare the three screenshots and verify there is no missing panel chrome,
   clipped content, or unexpected panel disappearance.

## Failure Signatures

Fail the smoke pass if any of the following happen:

1. A wrapper log contains `arg1=-l` for a `kind: shell` custom command with no
   explicit args
2. The `Custom Wrapper` panel exits immediately on launch
3. The `Custom Wrapper With Arg` panel receives extra args beyond
   `--sentinel`
4. The default shell no longer reports login-shell startup on macOS/BSD
5. Restored custom shell panels relaunch with `-l` after quitting and
   reopening Horizon
