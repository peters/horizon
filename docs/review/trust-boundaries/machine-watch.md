---
applyTo: "crates/horizon-core/src/machine_watch.rs,crates/horizon-core/src/machine_watch/**,crates/horizon-core/src/config/machines.rs"
---

# Trust boundary of the machine watch

Horizon shows, under each machine of its machine list, the agents that run on
that machine in `tmux`, with no panel open. One `ssh` process of this computer
per machine asks the machine for a report, and the report names its agent panes
and their states.

A machine is a computer that Horizon rented or reached. It can be taken over, and
every byte that its report returns is untrusted. The watch therefore reads a text
of one fixed format, drops a report that breaks a bound, and stores no text of a
machine screen.

The module comment of
[`crates/horizon-core/src/machine_watch.rs`](../../../crates/horizon-core/src/machine_watch.rs)
and the design page [`docs/architecture/machine-watch.md`](../../architecture/machine-watch.md)
state the same rules. This page starts at the bytes that the `ssh` child of the
watch returns. The record of a machine, its route and its key are the boundary of
the machine list and of a terminal of that machine, which
[`crates/horizon-core/src/machines.rs`](../../../crates/horizon-core/src/machines.rs)
and [`crates/horizon-core/src/place.rs`](../../../crates/horizon-core/src/place.rs)
state.

## What Horizon starts, and what it does not

- **One `ssh` process per machine.** The watch starts `ssh` with the options of a
  terminal of that machine: the route of the record, `BatchMode=yes`, the connect
  bound and the keepalives of a machine terminal, and, when the record binds a
  machine key, that key file, `IdentitiesOnly`, `IdentityAgent=none` and the
  known-hosts file of that key. It allocates no terminal (`-t`), so the machine
  runs the command on a pipe.
- **The remote command is one word.** It is `sh`, and no value of a record takes
  part in it. `sshd` runs a remote command with the login shell of the user of the
  machine, so a text with spaces would be a text that a login shell which is not
  POSIX cannot run. The reader writes the text of the watch to the standard input
  of the `ssh` child and closes that pipe at once, so `sh` reads it there.
- **The remote text is one constant of this build.** It is POSIX `sh` and it uses
  `tmux`, `ps`, `cksum` and the built-ins of `sh`. It reads the panes of the
  `tmux` sessions of that user, and the pane option `@horizon_state` of each pane.
  It runs in a loop, and it prints a report only when the report changed; a still
  machine prints one short heartbeat line instead. The text is no argument of a
  process, so no command line of this computer and no process list of the machine
  holds it.- **No value of a record reaches that text, and no value of a report reaches a
  command line of this computer.** The route and the key are arguments of `ssh`,
  and they pass the rule of an SSH terminal first. The text reads no name, no
  path, no session and no key of the record.
- **Nothing is installed on the machine and nothing there reaches this
  computer.** The machine sends no answer to a port of this computer, starts no
  program of this computer and reads no file of this computer. The link leaves
  this computer only because the watch started it.
- **This computer watches at most [`MAX_WATCHED_MACHINES`] machines at one
  time.** A later machine of the list is not watched, and its state says so.
- **The setting `machines.watch_agents` turns the whole watch off.** It is on by
  default. With the setting off, or with no machine in the list, the watch starts
  no thread and no process.

## Untrusted: every line that the machine returns

- **The format is one version.** A report starts with a header that names the
  version of the format, holds one block for each pane, and ends with a terminator
  line. A report with another version is dropped whole with a typed reason, and
  the watch goes on.
- **A line that breaks a rule drops the whole report.** An unknown line kind, a
  pane id that is not `%` and digits, a field count of the wrong length, a line
  that is not UTF-8, a line above [`MAX_LINE_BYTES`], a report above
  [`MAX_REPORT_BYTES`] or above [`MAX_REPORT_LINES`] lines, and more than
  [`MAX_PANES`] panes, [`MAX_ROWS`] rows of one pane or [`MAX_COMMAND_LINES`]
  command lines of one pane are these rules. The watch keeps the report before,
  reports the machine as watching, and reads the next report.
- **A line that never ends is read to its bound and dropped.** The reader of the
  child takes at most [`MAX_LINE_BYTES`] bytes of one line, reads the rest of it
  in fixed chunks and holds no byte of it, so a machine cannot make this computer
  grow a buffer.
- **Every field is one line of words.** The remote text replaces each run of
  spaces, tabs and newlines of a field by one space, so no tab and no newline of a
  session name, of a folder name, of a title or of a program name can break a
  line. A value that this text does not flatten, because a program of the machine
  wrote it, is still one line of the report or the report is dropped.
