# macOS Parent-Session Resume Smoke Plan

## Scope

Validate the Codex session-discovery and recovery fix on macOS using synthetic
local state. Horizon must never start or offer a parent-controlled Codex child
thread, even when that child is newer than its interactive root. A previously
persisted child binding must be repaired to its verified root before the panel
process starts.

This plan does not use a real Codex account or copy real session titles,
transcripts, or identifiers into PR evidence.

## Prerequisites

- macOS 14 or newer on Apple Silicon or Intel.
- Rust stable 1.88 or newer (`rustc --version`).
- Xcode Command Line Tools installed.
- Git LFS installed, with the checkout's LFS assets downloaded.
- GitHub CLI (`gh`) installed and authenticated for the repository.
- `/usr/bin/sqlite3` available.
- `rg` available (`brew install ripgrep`), or use `grep -nE` for the listed
  read-only text checks.
- The PR branch checked out at the exact head under test.

Run every command below in one zsh control terminal. Make failed assertions,
unset variables, and failed pipelines stop the lane instead of being hidden by
a later successful command. Then authenticate, hydrate LFS assets, record the
environment, and build the debug binary:

```zsh
setopt ERR_EXIT NO_UNSET PIPE_FAIL
gh auth status
git lfs version
git lfs pull
rustc --version
export HORIZON_SMOKE_PR='<PR number>'
export HORIZON_SMOKE_HEAD="$(git rev-parse HEAD)"
test "$HORIZON_SMOKE_HEAD" = "$(gh pr view "$HORIZON_SMOKE_PR" --json headRefOid --jq .headRefOid)"
sw_vers
uname -m
cargo build
```

Use the exact absolute binary from this checkout for every step below:

```zsh
export HORIZON_SMOKE_BIN="$PWD/target/debug/horizon"
test -x "$HORIZON_SMOKE_BIN"
```

## Isolated Fixture

Create a test-specific home and workspace. Keep these task-specific variables;
do not overwrite the shell's own `HOME` variable.

```zsh
export HORIZON_SMOKE_ROOT="$(mktemp -d /tmp/horizon-codex-parent-smoke.XXXXXX)"
export HORIZON_SMOKE_USER_HOME="$HORIZON_SMOKE_ROOT/home"
export HORIZON_SMOKE_CODEX_HOME="$HORIZON_SMOKE_ROOT/codex-home"
export HORIZON_SMOKE_WORKSPACE="$HORIZON_SMOKE_ROOT/workspace"
export HORIZON_SMOKE_ARGV="$HORIZON_SMOKE_ROOT/fake-codex-argv.log"
mkdir -p "$HORIZON_SMOKE_CODEX_HOME" "$HORIZON_SMOKE_USER_HOME/.horizon" "$HORIZON_SMOKE_WORKSPACE"
```

Create a deterministic fake Codex executable. It records every launch, prints
an input-ready marker, and echoes terminal input without network access:

```zsh
cat > "$HORIZON_SMOKE_ROOT/fake-codex" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >> "$HORIZON_SMOKE_ARGV"
printf '%s\n' "$$" >> "$HORIZON_SMOKE_ROOT/fake-pids.log"
printf 'FAKE CODEX READY\npid: %s\nargv: %s\n' "$$" "$*"
while IFS= read -r line; do
  printf 'accepted: %s\n' "$line"
done
SH
chmod +x "$HORIZON_SMOKE_ROOT/fake-codex"
```

Create synthetic rollout metadata. The child files deliberately point back to
the same interactive root:

```zsh
cat > "$HORIZON_SMOKE_ROOT/root-a.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"root-a","session_id":"root-a"}}
JSONL
cat > "$HORIZON_SMOKE_ROOT/root-b.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"root-b","session_id":"root-b"}}
JSONL
cat > "$HORIZON_SMOKE_ROOT/child-a.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"child-a","session_id":"child-a","parent_thread_id":"root-a"}}
JSONL
cat > "$HORIZON_SMOKE_ROOT/guardian-a.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"guardian-a","session_id":"root-a","parent_thread_id":"root-a"}}
JSONL
cat > "$HORIZON_SMOKE_ROOT/review-a.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"review-a","parent_thread_id":"root-a"}}
JSONL
```

Seed the Codex database. The child rows are newer than both roots so an
unfiltered recency match would choose a child. The database intentionally omits
newer optional Codex columns to cover the legacy-compatible query surface.

