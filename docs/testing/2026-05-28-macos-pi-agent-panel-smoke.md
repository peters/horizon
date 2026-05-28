# macOS Pi Agent Panel Smoke Plan

## Scope

Validate the Pi agent panel integration on macOS from a clean user runtime. Horizon should shell out to a user-installed `pi` CLI, create fresh Pi panels with `pi`, resume exact sessions with `pi --session <id>`, and keep existing agent presets unchanged.

## Prerequisites

- macOS 14 or newer on Apple Silicon or Intel.
- Xcode Command Line Tools installed:
  ```bash
  xcode-select --install
  ```
- Build Horizon from the PR branch:
  ```bash
  cargo build
  ```

## Install Pi Runtime

The smoke-test agent must install the Pi runtime themselves. Horizon does not install or vendor Pi.

1. Install Node.js `22.19.0` or newer. Using `nvm`:
   ```bash
   brew install nvm
   mkdir -p ~/.nvm
   export NVM_DIR="$HOME/.nvm"
   . "$(brew --prefix nvm)/nvm.sh"
   nvm install 22.19.0
   nvm use 22.19.0
   ```

2. Install the Pi coding-agent package:
   ```bash
   npm install -g @earendil-works/pi-coding-agent
   ```

3. Verify the CLI resolves:
   ```bash
   node --version
   pi --version
   command -v pi
   ```

4. For repeatable smoke runs, disable Pi network side effects unless the test explicitly covers them:
   ```bash
   export PI_SKIP_VERSION_CHECK=1
   export PI_TELEMETRY=0
   ```

## Isolated Runtime Setup

Use an isolated home directory so the test does not mutate the tester's real Horizon or Pi state.

```bash
export SMOKE_HOME="$(mktemp -d /tmp/horizon-pi-smoke.XXXXXX)"
mkdir -p "$SMOKE_HOME/.horizon" "$SMOKE_HOME/.pi/agent/sessions/project"
cat > "$SMOKE_HOME/.horizon/config.yaml" <<'YAML'
version: 7
window:
  width: 1280
  height: 860
presets:
  - name: Shell
    alias: sh
    kind: shell
workspaces:
  - name: Pi Smoke
    cwd: /tmp
    terminals:
      - name: Pi Smoke
        kind: pi
        resume: last
YAML
cat > "$SMOKE_HOME/.pi/agent/sessions/project/session-123.jsonl" <<'JSONL'
{"type":"session","id":"session-123","cwd":"/tmp"}
{"type":"user_message","text":"Smoke Pi panel"}
JSONL
```

## Launch And Visual Checks

```bash
HOME="$SMOKE_HOME" RUST_LOG=horizon=info,horizon_core=info target/debug/horizon --config "$SMOKE_HOME/.horizon/config.yaml" --ephemeral
```

Verify:

- Horizon opens without crashing.
- The config migrates to `version: 8`.
- The panel badge reads `PI`.
- The Pi panel is visible in the `Pi Smoke` workspace.
- The terminal launches the installed Pi TUI. If Pi requires auth, it may show its normal auth/setup prompt; Horizon must remain responsive.
- The Pi preset appears exactly once in settings or the panel picker.

## Resume Checks

With the seeded session file still present, relaunch Horizon and confirm the log contains:

```text
kind=Pi resume=Last session_id="session-123"
cmd=... pi --session session-123
```

Then create a fresh Pi panel from the picker and confirm the launch command is plain `pi` with no implicit `--offline` or session flag.

## Interaction Checks

- Type into the Pi panel and confirm keyboard input reaches the TUI.
- Resize the panel and confirm the terminal grid redraws cleanly.
- Restart the Pi panel and confirm it keeps the selected working directory.
- Save runtime state, quit, relaunch, and confirm the Pi panel restores as kind `pi` with the same session binding.

## Missing Runtime Check

Temporarily remove Pi from `PATH`:

```bash
PATH="/usr/bin:/bin:/usr/sbin:/sbin" HOME="$SMOKE_HOME" target/debug/horizon --config "$SMOKE_HOME/.horizon/config.yaml" --ephemeral
```

Verify Horizon shows the normal terminal command failure for `pi` and does not crash.

## Evidence To Attach To PR

- `cargo build` result.
- A screenshot after launch showing the `PI` badge and Pi terminal.
- Log lines proving `pi --session session-123` for the seeded session.
- A short note saying whether Pi reached the TUI or stopped at its expected auth/setup prompt.
