# macOS Shell Panel CWD Restore Smoke Plan

## Scope

Validate that a shell panel restores its **last working directory** after the user
`cd`s and restarts Horizon, on macOS. This exercises the cross-platform cwd
persistence feature whose Linux path is fixed in this PR. The macOS read path
(`lsof` + deepest-child walk in `crates/horizon-core/src/terminal/support.rs`) is
unchanged by this PR, so this smoke is a regression check that macOS still tracks
and restores the live shell cwd, and that the build is healthy on macOS.

Background: shell/command panels are launched wrapped in `script` for transcript
capture, so the PTY child is the wrapper, not the shell. The cwd reader must look
past the wrapper to the real shell. On macOS this already works via the lsof
path; this plan proves it.

## Prerequisites

- macOS 14 or newer on Apple Silicon or Intel.
- Xcode Command Line Tools:
  ```bash
  xcode-select --install
  ```
- Build Horizon from the PR branch:
  ```bash
  git switch fix/shell-panel-cwd-restore
  cargo build
  ```

## Isolated Runtime Setup

Use an isolated home directory so the test does not mutate the tester's real
Horizon state.

```bash
export SMOKE_HOME="$(mktemp -d /tmp/horizon-cwd-smoke.XXXXXX)"
mkdir -p "$SMOKE_HOME/.horizon"
export CWD_TARGET="$SMOKE_HOME/cd-target"
mkdir -p "$CWD_TARGET"
cat > "$SMOKE_HOME/.horizon/config.yaml" <<YAML
version: 8
window:
  width: 1280
  height: 860
workspaces:
  - name: CWD Smoke
    cwd: $SMOKE_HOME
    terminals:
      - name: Roamer
        kind: shell
      - name: Stayer
        kind: shell
YAML
```

`Roamer` is the panel we will `cd` in. `Stayer` is the no-`cd` control that must
still restore at the workspace directory.

## Launch

```bash
HOME="$SMOKE_HOME" RUST_LOG=horizon=info,horizon_core=info target/debug/horizon \
  --config "$SMOKE_HOME/.horizon/config.yaml"
```

Verify Horizon opens without crashing and both `Roamer` and `Stayer` shell panels
are visible in the `CWD Smoke` workspace.

## Test 1 - Live cwd tracking follows `cd`

1. Click into the `Roamer` panel and run:
   ```bash
   cd "$CWD_TARGET"
   pwd -P          # note this resolved path, call it TARGET_RESOLVED
   ```
   (On macOS `/tmp` and `$TMPDIR` are symlinks, so `pwd -P` gives the real path
   that will be persisted. Use that value for comparisons below.)
2. Leave the `Stayer` panel untouched.
3. Wait about 2 seconds (runtime state auto-saves on a 500 ms debounce), then in a
   separate terminal inspect the persisted state:
   ```bash
   cat "$SMOKE_HOME"/.horizon/sessions/*/runtime.yaml
   ```
   Confirm:
   - the `Roamer` panel entry has `cwd:` equal to `TARGET_RESOLVED` (the cd'd dir),
     **not** the workspace dir.
   - the `Stayer` panel entry still has `cwd:` equal to the workspace dir
     (the resolved form of `$SMOKE_HOME`).

## Test 2 - Restart restores the cd'd directory

1. Quit Horizon (Cmd-Q or close the window). This triggers a final save.
2. Relaunch with the same command as in **Launch**.
3. In the restored `Roamer` panel run:
   ```bash
   pwd -P
   ```
   Confirm it prints `TARGET_RESOLVED` (the directory you cd'd into before quit).
4. In the restored `Stayer` panel run `pwd -P` and confirm it prints the workspace
   directory. This is the no-regression control.

Expected result: `Roamer` reopens in the directory it was left in; `Stayer`
reopens at the workspace directory. Before this feature worked, both reopened at
the workspace directory.

## Test 3 - Transcript-disabled fallback (optional)

Confirm the wrapper-skip logic does not misbehave when `script` is unavailable:

```bash
PATH="/usr/bin:/bin:/usr/sbin:/sbin" HOME="$SMOKE_HOME" target/debug/horizon \
  --config "$SMOKE_HOME/.horizon/config.yaml"
```

`cd` in `Roamer`, quit, relaunch, and confirm the panel still restores the cd'd
directory (cwd is read directly from the shell when there is no `script` wrapper).

## Evidence To Attach To PR

- `cargo build` result on macOS (arch: arm64 or x64).
- The `runtime.yaml` `Roamer` and `Stayer` `cwd:` lines from Test 1.
- `pwd -P` output from the restored panels in Test 2.
- A one-line note confirming `Roamer` restored to the cd'd dir and `Stayer` stayed
  at the workspace dir.
