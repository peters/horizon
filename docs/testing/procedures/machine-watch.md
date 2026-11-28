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
and the notification. It tests the state that the program of a pane reports in its
pane option, and the row of a job that reports one.

## 1. Purpose

This procedure proves that this build reads the agents that run in `tmux` on one
machine, that it keeps their states by the rules of a local panel, that it reads
the state that the program of a pane reports, and that it starts and stops one
`ssh` child per machine of the list.

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

3. Read the report of a pane that reports a state:

   ```bash
   cargo test -j 8 -p horizon-core --lib machine_watch::tests::remote::a_state_that_a_pane_reports \
     machine_watch::tests::remote::a_pane_that_reports
   ```

   Result: both tests pass. The first one sets the pane option `@horizon_state` of
   a Codex stand-in to `blocked` and reads the sixth field of its pane line, and
   the second one gives a report to a `sleep` pane.

   Read the same field by hand on the machine of MW-5. Run in one `tmux` session
   of the machine:

   ```bash
   tmux set-option -p -t "$TMUX_PANE" @horizon_state working
   ```

   Run the text of step 2 of MW-5 again. Result: the sixth field of the pane line
   of that session is `working`. Run
   `tmux set-option -p -u -t "$TMUX_PANE" @horizon_state` in the session, and run
   the text again: the field is empty, and the title or the screen answers again.

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

   Result: every test passes, including `a_first_sight_in_the_middle_of_a_turn_is_working_and_its_end_is_done`,
   `a_state_that_the_pane_reports_answers_before_the_title_and_the_screen` and
   `a_done_that_the_pane_reports_is_seen_as_a_done_of_the_title_is_seen`.

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

