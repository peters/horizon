# Browser durable preparation macOS smoke plan

Temporary exact-head validation plan for issue #324. Run every command from
the pull-request checkout on macOS. This slice changes only deterministic
`horizon-browser run` preparation and deadline state; it does not require
launching or stopping the Horizon GUI or any existing browser process.

## Safety and exact-head gate

- Use a clean checkout of the pull-request branch.
- Do not reuse, signal, or stop any pre-existing Horizon process.
- Keep every CLI artifact under a fresh temporary `HOME`.
- Record `uname -a`, `uname -m`, and `rustc --version` in the report.
- Record `git rev-parse HEAD` and compare it with the current pull-request head.
  Stop on any mismatch.
- After the test, remove only the temporary directory created below.

```bash
git status --short
git rev-parse HEAD
uname -a
uname -m
rustc --version
```

Expected: the checkout is clean, the SHA matches the current pull-request
head, and the host reports macOS. State whether the machine is arm64 or x86_64.

## Build and process-level regression suite

```bash
cargo build -p horizon-browser-cli
cargo test -p horizon-browser-cli
cargo test -p horizon-browser-cli \
  run_state::tests::prepared_state_is_a_bounded_lease_before_terminal_report \
  -- --exact --nocapture
cargo test -p horizon-browser-cli \
  tests::expired_control_stops_before_mcp_initialization \
  -- --exact --nocapture
cargo test -p horizon-browser-cli \
  tests::ready_deadline_does_not_mark_an_unpolled_action_in_flight \
  -- --exact --nocapture
cargo test -p horizon-browser-cli --test cli \
  run_deadline_persists_a_partial_report_and_stable_exit_code \
  -- --exact --nocapture
```

Expected:

- all commands pass on the exact head;
- prepared state resolves to `running` only before its deadline and to
  `timed_out` at or after it;
- an expired control stops before MCP initialization;
- a ready deadline does not poll or label a browser action as in flight;
- the deadline integration retains its completed prefix, persists
  `timed_out`, and observes exit code 124;
- a requested-output write failure still preserves exit code 124.

## Normal run, atomic publication, and permissions

```bash
SMOKE_HOME="$(mktemp -d)"
export SMOKE_HOME
unset HORIZON HORIZON_BROWSER_ACTOR
mkdir -p "$SMOKE_HOME/input"
printf '%s\n' '{"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}' \
  > "$SMOKE_HOME/input/plan.json"
HOME="$SMOKE_HOME" RUST_LOG=off target/debug/horizon-browser run \
  "$SMOKE_HOME/input/plan.json" --output "$SMOKE_HOME/output.json"
/usr/bin/python3 - "$SMOKE_HOME/output.json" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert report["ok"] is True
assert report["completed_steps"] == 1
assert report["steps"][0]["tool"] == "browser_list"
job_dir = pathlib.Path(report["job_dir"])
state = json.loads((job_dir / "state.json").read_text())
assert state["version"] == 2
assert state["status"] == "succeeded"
assert state["execution_timeout_seconds"] == 1800
assert isinstance(state["deadline_at_millis"], int)
assert 0 < state["deadline_at_millis"] - state["created_at_millis"] <= 1_800_000
assert state["report_file"] == "report.json"
assert (job_dir / "plan.json").is_file()
assert (job_dir / "report.json").is_file()
job_root = job_dir.parent
assert all(path.name.startswith("job-") for path in job_root.iterdir())
print(job_dir)
PY
stat -f '%Lp %N' "$SMOKE_HOME/output.json" \
  "$SMOKE_HOME"/.horizon/browser-jobs/job-*/plan.json \
  "$SMOKE_HOME"/.horizon/browser-jobs/job-*/state.json \
  "$SMOKE_HOME"/.horizon/browser-jobs/job-*/report.json
stat -f '%Lp %N' "$SMOKE_HOME"/.horizon/browser-jobs/job-*
```

Expected: exit code 0; one successful step; version-2 state with an absolute
deadline no more than 1800 seconds after job creation; no visible
`.preparing-*` directory; owner-only `600` output/plan/state/report files and
`700` job directories; and all artifacts confined below the temporary home.

