---
procedure: scripts/test-duration-baseline.py (no procedure document yet)
candidate_commit: b25ae1b31 (origin/main) plus the changes of the pull request that adds this report
candidate_sha256: no frozen candidate; the run used a cargo build of the tree above
date: 2026-10-24
lanes: [linux]
issue: the pull request that adds this report
---

# Test suite duration baseline, 2026-10-24

## 1. Summary

The whole workspace suite of 7121 tests took 54.1 minutes of test time and 3.0
minutes of wall clock on 48 test threads. No test failed. The slowest 35 tests
carried half of the test time, and the fastest 3000 tests carried 0.7 percent of
it.

The same suite took 33.9 minutes of test time on a machine with a lighter load.
The absolute time follows the load of the computer, but the shape does not: in
both runs a small set of tests held most of the time and most tests held almost
none. The slow tests are the ones that wait for a real process, a real socket or
a real deadline, and those tests get slower when the machine is busy.

The run needed one fix first. The test binary `webauthn_live` started both
browsers when it answered the `--list` request from the test runner, so no run
that lists the tests could start. The same pull request changes the binary to
answer the request without side effects.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| D01 | pass | `scripts/test_test_duration_baseline.py` ended with `OK` and 30 tests. | — |
| D02 | pass | The whole workspace ran 7121 tests and no test failed. | — |
| D03 | pass | The run named the crate of every test, and no test needed a name fallback. | — |
| D04 | pass | The script wrote a raw result file with the time of every test. | — |
| D05 | pass | The report named the target kind of every test from the map of suites. | — |

## 3. Defects

- No open defect. The pull request fixes the `--list` defect that D02 found
  before the run could start.
- Two tests of `horizon-ui` failed one earlier full-workspace run under heavy
  load and passed in isolation and in every later run. Section 6 records them.

## 4. Deviations from the procedure

- No procedure document exists for this measurement. The script
  `scripts/test-duration-baseline.py` holds the method.
- The report template asks for a frozen candidate and its hash. This run
  measured a tree, not a frozen binary, because a test run builds the binaries
  it measures.
- The machine had 48 test threads and other builds of this repository ran during
  the day. Section 7.6 gives the spread of three runs.

## 5. Cleanup

- No resource. The run used the build cache of the task worktree and wrote its
  result files to `/tmp`.

## 6. Evidence

- Rust 1.99.0, cargo 1.99.0, cargo-nextest 0.9.149, Python 3.14.4, Linux.
- AMD Ryzen Threadripper PRO 5965WX, 48 test threads.
- The command was `CARGO_TARGET_DIR=<task worktree>/target
  python3 -B scripts/test-duration-baseline.py --json <file> --top 40
  --crates 20`.
- One earlier `cargo test --workspace` run failed 2 of 2348 tests of the
  horizon-ui binary target, in `app::device_requests::tests::reveal_wait`. Both
  passed alone in 1.3 seconds and in the next full run. A build of another
  checkout was active at the time. This pull request changes no file of that
  crate.

## 7. Numbers

| Measurement | Value |
|---|---|
| Tests measured | 7121 |
| Test time, one test at a time | 54.1 min |
| Wall clock, 48 threads | 3.0 min |
| Effective parallelism | 17.8x |
| Slowest 35 tests | half of the test time |
| Slowest 73 tests, 5 s or more | 59.0 percent |

### 7.1 Test time by crate

| Crate | Tests | Test time | Share |
|---|---|---|---|
| horizon-cloud-worker | 210 | 20.1 min | 37.1% |
| horizon-ui | 2355 | 11.9 min | 22.0% |
| horizon-core | 2141 | 10.7 min | 19.8% |
| horizon-app-host | 121 | 4.3 min | 7.9% |
| horizon-cloud | 491 | 3.5 min | 6.4% |
| horizon-browser-cli | 178 | 1.0 min | 1.9% |
| All other crates | 1625 | 2.7 min | 4.9% |

210 tests of `horizon-cloud-worker` take more time than the 2141 tests of
`horizon-core`.

### 7.2 Test time by target kind

| Kind | Tests | Test time | Share |
|---|---|---|---|
| bin | 2575 | 32.0 min | 59.1% |
| lib | 4362 | 20.1 min | 37.1% |
| test | 184 | 2.0 min | 3.8% |

The tests of `horizon-ui` run in its binary target: 2350 of its 2355 tests. The
integration targets carry 184 tests in the whole workspace.

### 7.3 Test time by duration