```zsh
/usr/bin/sqlite3 "$HORIZON_SMOKE_CODEX_HOME/state_5.sqlite" <<SQL
CREATE TABLE threads (
  id TEXT PRIMARY KEY,
  rollout_path TEXT NOT NULL,
  source TEXT NOT NULL,
  title TEXT NOT NULL,
  cwd TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  archived INTEGER NOT NULL DEFAULT 0
);
INSERT INTO threads VALUES
  ('root-a', '$HORIZON_SMOKE_ROOT/root-a.jsonl', 'cli', 'Interactive root A', '$HORIZON_SMOKE_WORKSPACE', 100, 0),
  ('root-b', '$HORIZON_SMOKE_ROOT/root-b.jsonl', 'vscode', 'Interactive root B', '$HORIZON_SMOKE_WORKSPACE', 90, 0),
  ('child-a', '$HORIZON_SMOKE_ROOT/child-a.jsonl', '{"subagent":{"thread_spawn":{"parent_thread_id":"root-a"}}}', 'Spawned child - must stay hidden', '$HORIZON_SMOKE_WORKSPACE', 400, 0),
  ('guardian-a', '$HORIZON_SMOKE_ROOT/guardian-a.jsonl', '{"subagent":{"other":"guardian"}}', 'Guardian child - must stay hidden', '$HORIZON_SMOKE_WORKSPACE', 300, 0),
  ('review-a', '$HORIZON_SMOKE_ROOT/review-a.jsonl', '{"subagent":"review"}', 'Review child - must stay hidden', '$HORIZON_SMOKE_WORKSPACE', 200, 0),
  ('archived-root', '$HORIZON_SMOKE_ROOT/root-a.jsonl', 'cli', 'Archived root - must stay hidden', '$HORIZON_SMOKE_WORKSPACE', 500, 1),
  ('other-root', '$HORIZON_SMOKE_ROOT/root-b.jsonl', 'exec', 'Other directory', '/tmp/horizon-other-workspace', 600, 0);
SQL
```

Create a single Codex panel. Shell expansion is intentional so the config uses
absolute fixture paths:

```zsh
cat > "$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml" <<YAML
version: 8
window:
  width: 1280
  height: 860
workspaces:
  - name: Parent Resume Smoke
    cwd: $HORIZON_SMOKE_WORKSPACE
    terminals:
      - name: Synthetic Codex
        kind: codex
        command: $HORIZON_SMOKE_ROOT/fake-codex
        args:
          - --no-alt-screen
        resume: last
YAML
```

## Test 1 - Cold-Start Discovery

Keep the remaining commands in this control terminal so exported fixture state
and exact PIDs remain available. Define a helper that launches the primary
instance without a pipeline, records its exact PID, and writes each launch to a
separate log:

```zsh
horizon_smoke_launch_primary() {
  local log_name="$1"
  shift
  if test -n "${HORIZON_SMOKE_PID:-}" && kill -0 "$HORIZON_SMOKE_PID" 2>/dev/null; then
    echo "previous primary is still running: $HORIZON_SMOKE_PID" >&2
    return 1
  fi
  env HOME="$HORIZON_SMOKE_USER_HOME" \
    CODEX_HOME="$HORIZON_SMOKE_CODEX_HOME" \
    HORIZON_SMOKE_ARGV="$HORIZON_SMOKE_ARGV" \
    RUST_LOG=horizon=info,horizon_core=info \
    "$HORIZON_SMOKE_BIN" \
    --config "$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml" \
    "$@" > "$HORIZON_SMOKE_ROOT/$log_name" 2>&1 &
  export HORIZON_SMOKE_PID=$!
  printf '%s\n' "$HORIZON_SMOKE_PID" > "$HORIZON_SMOKE_ROOT/horizon-primary.pid"
  kill -0 "$HORIZON_SMOKE_PID"
}

horizon_smoke_launch_primary horizon-first.log --new-session
```

From the same control terminal, capture the fake panel PID before interacting
with the window:

```zsh
HORIZON_SMOKE_WAIT_ATTEMPT=0
while test ! -s "$HORIZON_SMOKE_ROOT/fake-pids.log" && test "$HORIZON_SMOKE_WAIT_ATTEMPT" -lt 100; do
  kill -0 "$HORIZON_SMOKE_PID"
  sleep 0.1
  HORIZON_SMOKE_WAIT_ATTEMPT=$((HORIZON_SMOKE_WAIT_ATTEMPT + 1))
done
test -s "$HORIZON_SMOKE_ROOT/fake-pids.log"
export HORIZON_SMOKE_FAKE_PID="$(tail -n 1 "$HORIZON_SMOKE_ROOT/fake-pids.log")"
test -n "$HORIZON_SMOKE_FAKE_PID"
ps -p "$HORIZON_SMOKE_PID,$HORIZON_SMOKE_FAKE_PID" -o pid=,ppid=,command=
```

