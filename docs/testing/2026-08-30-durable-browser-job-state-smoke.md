# Durable Browser Job State Smoke

Validate the exact PR head on macOS. This slice changes only deterministic
`horizon-browser run` persistence; it does not change browser rendering,
backend behavior, or prompt-driven jobs.

## Setup

1. Check out the exact PR head and record `git rev-parse HEAD`.
2. Run `cargo build -p horizon-browser-cli`.
3. Create a disposable directory outside the repository and use it as `HOME`
   for every command below. Do not inspect or modify the user's normal Horizon
   state.
4. Create `list.json` in the disposable directory:

   ```json
   {"version":1,"steps":[{"id":"panels","tool":"browser_list"}]}
   ```

## Successful run

1. Run `HOME=<disposable> RUST_LOG=off target/debug/horizon-browser run <disposable>/list.json` and capture stdout and stderr separately.
2. Require exit 0, empty stderr, and one parseable JSON object on stdout.
3. Require non-empty `job_id`, `job_dir`, and `state_path`; `job_dir` must be
   exactly beneath `<disposable>/.horizon/browser-jobs/`, and `state_path` must
   equal `<job_dir>/state.json`.
4. Require `<job_dir>/plan.json`, `state.json`, and `report.json` to exist.
5. Require `state.json` to report version 1, the same job id, `succeeded`, one
   completed step, `plan.json`, and `report.json`.
6. Require the saved plan to equal the validated input and the private report
   to equal stdout JSON.
7. Require mode 0700 for the job directory and 0600 for all three files.

## Output-file run

1. Repeat with `--output <disposable>/copied-report.json`.
2. Require exit 0 and empty stdout/stderr.
3. Require the copied report to equal the new job's private `report.json`, and
   require mode 0600 for both.

## Preflight failure

1. Create `invalid.json` with one step whose tool is `browser_missing`.
2. Run it with the same isolated `HOME`.
3. Require a non-zero exit, empty stdout, and stderr that names both the stable
   job id and its exact state path without including plan arguments.
4. Require the new state to be `failed` with zero completed steps and a bounded
   unavailable-tool error. `plan.json` must exist and `report.json` must not.

## Non-UTF-8 path regression

Run `cargo test -p horizon-browser-cli run_state::tests::non_utf8_report_paths_are_json_safe -- --exact`.
Require the test to pass; it proves durable report paths remain valid JSON when
a Unix home path is not valid UTF-8. Apple filesystems reject invalid UTF-8
names before Horizon can create a job root; the separate lifecycle test proves
successful report persistence records a terminal state.

## Cleanup and report

Remove only the disposable directory created for this smoke. Report every lane
as pass or fail, include the exact tested head, and finish the PR comment with:

`SMOKE-TEST: DONE`
