# The watch of the agents of a machine

A person rents a machine, starts agents there in `tmux` and closes the panels or
the laptop lid. This page describes the watch that names those agents in the
sidebar of Horizon without a panel. The core part is in this build. The sidebar
row, the click that opens a panel and the notification come in the next rounds.

The page states the flow, the format of one report, the bounds of an untrusted
machine, and the state rules that the watch shares with a local panel. The trust
boundary is
[the boundary page](../review/trust-boundaries/machine-watch.md). The module
comment of
[`crates/horizon-core/src/machine_watch.rs`](../../crates/horizon-core/src/machine_watch.rs)
states the same rules for a reader of the code.

## The flow

1. The machine list changes, or the setting changes, and the window calls
   `MachineWatch::reconcile` on a worker thread.
2. For each machine of the list, within the limit of
   `MAX_WATCHED_MACHINES` (8), the watch builds the `ssh` command line of that
   machine (`WatchTarget::of`). The record passes the rule of an SSH terminal
   again, as it does for a terminal of that machine.
3. One worker thread per machine starts that `ssh` child. The child runs one
   constant text of POSIX shell on the machine
   (`crates/horizon-core/src/machine_watch/remote.rs`).
4. The reader of the thread reads the standard output of the child line by line
   with the bound of one line, and collects one report until its terminator line.
5. The parser turns the report into a typed value with the bounds, and the model
   keeps one row for each pane whose program is an agent kind.
6. The shared state publishes a new snapshot only when a link or a row changed.
   A frame reads `MachineWatch::snapshot`, which is one `Arc`, and compares its
   version with the version that it holds.
7. An end of the child is a lost link. The watch waits the delay of
   `place/reconnect.rs` and starts the next attempt. A stop kills the child and
   joins the reader.

No machine in the list, and the setting `machines.watch_agents` off, start no
thread and no process. A ninth machine of the list is not watched, and its link
says `OverTheLimit`.

## The remote text

The text is one constant of this build, and it is 60 statements of POSIX shell.
It runs under `dash`, under `bash` and under the shell of `BusyBox`, and its tests
run it under each shell that this computer has. It uses `tmux`, `ps` and the
built-ins of the shell.

The loop prints one report, waits `WATCH_INTERVAL_SECS` (2) seconds, and builds
the next report. It prints that report only when it differs from the report
before it: a machine whose report did not change prints the short line `B`
instead. A machine with no `tmux`, or with no `tmux` server, prints one note and
repeats it after `SLOW_INTERVAL_SECS` (30) seconds.

The text reads the panes of every `tmux` session of that user:

| What | Where it comes from |
|---|---|
| The session of the pane | `#{session_name}` |
| The pane id | `#{pane_id}` |
| The foreground program | `#{pane_current_command}` |
| The folder | `#{pane_current_path}` |
| The title | `#{pane_title}` |
| The command lines of the pane, when an interpreter runs there | `ps -t` of `#{pane_tty}` |
| The last six rows of the screen | `tmux capture-pane -p` |

`tmux` keeps the title of every pane with no setting, which is why a Codex pane
reports its own state there. A pane that runs an interpreter such as `node` or
`python3` names the interpreter and not the program, so the text reads the
command lines of the processes of that terminal and reports the newest three.
The model finds the agent kind in the first words of those lines.

Every field of a report is one line of words. The text replaces each run of
spaces, tabs and newlines of a field by one space, so a value that a program or a
person put into a session name, a folder name or a title cannot break a line.
`tmux` filters the control characters of a title itself, and it escapes them in a
session name.

## The format of one report

One report holds these lines, separated by a tab:

| Line | Fields |
|---|---|
| `HZ` | The version of the format, which is `1`. |
| `P` | The pane id, the session, the program, the folder and the title. |
| `A` | The pane id and one command line of the processes of its terminal. |
| `R` | The pane id and one row of its screen. |
| `X` | The note `no-tmux` or `no-server`. |
| `Z` | Nothing. This line ends the report. |

