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
- Xcode Command Line Tools installed.
- `/usr/bin/sqlite3` available.
- `rg` available (`brew install ripgrep`), or use `grep -nE` for the listed
  read-only text checks.
- The PR branch checked out at the exact head under test.

Record the environment and build the debug binary:

```bash
export HORIZON_SMOKE_PR='<PR number>'
export HORIZON_SMOKE_HEAD="$(git rev-parse HEAD)"
test "$HORIZON_SMOKE_HEAD" = "$(gh pr view "$HORIZON_SMOKE_PR" --json headRefOid --jq .headRefOid)"
sw_vers
uname -m
cargo build
```

Use the exact absolute binary from this checkout for every step below:

```bash
export HORIZON_SMOKE_BIN="$PWD/target/debug/horizon"
test -x "$HORIZON_SMOKE_BIN"
```

## Isolated Fixture

Create a test-specific home and workspace. Keep these task-specific variables;
do not overwrite the shell's own `HOME` variable.

```bash
export HORIZON_SMOKE_ROOT="$(mktemp -d /tmp/horizon-codex-parent-smoke.XXXXXX)"
export HORIZON_SMOKE_USER_HOME="$HORIZON_SMOKE_ROOT/home"
export HORIZON_SMOKE_WORKSPACE="$HORIZON_SMOKE_ROOT/workspace"
export HORIZON_SMOKE_ARGV="$HORIZON_SMOKE_ROOT/fake-codex-argv.log"
mkdir -p "$HORIZON_SMOKE_USER_HOME/.codex" "$HORIZON_SMOKE_USER_HOME/.horizon" "$HORIZON_SMOKE_WORKSPACE"
```

Create a deterministic fake Codex executable. It records every launch, prints
an input-ready marker, and echoes terminal input without network access:

```bash
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

```bash
cat > "$HORIZON_SMOKE_ROOT/root-a.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"root-a","session_id":"root-a"}}
JSONL
cat > "$HORIZON_SMOKE_ROOT/root-b.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"root-b","session_id":"root-b"}}
JSONL
cat > "$HORIZON_SMOKE_ROOT/child-a.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"child-a","session_id":"root-a","parent_thread_id":"root-a"}}
JSONL
cat > "$HORIZON_SMOKE_ROOT/guardian-a.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"guardian-a","session_id":"root-a","parent_thread_id":"root-a"}}
JSONL
cat > "$HORIZON_SMOKE_ROOT/review-a.jsonl" <<'JSONL'
{"type":"session_meta","payload":{"id":"review-a","session_id":"root-a","parent_thread_id":"root-a"}}
JSONL
```

Seed the Codex database. The child rows are newer than both roots so an
unfiltered recency match would choose a child. The database intentionally omits
newer optional Codex columns to cover the legacy-compatible query surface.

```bash
/usr/bin/sqlite3 "$HORIZON_SMOKE_USER_HOME/.codex/state_5.sqlite" <<SQL
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

```bash
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

Launch a new persistent Horizon session with the isolated environment:

```bash
env HOME="$HORIZON_SMOKE_USER_HOME" \
  HORIZON_SMOKE_ARGV="$HORIZON_SMOKE_ARGV" \
  RUST_LOG=horizon=info,horizon_core=info \
  "$HORIZON_SMOKE_BIN" \
  --config "$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml" \
  --new-session 2>&1 | tee "$HORIZON_SMOKE_ROOT/horizon-first.log"
```

From another terminal, capture only this fixture's exact Horizon PID and fake
panel PID before interacting with the window:

```bash
export HORIZON_SMOKE_PID="$(pgrep -n -f "$HORIZON_SMOKE_BIN.*$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml")"
test -n "$HORIZON_SMOKE_PID"
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

Inspect the exact launch evidence from another terminal:

```bash
nl -ba "$HORIZON_SMOKE_ARGV"
```

## Test 2 - Persisted-Child Recovery

Quit the exact Horizon process with Cmd-Q and locate the isolated runtime file:

```bash
export HORIZON_SMOKE_RUNTIME="$(find "$HORIZON_SMOKE_USER_HOME/.horizon/sessions" -name runtime.yaml -type f -print -quit)"
test -n "$HORIZON_SMOKE_RUNTIME"
rg -n 'resume:|session_id:|label:' "$HORIZON_SMOKE_RUNTIME"
```