2. Read the report of the watch by hand, with the text of the watch. The remote
   command is the one word `sh`, and the text arrives on the input of `ssh`:

   ```bash
   sed -n '/^pub const REMOTE_TEXT/,/^"#;/p' \
     crates/horizon-core/src/machine_watch/remote.rs | sed '1d;$d' | ssh <machine> sh
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

4. Read the row of a job that reports its state. Run on the machine:

   ```bash
   ssh <machine> "tmux new-session -d -s hz-mw-job -c /tmp 'sleep 300'"
   ssh <machine> "tmux set-option -p -t hz-mw-job @horizon_state working"
   ```

   Result: the sidebar shows one row with the mark `>_`, the name `hz-mw-job` and
   the word `Working`. Press the row: Horizon opens the terminal of that session.
   Run `ssh <machine> "tmux set-option -p -t hz-mw-job @horizon_state blocked"`:
   the row says `Needs you`. Run
   `ssh <machine> "tmux set-option -p -u -t hz-mw-job @horizon_state"`: the row
   leaves within two seconds, because a pane with no report and no agent program
   is no row.

5. Begin a turn in one agent, and then close and open the lid of the laptop.

   Result: the row of the machine says `link lost just now`, and the place of
   the agent rows holds one line: `2 agents · trying again`. Its hover says
   `Last seen: 2 agents. Horizon tries again.` After the link comes back, the
   rows and their states return.

6. Press the chevron at the right end of the row of the machine.

   Result: the agent rows fold away, and the row of the machine says `1 needs you ·
   1 done`, else `2 working`, else nothing, in the place of its second line.
   Press the chevron again: the rows return.

7. Press the row of one agent.

   Result: Horizon opens a terminal in the workspace that carries the name of the
   machine, and that terminal holds the `tmux` session of the agent. Press the row
   again: the window goes to that terminal and opens no second one.

8. Begin a turn in one agent, focus another panel, and wait for the turn to end.

   Result: the row of the agent says `Done`. Press the shortcut for the next agent
   that waits for the person.

   Result: the window opens the terminal of that agent, and its row says no state.

9. Turn the setting off: **Settings**, **General**, **Machines**, clear **Show the
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

### 6.8 MW-8 — The watch over a real `sshd`, with no machine

This lane uses no rented machine. The machine of the fixture is one real `sshd` of
this computer, inside a namespace of its own, so the watch runs through the login
shell of the user and through the plain `tmux` of that machine.

1. Unpack an `sshd` into a folder of your own. Neither command needs root:

   ```bash
   apt-get download openssh-server
   dpkg-deb -x openssh-server_*.deb /var/tmp/openssh-server
   ```

   Result: `/var/tmp/openssh-server/usr/sbin/sshd` and the two session programs in
   `/var/tmp/openssh-server/usr/lib/openssh/` are files.

2. Start the fixture with the real machine. Use a new state folder:

   ```bash
   scripts/device-smoke/serve.py --horizon target/debug/horizon \
     --state /var/tmp/machine-watch-sshd-1 --native-view --machine-watch \
     --machine-sshd /var/tmp/openssh-server/usr/sbin/sshd
   ```

   Result: the run prints the VNC address and the manifest names the section
   `machine_sshd` with the port, the log and the stop list.

3. Show the isolated desktop in a Horizon Device panel of the current workspace,
   with the `device_panel` tool of the `horizon-device` skill.

   Result: the panel shows the desktop of the fixture, and its image and its frame
   sequence change while you look.

4. Read the rows of the machine section of the sidebar.

   Result: the machine `build-1` holds `1 needs you` and `1 done` of the two Codex
   stand-ins, as MW-6 shows it. The watch of these rows came through the login
   shell of the machine and through `ssh`, not through a stand-in.

5. Read the report of the watch by hand. The configuration of the machine lies in
   the private home of the run:

   ```bash
   sed -n '/^pub const REMOTE_TEXT/,/^"#;/p' \
     crates/horizon-core/src/machine_watch/remote.rs | sed '1d;$d' | \
     ssh -F /var/tmp/machine-watch-sshd-1/data/home/.ssh/config \
       -i /var/tmp/machine-watch-sshd-1/machine-sshd/fixture_key build-1 sh
   ```

   Result: the first line is `HZ	1`, one `S` line names `hz-shop`, `hz-notes`,
   `hz-night` and `hz-relay`, one `P` line names each pane, and the report ends with
   `Z`. The text runs because the login shell of the machine runs the word `sh`.

6. Read the log of the machine:

   ```bash
   grep -c 'Accepted publickey' /var/tmp/machine-watch-sshd-1/machine-sshd/sshd.log
   ```

   Result: the number is not `0`, and the log names the user of this computer.

7. Start one more session on the machine, and wait two seconds:

   ```bash
   ssh -F /var/tmp/machine-watch-sshd-1/data/home/.ssh/config \
     -i /var/tmp/machine-watch-sshd-1/machine-sshd/fixture_key build-1 \
     'tmux new-session -d -s hz-mw-late'
   ```

   Result: a row of `hz-mw-late` appears in the sidebar within two seconds, and the
   report of step 5 names it.

8. Read the `/tmp` of this computer and the `/tmp` of the machine:

   ```bash
   ls /tmp/tmux-$(id -u)/
   ls /var/tmp/machine-watch-sshd-1/machine-sshd/tmp/tmux-$(id -u)/
   ```

   Result: the second folder holds the server of the machine, and no server of the
   machine stands in the first one. No session of the machine touches this computer.

9. Stop the sshd of the run, and read the rows of the machine section.

   ```bash
   kill "$(cat /var/tmp/machine-watch-sshd-1/machine-sshd/sshd.pid)"
   ```

   Result: the row of the machine says that the link was lost and Horizon tries
   again, and the agent rows of the machine leave the section.

10. End the fixture with `Ctrl+C` in its terminal, and start it again with
    `--reuse-state` and the same options.

   Result: the run starts the machine again and its rows return. The run ends the
   `sshd` and the `tmux` server of the machine at its own exit.

## 7. Pass criteria

- Every test of section 6.1 to 6.7 passes, including the tests of the state that a
  pane reports and of the row of a job.
- The tasks of section 6.6, of section 6.7 and of section 6.8 show the results that
  they name.
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
   ssh <machine> "tmux kill-session -t hz-mw-job"
   ```

   Result: `ssh <machine> "tmux list-sessions"` names none of them.

2. Remove the state folder of MW-8, the sshd that you unpacked, and the build folder
   of the test. Run:

   ```bash
   rm -rf /var/tmp/machine-watch-sshd-1 /var/tmp/openssh-server
   rm -rf "$CARGO_TARGET_DIR" "$TMPDIR"
   ```

   Result: all four folders are gone, and no socket file and no `tmux` server of a
   machine stays on this computer.

## 9. Record of results

Put the result of each task in the pull request. Write a report in
`docs/testing/reports/` with [the report template](../reports/TEMPLATE.md) when
the run must be kept.
