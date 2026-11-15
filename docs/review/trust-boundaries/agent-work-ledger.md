---
applyTo: "crates/horizon-core/src/panel.rs,crates/horizon-core/src/panel/agent_status.rs,crates/horizon-core/src/agent_work/**,crates/horizon-pty-tap/**,crates/horizon-core/src/terminal/lifecycle.rs,crates/horizon-core/src/terminal/events.rs,crates/horizon-core/src/attention.rs,crates/horizon-core/src/attention/**,crates/horizon-core/src/workspace_list.rs,crates/horizon-ui/src/agent_work_hook.rs,crates/horizon-ui/src/plugin_install/work_hooks.rs,crates/horizon-ui/src/plugin_install/work_extension.rs,assets/plugins/pi/**,crates/horizon-ui/src/app/attention.rs,crates/horizon-ui/src/app/attention/**,crates/horizon-ui/src/app/sidebar/list.rs,crates/horizon-ui/src/app/sidebar/rows.rs,crates/horizon-ui/src/app/sidebar/panel_rows.rs"
---

# Trust boundary of the agent work ledger and of the attention state

Horizon gives an agent panel a way to report its state, on Linux and on macOS,
with the default command of the preset or with a command of its own. Three
programs use it, and the third needs nothing of Horizon:

- A **Claude Code** launch receives the plugin folder of the work hooks with
  `--plugin-dir`. Each hook event reaches Horizon as one line of JSON on the
  standard input of `<candidate> --agent-work-hook`.
- A **pi** launch receives one JavaScript module with `-e`, from the Horizon
  home. The module sends the lifecycle events of its process to the same entry,
  with one call for each event. It sends no text of the person and no text of the
  model, it waits for no answer, and it does nothing at all when the five
  `HORIZON_WORK_*` variables of the launch are absent.
- A **Codex** launch receives no argument and no file for its state. Codex
  reports its state with the Program Status Protocol (OSC 7501), which the reader
  of the panel hands over (`crates/horizon-pty-tap`), and it also writes the state
  into the title of its terminal, which the VT parser already hands on. Horizon
  matches the payload of a panel of that kind against one table
  (`crates/horizon-core/src/agent_work/program_status.rs`), matches the title
  against another (`crates/horizon-core/src/agent_work/title.rs`), and both
  matched states enter the same turn ledger that a hook event writes.

Each event or title claim writes the state of one **work ledger** per panel. A
hook event writes the record of the panel in the Horizon home, and a title claim
stays in memory, because a title names no turn that survives a restart. The
sidebar reads that state to say which agent panel needs the person, which one
finished while the person looked elsewhere, and which one works.

The module of a pi launch is a file that Horizon owns and writes
(`crates/horizon-ui/src/plugin_install/work_extension.rs`), and a launch adds it
to the command line only while it is a file: pi does not start at a path that
names no file. A launch whose report is missing therefore keeps its command line
and the rule of its screen.

The **report** of a panel is separate from the **continuation** of its work. The
report covers every Claude launch and every pi launch on Linux and macOS. The
continuation covers Linux, a Claude panel, the default command of the preset, and
the per-panel policy `Continue work after restart`; only that path offers a
resume, writes a handoff and seeds a prompt.

The module comment of
[`agent_work/lifecycle.rs`](../../../crates/horizon-core/src/agent_work/lifecycle.rs),
of [`agent_work/ledger.rs`](../../../crates/horizon-core/src/agent_work/ledger.rs)
and of [`store.rs`](../../../crates/horizon-core/src/agent_work/store.rs) state
the same rules for the record. [`attention.rs`](../../../crates/horizon-core/src/attention.rs)
holds the only code that turns a record into a state.

## Untrusted: every lifecycle event of an agent process

The event comes from the agent process, which runs as the same local user as
Horizon. The person runs that agent, and its output is not Horizon's text.

- **The launch identity gates the event.** The environment names the Horizon
  root, the panel and the launch token. The store accepts an event only for a
  panel that `register_owner` registered, only while that launch's health marker
  holds `1`, and only while the record names the same owner token and panel kind.
  An event of another launch is refused, and a refused hook ends the launch's
  evidence instead of writing it.