Verify:

- The window opens and the panel prints `FAKE CODEX READY`.
- The recorded launch is exactly `--no-alt-screen resume root-a`.
- No launch-log line contains `child-a`, `guardian-a`, or `review-a`.
- The startup log identifies the Metal renderer and contains no panic.
- Typing `hello parent` prints `accepted: hello parent`, proving direct input
  reaches the resumed panel.

Inspect the exact launch evidence from the same control terminal:

```zsh
nl -ba "$HORIZON_SMOKE_ARGV"
```

## Test 2 - Persisted-Child Recovery

Quit the exact Horizon process with Cmd-Q, reap that process in the control
terminal, and locate the isolated runtime file:

```zsh
wait "$HORIZON_SMOKE_PID"
export HORIZON_SMOKE_RUNTIME="$(find "$HORIZON_SMOKE_USER_HOME/.horizon/sessions" -name runtime.yaml -type f -print -quit)"
test -n "$HORIZON_SMOKE_RUNTIME"
rg -n 'resume:|session_id:|label:' "$HORIZON_SMOKE_RUNTIME"
```

Confirm the saved binding is `root-a`. Then simulate a runtime written by an
affected older Horizon build. Use the review-child fixture whose rollout has
only `parent_thread_id` metadata so this exercises that real recovery shape,
inside the isolated fixture only:

```zsh
perl -pi -e 's/session_id: root-a/session_id: review-a/' "$HORIZON_SMOKE_RUNTIME"
rg -n 'session_id:' "$HORIZON_SMOKE_RUNTIME"
rg -q 'session_id: review-a' "$HORIZON_SMOKE_RUNTIME"
```

Relaunch without `--new-session`; the helper replaces the primary PID record:

```zsh
horizon_smoke_launch_primary horizon-recovery.log
```

Verify before interacting with the panel:

- The new launch-log line is `--no-alt-screen resume root-a`, never
  `resume review-a`.
- The panel accepts `hello recovered` and echoes it.
- After waiting at least one second, the isolated `runtime.yaml` again contains
  `session_id: root-a` and no `session_id: review-a`.

Pin the migration before the persistence check:

```zsh
test "$(tail -n 1 "$HORIZON_SMOKE_ARGV")" = '--no-alt-screen resume root-a'
rg -q 'session_id: root-a' "$HORIZON_SMOKE_RUNTIME"
! rg -q 'session_id: review-a' "$HORIZON_SMOKE_RUNTIME"
kill -0 "$HORIZON_SMOKE_PID"
```

Quit that exact primary with Cmd-Q, reap it, and relaunch the repaired runtime:

```zsh
wait "$HORIZON_SMOKE_PID"
export HORIZON_SMOKE_RECOVERY_RELAUNCH_LINES="$(wc -l < "$HORIZON_SMOKE_ARGV")"
horizon_smoke_launch_primary horizon-recovery-relaunch.log
```

After the panel appears, prove one relaunch still resumes `root-a` and leave it
running for the validation-failure lane:

```zsh
test "$(wc -l < "$HORIZON_SMOKE_ARGV")" -eq "$((HORIZON_SMOKE_RECOVERY_RELAUNCH_LINES + 1))"
test "$(tail -n 1 "$HORIZON_SMOKE_ARGV")" = '--no-alt-screen resume root-a'
kill -0 "$HORIZON_SMOKE_PID"
```

This is the decisive regression lane for users whose existing saved panel is
already bound to a parent-controlled child.

### Store-Unavailable Safe Degradation

Quit and reap the current primary. Move the isolated database aside, record the
current fake launch count, and relaunch. Horizon must open automatically rather
than blocking on a full-screen validation error. The panel starts exactly once
without a `resume` subcommand, while the saved id remains recoverable with an
explicit non-resumable marker:

```zsh
wait "$HORIZON_SMOKE_PID"
mv "$HORIZON_SMOKE_CODEX_HOME/state_5.sqlite" "$HORIZON_SMOKE_ROOT/state_5.sqlite.saved"
export HORIZON_SMOKE_SAFE_ARGV_LINES="$(wc -l < "$HORIZON_SMOKE_ARGV")"
horizon_smoke_launch_primary horizon-store-unavailable.log
```