| Tests at or above | Count | Test time | Share |
|---|---|---|---|
| 0.1 s | 1672 | 52.3 min | 96.8% |
| 1 s | 443 | 46.1 min | 85.1% |
| 5 s | 73 | 31.9 min | 59.0% |
| 10 s | 42 | 28.5 min | 52.6% |
| 30 s | 18 | 21.3 min | 39.3% |

18 tests of 30 seconds or more carry 39.3 percent of the suite.

### 7.4 What a smaller count would save

| Tests removed, fastest first | Test time of those tests | Share of the suite |
|---|---|---|
| 1000 | 5.6 s | 0.2% |
| 2000 | 13.2 s | 0.4% |
| 3000 | 23.2 s | 0.7% |
| 4000 | 38.1 s | 1.2% |
| 5000 | 70.0 s | 2.2% |
| 6000 | 3.1 min | 5.7% |

Half of the tests cost less than 1 percent of the suite. A pull request that
merges them into one test changes a count, not a duration, and it costs a
review of every one of them.

### 7.5 The slowest tests

| Test | Crate | Time |
|---|---|---|
| `tailnet::bindings::tests::repeated_saved_network_deletion_does_not_exhaust_api_metadata` | horizon-cloud | 175 s |
| `bootstrap::initialize::tests::membership::namespaces::source::sessions::publication_boundary_recovery_never_resets_exposed_data_or_restarts_uncertain_builds` | horizon-cloud-worker | 131 s |
| `bootstrap::initialize::tests::membership::namespaces::source::sessions::writable_sessions_preserve_edits_commits_config_and_siblings_after_restart` | horizon-cloud-worker | 130 s |
| `bootstrap::initialize::tests::membership::namespaces::source::source_recovers_stream_and_publication_without_resetting_data` | horizon-cloud-worker | 108 s |
| `bootstrap::initialize::tests::membership::namespaces::source::sessions::changed_ready_staging_and_replaced_child_roots_are_never_acknowledged` | horizon-cloud-worker | 83 s |
| `bootstrap::initialize::tests::membership::namespaces::source::agent_sessions_survive_restart_without_creating_runtime_and_cancellation_retains_siblings` | horizon-cloud-worker | 80 s |
| `actor::tests::repeated_provider_admission_retains_bounded_history_and_live_allocations` | horizon-app-host | 73 s |
| `bootstrap::initialize::tests::membership::namespaces::source::session_publication_recovers_every_manifest_boundary_and_rechecks_source_before_commit` | horizon-cloud-worker | 67 s |
| `bootstrap::initialize::tests::membership::namespaces::source::session_grants_revision_identity_and_capacity_reject_without_consuming_cancellation` | horizon-cloud-worker | 66 s |
| `cloud_runtime::bootstrap_initialization::tests::project_reservations::namespaces::source::setup::recovery::project_setup_capacity_reserves_unstarted_plans_and_counts_progress_once` | horizon-core | 64 s |
| `bootstrap::initialize::tests::membership::namespaces::source::sessions::content_hashing_is_bounded_and_changed_source_before_publication_is_rejected` | horizon-cloud-worker | 48 s |
| `actor::tests::provider_retention_uses_legacy_completed_records_after_actor_restart` | horizon-app-host | 46 s |

The `--top 40` run of the script names 40 tests. The raw result file names every
test.

### 7.6 The spread of three runs

| Run | Test time | Wall clock | Tests for half the time | Fastest 3000 |
|---|---|---|---|---|
| Quiet machine | 33.9 min | 1.3 min | 108 | 1.6% |
| This report | 54.1 min | 3.0 min | 35 | 0.7% |
| Busy machine | 71.5 min | 6.3 min | — | 0.7% |

The ranking of the crates moves between the runs: `horizon-cloud-worker` held
13.3 percent of the time in the quiet run and 37.1 percent here. A test that
waits for a socket or a process pays for the time that the scheduler takes to
give it one, so a busy machine makes those tests slower than a quiet one.

The durable parts of the measurement are these:

- About half of the test time sits in 35 to 108 tests, which is less than 1.5
  percent of the suite.
- The fastest half of the suite costs less than 2 percent of the time in every
  run.

## 8. What the numbers mean

- A test counts in the wall clock only when it is on the critical path. With 48
  threads, one test of 175 seconds is more than half of a 3.0 minute run.
- 42 tests of 10 seconds or more are 52.6 percent of the test time. Each one is a
  candidate for a shorter deadline, a fake clock, an in-process fake or a smaller
  loop count.
- The tests that move most between runs are the ones that do real work against a
  socket, a process or a file. They are the same tests that hold most of the
  time, so one change to a shared fixture or a deadline helps twice.
- A test that costs one millisecond is not worth a review of its own. A test of
  64 seconds is worth one.