- **The session must match.** An event carries a session identity. A ledger
  accepts it only when the ledger holds that session or no session yet, so a
  delayed event of an older session cannot reopen or close a later turn. A
  Claude launch names its conversation to Horizon. A pi launch names none, so the
  module of the launch names its own process, and that identity stays the same
  while the process runs.
- **The turn closes only its own turn.** An event that ends a turn names the turn
  that opened it. A repeated end or a late end of an older turn cannot close a
  turn that a later event opened, and it cannot open one either.
- **An event of another panel changes nothing.** The panel and the launch token
  come from the environment that Horizon set at the launch, not from the event.
  One panel therefore cannot write the state of another panel.
- **The first event must arrive.** A record of a launch that has heard no event
  changes no state: the panel keeps the rule of its terminal screen. An agent
  that runs no hooks, and a preset command that is a wrapper and drops Horizon's
  arguments, therefore keep the signal of the screen instead of a silent ledger.
  The titles of a program follow the same rule: they answer only from the first
  working or waiting title of a launch, and a launch whose titles never take one
  of the forms keeps the screen rule.
- **A later launch is not the earlier one.** A read of a launch whose generation
  is older than the one the sidebar holds changes nothing, so a delayed read of a
  panel process that is gone cannot decide the panel that replaced it.
- **A clock that goes back is not a turn.** An event whose time is before the
  record leaves the ledger `unknown`, and an unknown turn is idle.
- **The event is bounded and checked.** The input is at most 1 MiB, the session
  identity at most 512 bytes, and the working directory and the transcript path
  must be absolute. A record is at most 256 KiB, and a larger or a corrupt one
  raises an error instead of a state.
- **An older read is dropped.** A read of a record that is older than the one
  the state already holds changes nothing, so two reads that arrive out of order
  cannot move the state backwards.
- **A relaunch starts its own timeline.** A record of another session or of
  another launch of the panel is read from its own start. It shows the screen
  rule again until its first event, and it does not show the turn that ended
  before it as unread.

## Horizon guarantees: metadata only, and one panel at a time

- **No prompt and no tool content is stored.** A record holds the session, the
  turn, the event name, the tool identity, the working directory, the transcript
  path and, for a stop, the count of the background work. A prompt, an answer,
  the arguments of a tool and the text of the transcript are not part of it. The
  attention state reads the record and nothing else. The wider set of panels
  stores no field that a continuing panel does not store.
- **The module of a pi launch sends no text.** The module translates the events
  of pi and sends the launch identity, a turn identity, an event name and, for a
  dialog, its identity. It reads the environment of its own process and it
  writes no file of pi. A test runs the module that the build embeds and refuses
  any call that carries a text of the person or of the model.
- **A report waits for nothing.** The module starts the entry of Horizon as a
  detached process that it never joins, and it swallows every error of that
  start. A missing entry, a missing variable and a refused event therefore
  neither slow, print nor stop the agent.
- **The owner is the launch, while the panel lives.** No process table is read to
  decide the report on either system. The token that Horizon registered at the
  launch is the owner: a new launch of the panel replaces it, and a closed panel
  removes its record from the home. The liveness probe of the session registry
  stays with the continuation of work, and it remains Linux.
- **The state is not a claim about the work.** `Needs you` says that an event
  reported an open question, a failed turn or a dialog of an extension that waits
  for the person. It does not say that the question is still open in the agent,
  and it is not a permission. A permission that the person grants in the agent
  panel is not granted to any other panel, and pi asks no permission at all.
- **The screen is the rule until the first event, and the only rule after it.**
  An agent panel whose launch registered no ledger, or whose ledger has heard no
  event, takes its state from the working indicator of its own terminal. Two
  rules never decide one panel at the same time.
- **A report is not a continuation.** A panel without the policy shows no
  `Review work before continuing` card, starts no resume, writes no handoff and
  receives no prompt from Horizon. What an agent process can make the sidebar say
  is one of four states of its own row. It cannot make Horizon continue work, and
  it cannot reach the row of another panel.