Verify the board and fake panel appear without operator intervention, the panel
accepts `hello safe`, and pin the safe launch and retained-id evidence:

```zsh
test "$(wc -l < "$HORIZON_SMOKE_ARGV")" -eq "$((HORIZON_SMOKE_SAFE_ARGV_LINES + 1))"
test "$(tail -n 1 "$HORIZON_SMOKE_ARGV")" = '--no-alt-screen'
rg -q 'session_id: root-a' "$HORIZON_SMOKE_RUNTIME"
rg -q 'resumable: false' "$HORIZON_SMOKE_RUNTIME"
kill -0 "$HORIZON_SMOKE_PID"
```

Quit and reap that exact primary, restore the database, and relaunch. Exact
validation must recover the retained binding and resume `root-a` without any
manual state edit:

```zsh
wait "$HORIZON_SMOKE_PID"
mv "$HORIZON_SMOKE_ROOT/state_5.sqlite.saved" "$HORIZON_SMOKE_CODEX_HOME/state_5.sqlite"
horizon_smoke_launch_primary horizon-store-restored.log
```

After the panel appears and accepts `hello restored store`, pin the recovery
and leave this primary running for Test 3:

```zsh
test "$(wc -l < "$HORIZON_SMOKE_ARGV")" -eq "$((HORIZON_SMOKE_SAFE_ARGV_LINES + 2))"
test "$(tail -n 1 "$HORIZON_SMOKE_ARGV")" = '--no-alt-screen resume root-a'
kill -0 "$HORIZON_SMOKE_PID"
```

## Test 3 - Root-Only Rebind And Restart

With Horizon running:

1. Record the fake launch-log line count.
2. Secondary-click the `Synthetic Codex` title bar and open
   **Rebind & Restart**.
3. Confirm `Interactive root B` appears.
4. Confirm none of these appear: `Spawned child`, `Guardian child`,
   `Review child`, `Archived root`, or `Other directory`.
5. Select `Interactive root B`.

Verify:

- Exactly one new fake-process launch was recorded.
- Its arguments are `--no-alt-screen resume root-b`.
- The old panel process exits and the new exact process remains live; scope any
  process inspection to the PID printed by the active fake panel rather than
  matching all Horizon processes by name.
- The panel keeps its position and size.
- Typing `hello root b` prints `accepted: hello root b`.
- After Cmd-Q and relaunch, the persisted binding and launch argument remain
  `root-b`.

After Cmd-Q, run `wait "$HORIZON_SMOKE_PID"`, then use
`horizon_smoke_launch_primary horizon-root-b-relaunch.log` for that relaunch so
the primary PID record points to the current instance.

Capture a screenshot with the root-only rebind menu open and another after the
restarted panel accepts input. The fixture names are synthetic and safe for the
public PR.

## Test 4 - Startup Chooser And Fresh Control

While the isolated persistent session is live, start a second exact binary with
the same environment and no `--new-session`. Verify the Live Conflict chooser
appears before a second fake process starts. Choose **Open Copy** and confirm its
Codex panel still resumes a root ID, never a child ID.

Reload and validate the current primary PID, then launch the second process
without a pipeline so `$!` is the exact Horizon PID:

```zsh
export HORIZON_SMOKE_PID="$(cat "$HORIZON_SMOKE_ROOT/horizon-primary.pid")"
test -n "$HORIZON_SMOKE_PID"
kill -0 "$HORIZON_SMOKE_PID"
export HORIZON_SMOKE_PRIMARY_COMMAND="$(ps -p "$HORIZON_SMOKE_PID" -o command=)"
case "$HORIZON_SMOKE_PRIMARY_COMMAND" in
  *"$HORIZON_SMOKE_BIN"*"$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml"*) ;;
  *) echo "primary PID does not match the isolated fixture" >&2; exit 1 ;;
esac

env HOME="$HORIZON_SMOKE_USER_HOME" \
  CODEX_HOME="$HORIZON_SMOKE_CODEX_HOME" \
  HORIZON_SMOKE_ARGV="$HORIZON_SMOKE_ARGV" \
  RUST_LOG=horizon=info,horizon_core=info \
  "$HORIZON_SMOKE_BIN" \
  --config "$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml" \
  > "$HORIZON_SMOKE_ROOT/horizon-copy.log" 2>&1 &
export HORIZON_SMOKE_COPY_PID=$!
test "$HORIZON_SMOKE_COPY_PID" != "$HORIZON_SMOKE_PID"
kill -0 "$HORIZON_SMOKE_PID"
kill -0 "$HORIZON_SMOKE_COPY_PID"
ps -p "$HORIZON_SMOKE_PID,$HORIZON_SMOKE_COPY_PID" -o pid=,ppid=,command=
```