- **The words of `ssh` are read to one bound and then dropped.** A refusal needs
  the words of the `ssh` child of this computer, so the reader keeps at most
  [`MAX_REFUSAL_BYTES`] bytes of its standard error, names one of the typed
  reasons, and drops the words. No line of the log quotes them.
- **No text of a screen is stored.** The bottom rows of a pane are read for one
  state and then dropped. A name is cut at [`MAX_NAME_CHARS`] characters. No
  title, no row, no folder and no command line of a machine reaches a file, a log
  line, a snapshot field or a command line of this computer.
- **The state is a claim of the machine.** A row says that its machine claims an
  agent pane with a state. It is not proof that the agent runs, that its state is
  true, or that the pane belongs to the person.
- **The state of a pane is one of four words or no report.** The text reads the
  `tmux` pane option `@horizon_state` and sends it as one field of the pane line.
  The parser accepts `working`, `blocked`, `done` and `idle`, with the exact case,
  and it reads every other word, every empty field and a field above its bound as
  no report. A report of a pane is therefore one of the four states of this build
  or nothing, and no text of a program of a machine becomes a name, a word or a
  state of this window. The option itself is a value that any program of that
  user, and the machine, can set: it proves nothing.
- **A pane that reports a state and runs no agent program is a row.** Its kind is
  the plain terminal mark of this build, its name is the session name of its pane,
  cut at [`MAX_NAME_CHARS`] characters, and its state is the report. A press on
  that row asks the record for the route and opens the session, as a press on the
  row of an agent does. A pane with no report and no agent program stays out, so
  the machine chooses that a row appears and when it leaves, and it chooses no
  other value of that row.

## What a taken-over machine can do, and what it cannot do

A machine that an attacker took over controls every byte of its report. It can:

- show false rows of its own machine: a false session, a false pane, a false
  agent kind, a false name and a false state;
- show more than the panes that exist, up to the bounds above;
- stop its report at any time, which reads as a lost link;
- send much data, which costs this computer one dropped report per bound and one
  line of the log.

It cannot:

- run a program, a shell or a command on this computer;
- name another machine: every row of a report belongs to the machine of the link
  that returned it;
- write a file of this computer, or make this computer write one;
- read a key, a credential, a secret, a setting or a path of this computer;
- reach the machine list, a workspace or a panel of another machine;
- make a row of its report the row of a local panel or of an agent of another
  machine.

## What the window does with a row

- **A name and a state are plain text.** The sidebar draws the name of an agent,
  the name of its machine and its one state word as text, cut with an ellipsis at
  the width of the row. A name that is cut shows the whole name in its hover. No
  text of a report becomes the label of a file, a command line or a settings
  value.
- **A click passes the checked session name and nothing else.** The row of an
  agent carries no command from the machine. A press asks the record for the
  route again, and the command is the constant text of
  `machines::session_command` with the session name as its one argument, which
  `block_machines::check_session_name` accepts first. A session name outside that
  rule has no press: its row says so in its hover and opens nothing.
- **The terminal is a machine terminal of this build.** The connection holds the
  options of a terminal of that machine, the key of the record when it binds one,
  and no command of the report. The panel's title is the name of the agent, which
  is a value of the machine: it is cleaned and cut as the title of a panel, and it
  reaches no file and no command line.
- **The notice of a change is the notice of a panel.** An agent of a machine that
  changes to **Needs you** or to **Done** while no window of Horizon has the focus
  sends one desktop notification through the path of a local panel, with the name
  of the machine beside the name of the agent, and the bounds of that path:
  [`desktop-notifications.md`](desktop-notifications.md) states them.

## What the watch does not guarantee

- **That a state is true.** A state is the claim of a machine, as a hook event is
  the claim of an agent process. Horizon can tell one machine from another by the
  link, and it cannot tell a true report from a false one.
- **That a pane is the agent that it names.** The kind of an agent comes from the
  program name of the pane's foreground process, or from the command line of the
  processes of the pane's terminal when an interpreter runs there. A program that
  another person renames is not recognized, and a machine can name any program.
- **That the watch sees every agent.** A pane whose program runs through an
  interpreter is recognized only when the `ps` of that machine names the
  processes of a terminal. An agent that runs outside `tmux` of that user is
  invisible.
- **That the state is fresh.** The machine reports every
  [`WATCH_INTERVAL_SECS`] seconds, so a state can lag by that interval and by one
  read of the report. A link that drops keeps the last rows until the reconnect
  answers.
- **That a taken-over machine cannot watch this computer.** The watch adds no
  port, no socket and no file that a machine reaches. A machine reads only the
  bytes that its own `ssh` child asks for.
- **Where the report goes after the snapshot.** The snapshot is memory of this
  process. A later change that saves it, draws its text or sends it to an agent
  must keep the same bounds.
