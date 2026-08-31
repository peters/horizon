# Browser atomic publication macOS smoke plan

Temporary exact-head validation for the issue #324 durable-publication
prerequisite. Run every command from the pull-request checkout on macOS. This
slice changes only deterministic `horizon-browser run` filesystem publication;
it does not require launching, stopping, or reusing the Horizon GUI.

## Safety and exact-head gate

- Use a clean checkout of the pull-request branch.
- Do not signal or stop any pre-existing Horizon process.
- Keep all runtime artifacts in fresh task-owned temporary directories.
- Record the pull-request head, host architecture, macOS version, and Rust
  version in the report. Stop if the checkout SHA differs from the PR head.

```bash
git status --short
git rev-parse HEAD
uname -a
uname -m
rustc --version
```

Expected: a clean checkout at the exact PR head on macOS.

## Build and regression tests

```bash
cargo build -p horizon-browser-cli
cargo test -p horizon-browser-cli
cargo test -p horizon-browser-cli \
  run_state::tests::state_is_running_before_execution_and_terminal_after_report \
  -- --exact --nocapture
cargo test -p horizon-browser-cli --test cli \
  run_publishes_a_complete_relative_job_when_home_is_unset \
  -- --exact --nocapture
```

Expected: all commands pass on the exact head, including the fresh parent-chain
and HOME-unset relative-root regressions.

## Absolute-home publication and permissions

```bash
SMOKE_HOME="$(mktemp -d)"
export SMOKE_HOME
unset HORIZON HORIZON_BROWSER_ACTOR
printf '%s\n' '{"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}' \
  > "$SMOKE_HOME/plan.json"
HOME="$SMOKE_HOME" RUST_LOG=off target/debug/horizon-browser run \
  "$SMOKE_HOME/plan.json" --output "$SMOKE_HOME/output.json"
/usr/bin/python3 - "$SMOKE_HOME/output.json" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert report["ok"] is True
assert report["completed_steps"] == 1
job_dir = pathlib.Path(report["job_dir"])
assert job_dir.parent == pathlib.Path(sys.argv[1]).parent / ".horizon/browser-jobs"
state = json.loads((job_dir / "state.json").read_text())
assert state["version"] == 1
assert state["status"] == "succeeded"
assert state["report_file"] == "report.json"
assert (job_dir / "plan.json").is_file()
assert (job_dir / "report.json").is_file()
assert [entry.name for entry in job_dir.parent.iterdir()] == [job_dir.name]
print(job_dir)
PY
stat -f '%Lp %N' "$SMOKE_HOME/output.json" \
  "$SMOKE_HOME"/.horizon/browser-jobs/job-*/plan.json \
  "$SMOKE_HOME"/.horizon/browser-jobs/job-*/state.json \
  "$SMOKE_HOME"/.horizon/browser-jobs/job-*/report.json
stat -f '%Lp %N' "$SMOKE_HOME"/.horizon/browser-jobs/job-*
```

Expected: exit code 0; one complete final `job-*` directory and no visible
`.preparing-*` directory; owner-only `600` files and a `700` job directory.

## HOME-unset relative-root publication

```bash
SMOKE_CWD="$(mktemp -d)"
SMOKE_BINARY="$PWD/target/debug/horizon-browser"
export SMOKE_CWD
cp "$SMOKE_HOME/plan.json" "$SMOKE_CWD/plan.json"
(
  cd "$SMOKE_CWD"
  unset HOME HORIZON HORIZON_BROWSER_ACTOR
  RUST_LOG=off "$SMOKE_BINARY" run "$SMOKE_CWD/plan.json" \
    > "$SMOKE_CWD/output.json"
)
/usr/bin/python3 - "$SMOKE_CWD" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
report = json.loads((root / "output.json").read_text())
job_dir = pathlib.Path(report["job_dir"])
assert not job_dir.is_absolute()
assert job_dir.parent == pathlib.Path(".horizon/browser-jobs")
absolute_job_dir = root / job_dir
state = json.loads((absolute_job_dir / "state.json").read_text())
assert state["status"] == "succeeded"
assert (absolute_job_dir / "plan.json").is_file()
assert (absolute_job_dir / "report.json").is_file()
assert [entry.name for entry in absolute_job_dir.parent.iterdir()] == [absolute_job_dir.name]
print(absolute_job_dir)
PY
```

Expected: the command succeeds with HOME absent, publishes beneath the current
directory's relative `.horizon/browser-jobs`, and leaves only one complete
final job directory.

## Durability review and cleanup

Confirm from the exact diff that:

- `plan.json` and initial version-1 `running` state are flushed inside a staged
  directory before one rename publishes the job;
- the staged directory and final job-root rename are synchronized;
- every newly created parent is synchronized into its parent, with `.` used
  for an empty relative parent;
- macOS uses the Unix directory-sync path, while the focused Windows CI
  regression executes publication through the safe backup-semantics directory
  handle;
- deadline start time and lifecycle-state semantics are unchanged in this
  prerequisite.

```bash
test -n "$SMOKE_HOME" && test -d "$SMOKE_HOME"
test -n "$SMOKE_CWD" && test -d "$SMOKE_CWD"
rm -rf -- "$SMOKE_HOME" "$SMOKE_CWD"
git status --short
```

Expected: only task-owned temporary directories are removed and the checkout
remains clean.

Reply on the pull request with:

```text
SMOKE-TEST REPORT (macOS <version>, <architecture>, exact head <sha>)
- exact-head and clean-checkout gate: pass | fail — details
- build and package tests: pass | fail — details
- absolute-home atomic publication and permissions: pass | fail — details
- HOME-unset relative-root publication: pass | fail — details
- durability and unchanged-semantics review: pass | fail — details
Summary: findings, fixes pushed if any, and remaining limitations
SMOKE-TEST: DONE
```

The final marker belongs only on a completed report, never on the request.