Do not use an application-name window search while both instances are live.
Record the fake PID printed inside each panel and use those PIDs for
child-process checks.

As a fresh-launch control, activate each exact window, quit both the primary and
copy with Cmd-Q, and reap both processes before changing the isolated config:

```zsh
wait "$HORIZON_SMOKE_PID"
wait "$HORIZON_SMOKE_COPY_PID"
export HORIZON_SMOKE_FRESH_ARGV_LINE="$(($(wc -l < "$HORIZON_SMOKE_ARGV") + 1))"
perl -pi -e 's/resume: last/resume: fresh/' "$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml"

env HOME="$HORIZON_SMOKE_USER_HOME" \
  CODEX_HOME="$HORIZON_SMOKE_CODEX_HOME" \
  HORIZON_SMOKE_ARGV="$HORIZON_SMOKE_ARGV" \
  RUST_LOG=horizon=info,horizon_core=info \
  "$HORIZON_SMOKE_BIN" \
  --config "$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml" \
  --new-session > "$HORIZON_SMOKE_ROOT/horizon-fresh.log" 2>&1 &
export HORIZON_SMOKE_FRESH_PID=$!
kill -0 "$HORIZON_SMOKE_FRESH_PID"
```

Verify the fresh panel starts and accepts input. Its launch-log line at
`$HORIZON_SMOKE_FRESH_ARGV_LINE` must be exactly `--no-alt-screen`, with no
`resume` subcommand. Restore `resume: last` in the fixture afterward if
repeating earlier lanes.

## Test 5 - macOS Visual And Lifecycle Regression

On the exact PID under test:

- Resize the Horizon window and the Codex panel; text remains crisp at the
  display's Retina scale and the panel does not flicker or disappear.
- Capture a screenshot after the resize with the accepted-input marker visible.
- Cmd-Tab away and back; keyboard focus returns and input still reaches the
  fake panel.
- Open and dismiss the title-bar context menu several times; menu positioning
  and selection remain stable.
- Quit the exact fresh-control process with Cmd-Q and run
  `wait "$HORIZON_SMOKE_FRESH_PID"`. Horizon exits cleanly, the fake child
  exits, and no process from the isolated fixture remains.

## Required Evidence And Report

Attach or summarize only synthetic evidence:

- Exact commit SHA, macOS version, and architecture.
- `cargo build` result.
- Numbered fake launch-log lines for cold start, persisted-child repair,
  rebind/restart, and relaunch.
- Relevant synthetic `runtime.yaml` `session_id` lines before corruption, after
  corruption, and after repair.
- Validation-failure launch counts, the exact conflict-process PID pair, and
  the fresh safe-open argv line.
- Screenshots of the root-only rebind menu and accepted input after restart.
- Metal/Retina, resize, focus-return, startup-chooser, and clean-shutdown
  results.

Immediately before posting the report, prove that the tested checkout and the
open PR still have the exact head captured before the build:

```zsh
export HORIZON_SMOKE_FINAL_HEAD="$(git rev-parse HEAD)"
test "$HORIZON_SMOKE_FINAL_HEAD" = "$HORIZON_SMOKE_HEAD"
test "$HORIZON_SMOKE_FINAL_HEAD" = "$(gh pr view "$HORIZON_SMOKE_PR" --json headRefOid --jq .headRefOid)"
```

If the executor pushed a fix or the PR head changed at any point, do not post a
completion marker from the old run. Check out the new head, update
`HORIZON_SMOKE_HEAD`, rebuild, recreate the isolated fixture, and rerun the
complete plan before reporting.

Report on the PR using this shape:

```text
SMOKE-TEST REPORT (macOS <version>, <arm64|x86_64>, <commit>)
- cold-start root selection: pass | fail — note
- persisted-child automatic recovery: pass | fail — note
- root-only rebind and one restart: pass | fail — note
- persistence and relaunch: pass | fail — note
- validation-failure Retry and safe-open recovery: pass | fail — note
- startup chooser and fresh control: pass | fail — note
- Metal/Retina, resize, focus, and shutdown: pass | fail — note
Summary: fixes made, remaining failures, and evidence links
SMOKE-TEST: DONE
```

The final line must be exactly `SMOKE-TEST: DONE`. Keep the fixture until the
report is accepted; afterward move only `$HORIZON_SMOKE_ROOT` to Trash.
