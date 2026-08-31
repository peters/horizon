# Browser CLI whole-job deadline smoke

Use this temporary plan on the exact pull-request head. It exercises only
task-owned CLI processes and isolated homes; do not stop or reuse an existing
Horizon process.

## Record the candidate

```bash
git rev-parse HEAD
git status --short
```

Record the SHA in the smoke report and require a clean checkout.

## Build and focused regression suite

```bash
cargo build -p horizon-browser-cli
cargo test -p horizon-browser-cli
```

Require all library, binary, process, and documentation tests to pass. The
process suite specifically proves that an open stdin is bounded, a partial MCP
run retains its completed prefix, and durable state records the absolute
deadline.

## Baseline successful run

```bash
smoke_home="$(mktemp -d)"
printf '%s\n' '{"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}' > "$smoke_home/plan.json"
HOME="$smoke_home" HORIZON_BROWSER_ACTOR=deadline-smoke RUST_LOG=off \
  target/debug/horizon-browser run "$smoke_home/plan.json" --timeout 30 \
  > "$smoke_home/report.json"
python3 - "$smoke_home" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
report = json.loads((root / "report.json").read_text())
state = json.loads(pathlib.Path(report["state_path"]).read_text())
assert report["ok"] is True
assert report["completed_steps"] == 1
assert state["status"] == "succeeded"
assert state["execution_timeout_seconds"] == 30
assert isinstance(state["deadline_at_millis"], int)
assert state["deadline_at_millis"] >= state["created_at_millis"]
assert not list(pathlib.Path(report["job_dir"]).glob(".state-activation-*.json"))
PY
```

On Linux, require the job directory to be mode `700` and its JSON artifacts to
be `600` using `stat -c '%a'`. On macOS, use `stat -f '%Lp'` and require the same
values.

## Deadline and input edges

```bash
HOME="$smoke_home" RUST_LOG=off target/debug/horizon-browser run "$smoke_home/plan.json" --timeout 0
HOME="$smoke_home" RUST_LOG=off target/debug/horizon-browser run "$smoke_home/plan.json" --timeout 86401
```

Both commands must exit `2`. Then run the focused process test independently so
its open-stdin deadline is visible in the transcript:

```bash
cargo test -p horizon-browser-cli --test cli run_deadline_bounds_open_stdin_before_durable_setup -- --exact --nocapture
```

It must exit successfully, with the task-owned child exiting `124` within the
test's five-second guard and without creating a browser-job directory.

Run the partial-report deadline test as a second independent lane:

```bash
cargo test -p horizon-browser-cli --test cli run_deadline_persists_a_partial_report_and_stable_exit_code -- --exact --nocapture
```

It must prove exit `124`, one completed step, `stop_reason=deadline_exceeded`,
an in-flight-action warning in the partial report, and terminal `timed_out`
state even when the requested output cannot be written. The open-stdin lane
must use the phase-neutral deadline message without that warning.

## Preparation handoff and compatibility

```bash
cargo test -p horizon-browser-cli run_state::tests::deadline_abandons_active_preparation_without_publishing_running -- --exact --nocapture
cargo test -p horizon-browser-cli run_state::tests::abandoned_preparation_cannot_publish_running_state -- --exact --nocapture
cargo test -p horizon-browser-cli run_state::tests::caller_activates_a_conservatively_prepared_run_before_execution -- --exact --nocapture
cargo test -p horizon-browser-cli run_state::tests::state_accepts_jobs_created_before_execution_timeouts -- --exact --nocapture
```

Require all four to pass. Together they prove a deadline can abandon a worker
while its prepared result is still blocked, the late result remains
`timed_out` and cleans its staged activation file, the caller alone can publish
`running`, and pre-deadline state JSON still decodes.

## Cleanup and report

Remove only `smoke_home`, then report each lane as pass or fail and include the
exact SHA. A macOS report must end with:

```text
SMOKE-TEST: DONE
```
