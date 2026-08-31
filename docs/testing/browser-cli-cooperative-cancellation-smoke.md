# Browser CLI cooperative cancellation smoke

Temporary cross-machine plan for the cooperative-cancellation PR. Run every
lane on the exact requested head. The tests signal only task-owned child
processes; do not stop or reuse any pre-existing Horizon process.

## Preconditions

- A clean checkout of the requested PR commit.
- The repository build prerequisites for the host OS.
- No release, crate publication, or live Horizon window is required.

Record the candidate:

```bash
git rev-parse HEAD
git status --short
```

The SHA must match the newest smoke request and the checkout must be clean.

## Focused package gate

```bash
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test -p horizon-browser-cli
```

The suite must pass, including the Unix process-level interrupt tests. It also
proves that the action deadline starts only after plan input and validation,
and that a blocking action is dispatched before the deadline test relies on
its timeout.

## Active MCP action

```bash
RUSTFLAGS="-D warnings" cargo test -p horizon-browser-cli --test cli \
  interrupt_persists_cancelled_partial_report_and_exit_130 -- --exact --nocapture
```

This must prove that the task-owned CLI process completes one step, dispatches
a second blocking action, then exits 130 after SIGINT. Its JSON report must
contain `completed_steps: 1` and `stop_reason: "cancelled"`; `state.json` must
also be `cancelled` and reference the saved report. The message must warn that
the already-dispatched browser action may still complete.

## Input and finalization boundaries

```bash
RUSTFLAGS="-D warnings" cargo test -p horizon-browser-cli --test cli \
  interrupt_bounds_open_stdin_before_durable_setup -- --exact --nocapture
RUSTFLAGS="-D warnings" cargo test -p horizon-browser-cli --test cli \
  interrupt_terminates_blocked_report_finalization -- --exact --nocapture
```

The open-stdin lane must exit 130, emit no report, leave no browser-job
directory, and use a phase-neutral cancellation message. The FIFO lane must
exit 130 within the harness guard rather than hanging on report delivery; its
already-completed durable state may remain `succeeded`.

## Priority and durable encoding

```bash
RUSTFLAGS="-D warnings" cargo test -p horizon-browser-cli --lib \
  execution_control::tests::repeated_cancellation_is_idempotent_and_wins_a_ready_deadline \
  -- --exact --nocapture
RUSTFLAGS="-D warnings" cargo test -p horizon-browser-cli --lib \
  execution_control::tests::cancellable_control_arms_its_deadline_later \
  -- --exact --nocapture
RUSTFLAGS="-D warnings" cargo test -p horizon-browser-cli --lib \
  run_state::tests::cancellation_is_a_distinct_terminal_state \
  -- --exact --nocapture
RUSTFLAGS="-D warnings" cargo test -p horizon-browser-cli --bin horizon-browser \
  run_preparation::tests::cancellation_drains_a_late_worker_result_before_returning \
  -- --exact --nocapture
```

These lanes prove idempotence and cancellation priority, delayed deadline
arming, distinct durable state, and draining of a late preparation result.

## Report

Post the result on the PR using this shape:

```text
SMOKE-TEST REPORT (<machine/os>)
- exact head and clean checkout: pass | fail — <SHA and note>
- focused package gate: pass | fail — <note>
- active-action SIGINT and partial report: pass | fail — <note>
- open-stdin and blocked-output SIGINT: pass | fail — <note>
- priority, deadline boundary, and durable state: pass | fail — <note>
Summary: <fixes pushed, remaining findings, or no findings>
SMOKE-TEST: DONE
```

If a finding is fixed, rerun every affected lane on the new exact head and
report only that final head.