The line `B` is a heartbeat and is no report: the machine answered and its report
did not change.

## The bounds

| Bound | Constant | Value |
|---|---|---|
| The bytes of one line | `MAX_LINE_BYTES` | 8192 |
| The bytes of one report | `MAX_REPORT_BYTES` | 256 KiB |
| The lines of one report | `MAX_REPORT_LINES` | 704 |
| The panes of one machine | `MAX_PANES` | 64 |
| The rows of one pane | `MAX_ROWS` | 6 |
| The command lines of one pane | `MAX_COMMAND_LINES` | 3 |
| The characters of the name of an agent | `MAX_NAME_CHARS` | 64, cut |
| The bytes of the words of `ssh` that a refusal reads | `MAX_REFUSAL_BYTES` | 1024, then dropped |
| The machines of one watch | `MAX_WATCHED_MACHINES` | 8 |

A report that breaks one bound or one rule of the format is dropped whole with a
typed reason, and the watch goes on with the report before it. A line that never
ends is read to its bound and dropped.

## The state of one agent

The state follows the rules of a local panel, so one agent looks the same on a
machine and on this computer.

| Signal | Word |
|---|---|
| The title rule of the kind says that the program works, or a row of the screen holds a working line | Working |
| The title rule of the kind says that an approval waits | Needs you |
| The turn ended after the last look of the person | Done |
| Idle, and the person saw it | No word |

The title rule of the kind answers first, and the working line of the last rows
of the screen answers after it. Only the Codex kind has a title rule in this
build, so only a Codex pane reports **Needs you** in this round.

The identity of a row is its session and its pane id, which `tmux` keeps for the
life of the pane. A pane that a report does not name drops its row.

The first sight of one pane claims nothing while it is idle, and it is
**Working** when the pane already works, because the watch may start in the
middle of a turn of another person. A local launch differs: Horizon starts it and
knows that its first frames belong to its load.

`MachineWatch::mark_seen` is the look of the person. The sidebar of the next round
calls it when the person opens the agent or passes its row.

## The reconnect rule

The watch does not invent a second backoff. The end of the `ssh` child is one
`ConnectionLoss` with the status of the child and the time that the link was up,
and `place::after_connection_loss` decides: 3, 6, 12 and 24 seconds, then every 30
seconds, and a link that was up for `RECONNECT_RESET_AFTER` starts the delays
again. The watch reports the loss as `Lost` with the moment of the loss and the
number of the next attempt, or as `Refused` with the reason that the words of
`ssh` name: a host key that `ssh` does not know or accept, a key that the machine
refuses, or a record that no longer passes the rule of an SSH terminal.

## The API that the window reads

| Call | Work |
|---|---|
| `MachineWatch::snapshot` | One `Arc` of the snapshot. Cheap, and no input or output. |
| `MachineWatch::mark_seen` | One row of one agent is seen. Cheap, and no input or output. |
| `MachineWatch::reconcile` | Starts and stops readers. It joins threads, so a worker calls it. |
| `MachineWatch::stop_all` | Stops every reader. A worker calls it. |
| `MachineWatch::set_programs` | The programs of the presets of the settings. |
| `MachineAgent::open_command` | The remote command of a terminal panel of that agent. |
| `MachineAgent::can_open` | Whether the session name of the agent passes the rule of this build. |

The snapshot holds the machines in name order, the agents of each machine in
session and pane order, and the totals `needs_you` and `done`, which the attention
summary adds to its own.

## What the next round builds

- A sidebar row for each machine with the state of its link and its agents.
- A click on an agent: `open_command` and a terminal panel with the connection of
  that machine, and `mark_seen` after it opens.
- The notice of a row that changed to **Needs you** or **Done** while no window
  of Horizon has the focus.
- The call of `reconcile` when the machine list or the setting changes, on a
  worker thread, and a frame that reads the snapshot version.