Confirm the saved binding is `root-a`. Then simulate a runtime written by an
affected older Horizon build, inside the isolated fixture only:

```bash
perl -pi -e 's/session_id: root-a/session_id: child-a/' "$HORIZON_SMOKE_RUNTIME"
rg -n 'session_id:' "$HORIZON_SMOKE_RUNTIME"
```

Relaunch without `--new-session`:

```bash
env HOME="$HORIZON_SMOKE_USER_HOME" \
  HORIZON_SMOKE_ARGV="$HORIZON_SMOKE_ARGV" \
  RUST_LOG=horizon=info,horizon_core=info \
  "$HORIZON_SMOKE_BIN" \
  --config "$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml" \
  2>&1 | tee "$HORIZON_SMOKE_ROOT/horizon-recovery.log"
```

Verify before interacting with the panel:

- The new launch-log line is `--no-alt-screen resume root-a`, never
  `resume child-a`.
- The panel accepts `hello recovered` and echoes it.
- After waiting at least one second, the isolated `runtime.yaml` again contains
  `session_id: root-a` and no `session_id: child-a`.
- Quit and relaunch once more; it still resumes `root-a`.

This is the decisive regression lane for users whose existing saved panel is
already bound to a parent-controlled child.

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

Capture a screenshot with the root-only rebind menu open and another after the
restarted panel accepts input. The fixture names are synthetic and safe for the
public PR.

## Test 4 - Startup Chooser And Fresh Control

While the isolated persistent session is live, start a second exact binary with
the same environment and no `--new-session`. Verify the Live Conflict chooser
appears before a second fake process starts. Choose **Open Copy** and confirm its
Codex panel still resumes a root ID, never a child ID.

Launch the second process without a pipeline so `$!` is the exact Horizon PID:

```bash
env HOME="$HORIZON_SMOKE_USER_HOME" \
  HORIZON_SMOKE_ARGV="$HORIZON_SMOKE_ARGV" \
  RUST_LOG=horizon=info,horizon_core=info \
  "$HORIZON_SMOKE_BIN" \
  --config "$HORIZON_SMOKE_USER_HOME/.horizon/config.yaml" \
  > "$HORIZON_SMOKE_ROOT/horizon-copy.log" 2>&1 &
export HORIZON_SMOKE_COPY_PID=$!
ps -p "$HORIZON_SMOKE_PID,$HORIZON_SMOKE_COPY_PID" -o pid=,ppid=,command=
```

Do not use an application-name window search while both instances are live.
Record the fake PID printed inside each panel and use those PIDs for
child-process checks.

As a fresh-launch control, temporarily change the isolated config panel to
`resume: fresh`, start with `--new-session`, and verify its first launch is
`--no-alt-screen` with no `resume` subcommand. Restore the fixture afterward if
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
- Quit with Cmd-Q. Horizon exits cleanly, the fake child exits, and no process
  from the isolated fixture remains.

## Required Evidence And Report

Attach or summarize only synthetic evidence:

- Exact commit SHA, macOS version, and architecture.
- `cargo build` result.
- Numbered fake launch-log lines for cold start, persisted-child repair,
  rebind/restart, and relaunch.
- Relevant synthetic `runtime.yaml` `session_id` lines before corruption, after
  corruption, and after repair.
- Screenshots of the root-only rebind menu and accepted input after restart.
- Metal/Retina, resize, focus-return, startup-chooser, and clean-shutdown
  results.

Report on the PR using this shape:

```text
SMOKE-TEST REPORT (macOS <version>, <arm64|x86_64>, <commit>)
- cold-start root selection: pass | fail — note
- persisted-child automatic recovery: pass | fail — note
- root-only rebind and one restart: pass | fail — note
- persistence and relaunch: pass | fail — note
- startup chooser and fresh control: pass | fail — note
- Metal/Retina, resize, focus, and shutdown: pass | fail — note
Summary: fixes made, remaining failures, and evidence links
SMOKE-TEST: DONE
```

The final line must be exactly `SMOKE-TEST: DONE`. Keep the fixture until the
report is accepted; afterward move only `$HORIZON_SMOKE_ROOT` to Trash.