- **The unread mark is memory.** Whether the person has looked at a finished
  turn lives in the process and is not saved. A restart shows the state of the
  ledger, and every panel starts as looked at.
- **The files stay private.** The work directory is created `0700`, and a record
  and a health marker are written `0600` through a temporary file and a rename.
- **No frame waits.** A frame reads a value that the attention worker published.
  The worker reads the records of the home, and it wakes the window only when its
  answer differs from the one before it. A record leaves the home on a worker
  thread when its panel closes.

## Untrusted: every byte of a panel's output

A program in a terminal panel controls every byte of its own output, and the
report of the program status protocol is one of those bytes. Horizon reads it in
the reader of the panel's PTY, because the VT parser of this workspace drops a
private OSC number.

- **The scanner is bounded.** It keeps at most 256 bytes of one payload, and a
  longer report is read to its end and dropped whole, as the protocol allows a
  terminal to choose a lower limit than its 4096 bytes. It allocates nothing, it
  cannot block, and it cannot panic on any byte of a stream, including a sequence
  that a read splits at any byte and bytes that are not UTF-8.
- **The screen is unchanged.** The reader hands every byte to the VT parser
  before the scanner looks at it, so the grid of the panel shows what the program
  wrote, with or without a report.
- **The reader never waits for a frame.** A complete payload reaches a queue of
  eight reports that drops its oldest entry, and a frame reads the newest one. A
  program that writes as fast as it can therefore slows neither the event loop nor
  the frame, and a report that a newer one replaced before a frame ran is not
  replayed.
- **A report answers for one panel.** The bytes come from the program of that
  panel's own PTY, so one panel cannot report the state of another. There is no
  file and no identity to forge, and the claim never leaves the process.
- **`app` is a claim, and it only selects the table.** A panel of the Codex kind
  reads a report only while its `app` value is `codex`. A report of another
  program, a report with no `app`, a report of a record beneath the root, a state
  that the protocol does not name and a report that is not UTF-8 claim nothing.
- **No text of a report is stored.** The parser keeps the state and drops the
  keys: the `kind` of a wait, the `progress`, the `msg`, the `title` and the `id`
  of a record never reach a field, a view or a file.
- **A report opens and closes only its own turn.** A report names no turn, so the
  claim numbers one from a counter of the launch. A repeated report of the state
  that stands does no work, a `done`, an `error` or an `idle` ends only the turn
  that the claim holds open, and a `clear` forgets the launch. A launch that
  Horizon started again is a launch of its own.
- **A launch claims nothing while it starts.** A program that is not ready for the
  person has run no turn of theirs. The first `idle` report of a launch is the
  moment the program is ready, and a `working` report before it claims no turn. A
  `blocked` report before it claims **Needs you**, because the person must answer
  the question of the program, and the ready report forgets the turn that the
  question opened: no turn of the person ran in it. A program that never reports
  `idle` keeps the rule of the screen.
- **One source answers, in one order.** A record of a hook answers first, then
  the reports of the protocol, then the title, then the screen. A launch that has
  given one recognized report no longer reads its title, so a person who changes
  the title of a program cannot overrule a report.
- **The answer is constant.** The only bytes that Horizon writes for the protocol
  are the reply to its feature detection query, which is one constant sequence
  and holds nothing of any report. The protocol forbids any other write.

## Untrusted: the title of a panel, when its agent kind has a rule

The title is the program's own statement about its own panel. Codex writes it
with no configuration, and Horizon reads it without `unsafe` code and without a
second pass over the output of the terminal.

- **The rule is a table of agent kinds.** Only a panel of the Codex kind has a
  rule today. A shell panel in which a person starts `codex` by hand has no rule,
  so it reports nothing, and another program with a state title is one more row.
- **Two forms are a state, and every other title is plain.** A working title
  begins with one frame of the braille block and a space or the end. A waiting
  title begins with `[ . ] Action Required` or `[ ! ] Action Required`, then ends
  or continues with ` | `. Every other title, an empty title and a title beyond
  512 bytes claim nothing.
