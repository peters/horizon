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

The whole workspace suite of 7121 tests took 33.9 minutes of test time and 1.3
minutes of wall clock. The run had 48 test threads and 57 tests were skipped.
The fastest 3000 tests together took 32 seconds, which is 1.6 percent of the
test time. The slowest 103 tests took 48.9 percent. A count of tests therefore
does not say what the suite costs, and the 108 slowest tests are the only
useful target.

The run needed one fix first. The test binary `webauthn_live` started both
browsers when it answered the `--list` request from the test runner, so no run
that lists the tests could start. The same pull request changes the binary to
answer the request without side effects.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| D01 | pass | `scripts/test_test_duration_baseline.py` ended with `OK` and 23 tests. | — |
| D02 | pass | The whole workspace ran 7121 tests: 7121 passed, 57 skipped, 8 slow. | — |
| D03 | pass | The report of the run named the crate of every test, and no test needed a name fallback. | — |
| D04 | pass | The script wrote a raw result file with the time of every test. | — |

## 3. Defects

- No open defect. The pull request fixes the `--list` defect that D02 found
  before the run.

## 4. Deviations from the procedure

- No procedure document exists for this measurement. The script
  `scripts/test-duration-baseline.py` holds the method.
- The report template asks for a frozen candidate and its hash. This run
  measured a tree, not a frozen binary, because a test run builds the binaries
  it measures.
- The machine had 48 test threads. A computer with fewer cores takes more wall
  clock for the same test time, and CI runners have two or four cores.

## 5. Cleanup

- No resource. The run used the build cache of the main checkout and wrote its
  two result files to `/tmp`.

## 6. Evidence

- Rust 1.99.0, cargo 1.99.0, cargo-nextest 0.9.149, Python 3.14.4, Linux.
- AMD Ryzen Threadripper PRO 5965WX, 48 test threads.
- Other builds of this repository ran on the same computer during the day. A
  build of another checkout was active after this run. If a build shared the
  computer with the run, the wall clock is longer than the test time alone
  needs. The comparison in section 7.4 is a set of shares of one run, so load
  changes it much less than it changes wall clock.
- The command was `CARGO_TARGET_DIR=<main checkout>/target
  python3 -B scripts/test-duration-baseline.py -v --json <file> --top 40
  --crates 20`.
- A first `cargo test --workspace` run failed 2 of 2348 tests of the horizon-ui
  binary target in `app::device_requests::tests::reveal_wait`. Both passed in
  isolation in 1.3 seconds and in the next full run, with a build of another
  checkout active. They are sensitive to load; this pull request changes no file
  of that crate.

## 7. Numbers

| Measurement | Value |
|---|---|
| Tests measured | 7121 |
| Test time, one test at a time | 33.9 min |
| Wall clock, 48 threads | 1.3 min |
| Effective parallelism | 26.1x |
| Skipped tests | 57 |
| Slowest 108 tests | half of the test time |
| Slowest 373 tests | 80 percent of the test time |

### 7.1 Test time by crate

| Crate | Tests | Test time | Share |
|---|---|---|---|
| horizon-ui | 2355 | 17.1 min | 50.4% |
| horizon-core | 2141 | 7.7 min | 22.7% |
| horizon-cloud-worker | 210 | 4.5 min | 13.3% |
| horizon-app-host | 121 | 1.1 min | 3.3% |
| horizon-browser-cli | 178 | 46.9 s | 2.3% |
| All other crates | 2116 | 1.6 min | 8.0% |

### 7.2 Test time by target kind

| Kind | Tests | Test time | Share |
|---|---|---|---|
| bin | 2241 | 19.5 min | 57.6% |
| lib | 3083 | 9.0 min | 26.6% |
| test | 1797 | 5.3 min | 15.7% |

### 7.3 Test time by duration

| Tests at or above | Count | Test time | Share |
|---|---|---|---|
| 0.1 s | 1246 | 31.5 min | 93.0% |
| 1 s | 389 | 27.4 min | 80.8% |
| 5 s | 103 | 16.6 min | 48.9% |
| 10 s | 25 | 8.1 min | 23.8% |
| 30 s | 3 | 2.3 min | 6.8% |

### 7.4 What a smaller count would save

| Tests removed, fastest first | Test time of those tests | Share of the suite |
|---|---|---|
| 1000 | 6.5 s | 0.3% |
| 2000 | 16.5 s | 0.8% |
| 3000 | 32.2 s | 1.6% |
| 4000 | 54.9 s | 2.7% |
| 5000 | 88.5 s | 4.3% |
| 6000 | 2.6 min | 7.7% |

Half of the tests cost less than 2 percent of the suite. A pull request that
merges them into one test changes a count, not a duration, and it costs a
review of every one of them.

### 7.5 The slowest tests

| Test | Crate | Time |
|---|---|---|
| `cloud_runtime::bootstrap_initialization::tests::project_reservations::namespaces::source::setup::recovery::project_setup_capacity_reserves_unstarted_plans_and_counts_progress_once` | horizon-core | 1.1 min |
| `cloud_runtime::bootstrap_initialization::tests::project_reservations::namespaces::source::setup::recovery::project_setup_capacity_releases_terminal_future_work_but_keeps_live_cleanup` | horizon-core | 36.0 s |
| `cloud_runtime::bootstrap_initialization::tests::project_reservations::namespaces::source::setup::recovery::project_setup_capacity_includes_retained_low_level_history` | horizon-core | 34.5 s |
| `bootstrap::initialize::tests::membership::namespaces::source::sessions::publication_boundary_recovery_never_resets_exposed_data_or_restarts_uncertain_builds` | horizon-cloud-worker | 30.0 s |
| `bootstrap::initialize::tests::membership::namespaces::source::source_recovers_stream_and_publication_without_resetting_data` | horizon-cloud-worker | 27.1 s |
| `app::press_frame::tests::a_titlebar_drag_with_a_frame_per_event_moves_the_panel_in_every_direction_region_and_zoom` | horizon-ui | 23.5 s |
| `bootstrap::initialize::tests::membership::namespaces::source::sessions::writable_sessions_preserve_edits_commits_config_and_siblings_after_restart` | horizon-cloud-worker | 22.5 s |
| `app::press_frame::tests::a_titlebar_drag_in_one_batch_moves_the_panel_in_every_direction_region_and_zoom` | horizon-ui | 20.6 s |
| `app::cloud_panel::production::creation_tests::profiles::new_cloud_placement_is_relative_to_translated_workspace` | horizon-ui | 19.5 s |
| `cloud_runtime::bootstrap_initialization::tests::project_reservations::namespaces::source::setup::recovery::project_setup_foreign_pending_and_terminal_stop_do_not_progress` | horizon-core | 19.0 s |

The `--top 40` run of the script names 40 tests. The raw result file names
every test.

## 8. What the numbers mean

- A test counts in the wall clock only when it is on the critical path. With 48
  threads, the slowest test alone costs 1.1 minutes of the 1.3 minute run.
- 25 tests of 10 seconds or more are 23.8 percent of the test time. Each one is
  a candidate for a shorter deadline, a fake clock or a smaller loop count.
- horizon-ui holds half of the test time. 2011 of its 2355 tests run inside the
  `horizon` binary target, and they take 14.4 minutes.
- The two slowest horizon-ui tests are `app::press_frame::tests::a_titlebar_drag_*`,
  at 23.5 s and 20.6 s.
- A test that costs one millisecond is not worth a review of its own. A test of
  20 seconds is worth one.
