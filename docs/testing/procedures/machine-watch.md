---
procedure: machine-watch
feature: The watch of the agents of the machines of the machine list
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Machine watch test procedure

This procedure tests the part of the watch of the agents of a machine that has no
user interface. The sidebar row, the click that opens a panel and the
notification come in the next round, and section 6.6 gives their place.

## 1. Purpose

This procedure proves that this build reads the agents that run in `tmux` on one
machine, that it keeps their states by the rules of a local panel, and that it
starts and stops one `ssh` child per machine of the list.

## 2. Applicability

- Candidate: a source build of this branch, on the commit under test.
- Platforms: Linux and macOS as this computer. The machine under test is a Linux
  machine with `tmux`, or this computer itself when a test uses a private `tmux`
  server.
- This procedure does not test: the sidebar row, the click on an agent, the
  desktop notification and the jump to the panel. It does not test a machine that
  is behind a jump host.

## 3. Safety

> **CAUTION:** THE WATCH STARTS `ssh` AND THE TEXT OF THE WATCH ON THE MACHINE.
> Use a test machine of your own. Do not point the test at a machine that another
> person uses, because the text reads the panes of that user.

> **CAUTION:** A PRIVATE `TMUX` SERVER ENDS WITH `tmux -L <socket> kill-server`.
> That command ends every session of that socket. Type the socket name of the
> test and no other.

## 4. Equipment and preconditions

- A checkout of this branch.
- `tmux`, `dash`, `bash` and `python3` on this computer. `busybox` is optional.
- The OpenSSH client `ssh` in the `PATH` of this computer.
- One machine record in the machine list of a test Horizon home, with a route
  that `ssh` accepts and a machine key or an agent of the person. A record of a
  machine that `ssh` refuses is also useful, for the refusal lane.
- A terminal on this computer. Every step reads a log or a test name; no step
  needs a desktop.

## 5. Setup

1. Set the build folder of the test. Run:

   ```bash
   export CARGO_TARGET_DIR=/var/tmp/hz-machine-watch-target
   export TMPDIR=/var/tmp/hz-machine-watch-tmp
   export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
   mkdir -p "$TMPDIR"
   ```

   Result: both folders exist.

2. List the tests of the watch. Run:

   ```bash
   cargo test -j 8 -p horizon-core --lib machine_watch -- --list
   ```

   Result: the output names 48 tests, each with `: test` at the end.

## 6. Tasks

### 6.1 MW-1 — The text of the watch over a private `tmux` server

1. Run the tests of the text:

   ```bash
   cargo test -j 8 -p horizon-core --lib machine_watch::tests::remote
   ```

   Result: every test passes. `every_shell_of_this_computer_reports_the_panes_of_a_server`
   names each shell that this computer has.

2. Read the log of `a_working_line_that_a_pane_prints_is_one_of_its_last_rows`.
   It passes without output.

   Result: the test found the working line of the pane among the rows that the
   text reported.

### 6.2 MW-2 — The parser and the bounds

1. Run the tests of the parser:

   ```bash
   cargo test -j 8 -p horizon-core --lib machine_watch::tests::report
   ```

   Result: every test passes, and `the_bounds_of_a_report_drop_it_whole` names
   each bound.

### 6.3 MW-3 — The state of one agent

1. Run the tests of the state:

   ```bash
   cargo test -j 8 -p horizon-core --lib machine_watch::tests::state
   ```

   Result: every test passes, including `a_first_sight_in_the_middle_of_a_turn_is_working_and_its_end_is_done`.

### 6.4 MW-4 — The supervisor and its snapshot

1. Run the tests of the supervisor:

   ```bash
   cargo test -j 8 -p horizon-core --lib machine_watch::tests::supervisor
   ```

   Result: every test passes within 30 seconds. The test
   `a_link_that_drops_comes_back_by_the_rule_of_a_lost_link` needs the first
   delay of the reconnect rule, which is 3 seconds.

2. Run the tests of the snapshot and of the open command:

   ```bash
   cargo test -j 8 -p horizon-core --lib machine_watch::tests::snapshot
   cargo test -j 8 -p horizon-core --lib machine_watch::tests::open
   ```

   Result: both runs pass.

### 6.5 MW-5 — The watch of a real machine

> **CAUTION:** THIS STEP STARTS `ssh` TO THE MACHINE OF THE RECORD. Use a test
> machine of your own.

1. Start one `tmux` session on the machine, with one agent:

   ```bash
   ssh <machine> "tmux new-session -d -s hz-mw-1 '<agent command>'"
   ```

   Result: `ssh <machine> "tmux list-sessions"` names `hz-mw-1`.

2. Read the report of the watch by hand, with the text of the watch:

   ```bash
   ssh <machine> "$(sed -n '/^pub const REMOTE_TEXT/,/^"#;/p' \
     crates/horizon-core/src/machine_watch/remote.rs | sed '1d;$d')"
   ```

   Result: the first line is `HZ	1`, one `P` line names `hz-mw-1`, and the last
   line of the report is `Z`.

3. Start Horizon with the setting `machines.watch_agents` and read the snapshot
   through the next round, when the sidebar row exists. Until then, this step is
   not run, and the report of the run says so.

### 6.6 MW-6 — The sidebar, the click and the notice (next round)

This procedure gains these tasks when the sidebar row of a machine shows its
agents:

- MW-6.1 The sidebar shows the agent of MW-5 within two seconds.
- MW-6.2 A turn that ends while the person looks elsewhere says **Done**.
- MW-6.3 A click on the row opens the panel of that `tmux` session, and the row
  says no state after the click.
- MW-6.4 A denied approval says **Needs you** until the agent stops its wait.
- MW-6.5 A desktop notification tells a row that changed to **Needs you** while
  no window of Horizon has the focus.

## 7. Pass criteria

- Every test of section 6.1 to 6.4 passes.
- `cargo clippy -p horizon-core --all-targets -- -D warnings` passes.
- `./scripts/check-maintainability.sh` passes.
- No `ssh` child of a test stays after its test: `ps -A -o args= | grep -c hzw-child`
  prints `0`.

## 8. Cleanup

1. End the `tmux` session of MW-5. Run:

   ```bash
   ssh <machine> "tmux kill-session -t hz-mw-1"
   ```

   Result: `ssh <machine> "tmux list-sessions"` does not name `hz-mw-1`.

2. Remove the build folder of the test. Run:

   ```bash
   rm -rf "$CARGO_TARGET_DIR" "$TMPDIR"
   ```

   Result: both folders are gone.

## 9. Record of results

Put the result of each task in the pull request. Write a report in
`docs/testing/reports/` with [the report template](../reports/TEMPLATE.md) when
the run must be kept.