- **No text of the title is stored.** The words of the request and the name of
  the folder are the text of the program: no record holds them, and no view of
  this round shows them.
- **The turn belongs to the launch.** A title claim carries a turn number from a
  counter of its launch. A working title opens a turn when none is open, a
  waiting title blocks that turn, and a plain title ends only the turn that the
  claim holds open. A program that Horizon starts again in the same panel is a
  launch of its own: the claim of the launch before it answers nothing.
- **A launch claims nothing while it starts.** The first plain title of a launch
  is the moment the program is ready for the person, and a frame of the spinner
  before it claims no turn. A waiting title before it claims **Needs you**, because
  the person must answer the question of the program, and the ready title forgets
  the turn that the question opened: no turn of the person ran in it. A title that
  a later one replaces within one frame is not read, so a launch whose ready title
  stands for less than one frame keeps the rule of the screen.
- **A plain title must stand.** A program may write a plain title between two
  parts of one turn, so the end of a turn waits 400 ms of plain title. A working
  or waiting title in that window cancels the end, and the frame that asks for
  the end is the frame that already runs.
- **A frame of one state is no work.** A title of a working program changes about
  ten times a second. A state that the claim already holds changes no field, no
  record and no row, and it wakes no frame.
- **The claim is memory.** A title names no session, so a restart of Horizon
  starts every panel on the screen rule, and a title claim never reaches a file.

## Does not guarantee

- **A hook is evidence, not proof.** A process of the person that can write the
  hook input or the record files can make any state appear. The boundary keeps
  one panel out of another panel, and it keeps content out of the record; it does
  not authenticate the agent.
- **The state can lag.** The records are read at most every 500 ms, and an event
  that arrives after that read shows on the next one.
- **A failed turn and an open question look alike in the sidebar.** Both say
  `Needs you`. The record separates them, and the hover text does not.
- **The statement of an adapter.** The Claude plugin and the pi module are the
  two adapters that ship with Horizon. The stand-in of the fixture and an adapter
  that a person adds send their own events, and Horizon cannot tell how such an
  adapter counted its work.
- **Two sequences of a program are read, and no others.** Horizon reads the
  report of the program status protocol from the reader of the panel's PTY, and
  the title of the panel from the VT parser. Every other private sequence of a
  program is dropped. A hostile program in a panel can claim a state for its own
  panel through a report, a title or the events of its launch, and it cannot claim
  one for another panel.
- **A report that a newer one replaced is lost.** The protocol keeps one record
  for a panel, so the queue hands on the newest report of a frame and no backlog.
  A program that writes `error` and then `idle` before a frame runs shows the turn
  as interrupted rather than failed. Real programs pace their reports, and Codex
  has no error state.
- **A `clear` with no `app` claims nothing.** The protocol's own exit sequence is
  `state=clear` without `app`, and this panel reads a report only while it names
  the program of the kind. The end of the program is what clears the row, so the
  visible result is the same, and the record of a claim that speaks the protocol
  stays until the next launch.
- **The title claim is not exact.** It names no turn identity, no tool and no
  count of background work. A failed turn and an interrupted turn end like a
  finished one, so **Background** never comes from a Codex panel. A denied
  approval shows **Needs you** until the next title of that panel says otherwise.
- **A title through a multiplexer depends on that multiplexer.** `tmux` keeps the
  titles of its panes. A Codex panel behind such a `tmux` reports only when that
  `tmux` sets `set-titles on` and `set-titles-string '#{pane_title}'`, and it
  keeps the screen rule otherwise.
- **A program that a person starts by hand reports nothing.** A shell panel in
  which a person starts Claude Code, pi or another agent receives no argument and
  no variable of a launch, so its panel keeps the rule of its screen. Horizon
  cannot tell that the program of such a panel is an agent.
- **A panel with no event keeps the weaker signal.** A wrapper that drops the
  arguments of Horizon gives its panel the state of the screen, which is the same
  state as a panel without hooks. Horizon cannot tell that the wrapper dropped
  them.
