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
user interface, and the rows that the sidebar shows for those agents: the fold,
the line of a lost link, the press that opens a session, the jump, the shortcut
and the notification.

## 1. Purpose

This procedure proves that this build reads the agents that run in `tmux` on one
machine, that it keeps their states by the rules of a local panel, and that it
starts and stops one `ssh` child per machine of the list.

## 2. Applicability

- Candidate: a source build of this branch, on the commit under test.
- Platforms: Linux and macOS as this computer. The machine under test is a Linux
  machine with `tmux`, or this computer itself when a test uses a private `tmux`
  server.
- This procedure tests sections 6.1 to 6.5 without a desktop. Sections 6.6 and
  6.7 need a desktop, because they read the sidebar and the notification of the
  system.
- This procedure does not test a machine that is behind a jump host.

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

3. Start Horizon. Read the machine section of the sidebar.

   Result: the row of the machine stands under the heading **Machines**, the
   agents of its `tmux` sessions stand under that row, and the snapshot of step 4
   of this section names the same agents.

### 6.6 MW-6 — The rows of the sidebar

1. Run the tests of the rows, of the press and of the notice:

   ```bash
   cargo test -j 8 -p horizon-ui machine_watch
   ```

   Result: every test passes within 30 seconds. Each test starts a private
   `tmux` server and a stand-in `ssh`.

2. Start two agents on the machine by hand, each in its own `tmux` session:

   ```bash
   ssh <machine> "tmux new-session -d -s hz-mw-a '<agent command>'"
   ssh <machine> "tmux new-session -d -s hz-mw-b '<agent command>'"
   ```

3. Read the sidebar within four seconds.

   Result: one row stands under the machine for each session. Each row shows the
   two letters of its kind, the name of the last part of the folder of its pane,
   and the word and the dot of its state. A working agent shows `Working` with a
   green dot. An agent that waits for an approval shows `Needs you`.

4. Begin a turn in one agent, and then close and open the lid of the laptop.

   Result: the row of the machine says `link lost just now`, and the place of
   the agent rows holds one line: `Last seen: 2 agents. Horizon tries again.`
   After the link comes back, the rows and their states return.

5. Press the chevron in front of the name of the machine.

   Result: the agent rows fold away, and the row of the machine says `1 needs you ·
   1 done`, else `2 working`, else nothing, in the place of its second line.
   Press the chevron again: the rows return.

6. Press the row of one agent.

   Result: Horizon opens a terminal in the workspace that carries the name of the
   machine, and that terminal holds the `tmux` session of the agent. Press the row
   again: the window goes to that terminal and opens no second one.

7. Begin a turn in one agent, focus another panel, and wait for the turn to end.

   Result: the row of the agent says `Done`. Press the shortcut for the next agent
   that waits for the person.

   Result: the window opens the terminal of that agent, and its row says no state.

8. Turn the setting off: **Settings**, **General**, **Machines**, clear **Show the
   agents of each machine**.

   Result: the agent rows leave the machine section within one frame, and the
   section keeps the row of the machine alone. Turn the setting on again: the rows
   return.

### 6.7 MW-7 — The notification of a change

> **CAUTION:** THIS STEP SENDS A NOTIFICATION THROUGH THE NOTIFICATION PROGRAM OF
> THE DESKTOP. A desktop that shows notifications of other work shows this one
> too.

1. Focus another window of the desktop, so no window of Horizon has the focus.

2. Begin a turn in one agent on the machine, and wait until it asks for an
   approval.

   Result: one notification of the system says `<agent> needs you · <machine>`.

3. Focus the window of Horizon again, and repeat step 2.

   Result: no notification arrives, because the sidebar shows the change.

## 7. Pass criteria

- Every test of section 6.1 to 6.6 passes.
- The tasks of section 6.6 and of section 6.7 show the results that they name.
- `cargo clippy -p horizon-core -p horizon-ui --all-targets -- -D warnings` passes.
- `./scripts/check-maintainability.sh` passes.
- No `ssh` child of a test stays after its test: `ps -A -o args= | grep -c hzw-ui-ssh`
  prints `0`.
- No `tmux` server of a test stays after its test: `tmux -L hzw-ui-<pid>-1 ls`
  prints `no server running`.

## 8. Cleanup

1. End the `tmux` sessions of MW-5 and MW-6. Run:

   ```bash
   ssh <machine> "tmux kill-session -t hz-mw-1"
   ssh <machine> "tmux kill-session -t hz-mw-a"
   ssh <machine> "tmux kill-session -t hz-mw-b"
   ```

   Result: `ssh <machine> "tmux list-sessions"` names none of them.

2. Remove the build folder of the test. Run:

   ```bash
   rm -rf "$CARGO_TARGET_DIR" "$TMPDIR"
   ```

   Result: both folders are gone.

## 9. Record of results

Put the result of each task in the pull request. Write a report in
`docs/testing/reports/` with [the report template](../reports/TEMPLATE.md) when
the run must be kept.