## Plan input remains outside this slice's deadline

```bash
/usr/bin/python3 - "$PWD/target/debug/horizon-browser" "$SMOKE_HOME" <<'PY'
import json
import os
import pathlib
import subprocess
import sys
import time

binary, home = sys.argv[1:]
environment = os.environ.copy()
environment["HOME"] = home
environment["RUST_LOG"] = "off"
environment.pop("HORIZON", None)
environment.pop("HORIZON_BROWSER_ACTOR", None)
process = subprocess.Popen(
    [binary, "run", "-", "--timeout", "1"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    env=environment,
)
time.sleep(1.5)
assert process.poll() is None, "this slice must not move plan input into the deadline"
plan = b'{"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}\n'
stdout, stderr = process.communicate(plan, timeout=15)
assert process.returncode == 0, (process.returncode, stderr.decode())
report = json.loads(stdout)
state = json.loads((pathlib.Path(report["job_dir"]) / "state.json").read_text())
assert state["version"] == 2
assert state["status"] == "succeeded"
assert state["execution_timeout_seconds"] == 1
assert isinstance(state["deadline_at_millis"], int)
print(report["job_dir"])
PY
```

Expected: the task-owned process remains alive while stdin is open beyond one
second, then succeeds after receiving the plan. The deadline is selected only
after input and validation. The next serial PR, not this one, will bound plan
input and blocking durable setup.

## Invalid timeout boundaries

Run each command separately and record its status:

```bash
set +e
HOME="$SMOKE_HOME" target/debug/horizon-browser run \
  "$SMOKE_HOME/input/plan.json" --timeout 0
ZERO_STATUS=$?
HOME="$SMOKE_HOME" target/debug/horizon-browser run \
  "$SMOKE_HOME/input/plan.json" --timeout 86401
HIGH_STATUS=$?
HOME="$SMOKE_HOME" target/debug/horizon-browser run \
  "$SMOKE_HOME/input/plan.json" --timeout 1 --timeout 2
DUPLICATE_STATUS=$?
set -e
printf 'zero=%s high=%s duplicate=%s\n' \
  "$ZERO_STATUS" "$HIGH_STATUS" "$DUPLICATE_STATUS"
```

Expected: all three statuses are exactly 2 and each diagnostic describes the
invalid CLI input. No browser action or durable job is created for a rejected
command.

## Durability and replay semantics review

Confirm that:

- `plan.json` and `state.json` become visible together through one staged
  directory publication;
- new non-terminal state is `prepared`, while legacy `running` state remains
  readable;
- an expired prepared state resolves conservatively to `timed_out`, including
  prepared state missing its required deadline;
- MCP cannot initialize after the monotonic deadline has already elapsed;
- only fully completed steps survive a tool-call timeout;
- pre-MCP expiry uses the phase-neutral deadline message, while a tool-call
  timeout still warns that an in-flight browser action may complete;
- nothing claims a timed-out mutation is automatically safe to replay.

This slice deliberately does not make plan input or blocking filesystem calls
interruptible and does not implement cancellation or automatic resume.

## Cleanup and report

```bash
test -n "$SMOKE_HOME"
test -d "$SMOKE_HOME"
rm -rf -- "$SMOKE_HOME"
git status --short
```

Expected: only the task-owned temporary home is removed and the checkout
remains clean.

Reply on the pull request with:

```text
SMOKE-TEST REPORT (macOS <version>, <architecture>, exact head <sha>)
- exact-head and clean-checkout gate: pass | fail — details
- build and full package tests: pass | fail — details
- prepared lease and pre-MCP expiry: pass | fail — details
- normal run, atomic publication, and permissions: pass | fail — details
- plan-input scope boundary: pass | fail — details
- deadline process and invalid-boundary regressions: pass | fail — details
- durability and replay semantics: pass | fail — details
Summary: findings, fixes pushed if any, and remaining limitations
SMOKE-TEST: DONE
```

The final marker belongs only on a completed report, never on the request.
