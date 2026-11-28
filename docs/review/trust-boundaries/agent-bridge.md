---
applyTo: "crates/horizon-core/src/agent_bridge.rs,crates/horizon-core/src/agent_bridge/**,crates/horizon-ui/src/app/orchestrator_dock.rs,crates/horizon-ui/src/app/orchestrator_dock/**,crates/horizon-ui/src/app/sidebar/rent_machine.rs,scripts/device-smoke/orchestrator_standin.py,scripts/device-smoke/serve.py"
---

# Trust boundary of the agent bridge

Horizon runs one agent program with no terminal, sends the messages of the person
to it as JSON lines, and reads its JSON lines as typed events. The program is
Claude Code, and its caller is the orchestrator dock of the window
(`crates/horizon-ui/src/app/orchestrator_dock/`). The dock runs one program for
each conversation: one for each workspace, and one for the front desk, which
belongs to no workspace.

The program is another process of this computer, and its output is a text that no
part of Horizon controls. Every byte of its standard output and of its standard
error is untrusted. The bridge therefore reads one line format with the bounds
below, drops a line that breaks one, and keeps no text of the program outside the
events and the snapshot.

The module comment of
[`crates/horizon-core/src/agent_bridge.rs`](../../../crates/horizon-core/src/agent_bridge.rs)
and the design page
[`docs/architecture/agent-bridge.md`](../../architecture/agent-bridge.md) state
the same rules.

## Untrusted: every byte of the program

- **One grammar, and one version of it.** Each line must be one JSON object with
  a `type` string. The codec maps the types and subtypes that it knows and gives
  nothing for every other type, so a later version of the program never costs the
  conversation an event.
- **A refused line is dropped whole.** A line that is not JSON, a JSON line
  without a `type` string, a `system/init` line whose session id is not a UUID,
  and a line above `MAX_LINE_BYTES` become `BridgeEvent::Dropped` with a typed
  reason. The event holds the reason and never the line. A refused line ends
  nothing, with one exception: an over-long `result` line also ends its turn,
  because the turn would otherwise stay busy for ever. The duration and the cost
  of that turn stay unknown.
- **A long line is read to its newline and held nowhere.** The reader keeps at
  most `MAX_LINE_BYTES` of one line. It then reads the head of that line in a
  stream, for the top level `type`, `subtype` and `is_error` only: every other
  value is skipped without a value, one text of the head holds at most
  `MAX_SHAPE_CHARS` characters, and at most `MAX_SHAPE_BYTES` of one line is read
  at all. The rest of the line reaches its newline and the reader keeps no byte
  of it.
- **Every text has a bound.** One text of the agent is cut at `MAX_TEXT_BYTES`
  bytes, one summary, one refusal and one notice at `MAX_SUMMARY_CHARS`
  characters, and one id of the program at `MAX_ID_CHARS` characters. The codec
  joins at most 256 message ids and at most `MAX_TEXT_BYTES` of one message.
- **No text of the program reaches a log line, a path or a file, and one named
  value reaches a command line.** The bridge logs a start, an end and a dropped
  line with the reason. It writes no file of its own: the stored conversation is
  the file of `store`, which the section below states. A tool summary is built
  from one fixed key of the tool input (`command`, `file_path`, or the tool name
  alone), and the whole input never becomes an event. The one text of the program
  that reaches a command line is the session id, and the bridge accepts it only in
  the form of a UUID: no byte of it is read as another option of the program or as
  a word of a shell.
- **A tool name is a claim.** The name, the summary and the result of a tool come
  from the program. The dock draws them as plain text, and no value of them
  becomes a command, a path or a setting.

## What the bridge guarantees

- **One process, by its own id.** The bridge starts the program of a
  `ProgramSpec` with no shell around it. It records the id and the start time of
  that process once, at its start, and its stop and its drop end exactly that
  process, so no signal reaches a later process that the system gave the same id.
  A signal waits up to one second for a start time that a system fills after the
  start, and the bridge writes a log line when it cannot name the process.
  `Bridge::stop` waits at most `STOP_TIMEOUT`. A drop sends the signals on a
  thread of its own and waits for nothing.
- **The prompt is one line of the standard input.** The caller gives the text to
  `Bridge::send`, which builds the line with `serde_json`. The prompt never
  appears on the command line, in the process list or in a log line.
- **One message comes from Horizon and not from a person.**
  `HorizonApp::say_to_shown_orchestrator` sends the one sentence of the sidebar
  button `+ Rent a machine`, which is a constant of
  `app::sidebar::rent_machine`. That sentence is the words of Horizon: it holds
  no text of a program, of a page or of a file, so the button adds no untrusted
  input to the bridge. The message goes to the conversation that the dock shows,
  which is the conversation of a workspace: the button is disabled, and sends
  nothing, while the dock shows the front desk. It starts the program of that
  conversation when the conversation has none, on the worker of the dock, as the
  composer does.
- **A name of a nested agent is removed.** Every inherited variable with the
  prefix `CLAUDE_CODE_` or `HORIZON_WORK_`, the variable `CLAUDECODE`, and the
  four names of an agent identity (`HORIZON`, `HORIZON_BROWSER_ACTOR`,
  `HORIZON_BROWSER_HOST_INSTANCE` and `HORIZON_EXECUTABLE`) are removed from the
  environment of the child before it starts, so a bridge inside an agent panel
  does not speak as that panel. A start that has an identity of its own sets its
  four values after that removal.
- **No line of the program waits on a frame.** The reader, the writer and the
  reader of the standard error are threads of their own. `poll` reads a channel
  with `try_iter`, and `send` queues a line. A caller runs `start` and `stop` off
  the frame, on the worker thread `orchestrator-dock` of the dock
  (`app/orchestrator_dock/worker.rs`), which is separate from the coordination
  worker of the browser jobs. The [UI thread
  guide](../../architecture/ui-thread.md) requires that rule.

## What the program of a workspace gets

The dock starts one program for the orchestrator of a workspace with
`claude_for_workspace` (`crates/horizon-core/src/agent_bridge/program.rs`).

- **The identity is Horizon's own value.** The actor is built from the local id of
  a workspace of this board (`crates/horizon-core/src/agent_bridge/actor.rs`), and
  the host instance, the home and the executable are the values of this Horizon.
  No text of the program and no text of a card takes part.
- **The environment is the environment of an agent panel.** `HORIZON=1`,
  `HORIZON_BROWSER_ACTOR`, `HORIZON_BROWSER_HOST_INSTANCE` and
  `HORIZON_EXECUTABLE` come from the one function that an agent panel uses
  (`crates/horizon-core/src/panel/spawn/environment.rs`,
  `agent_identity_vars`). `HORIZON_HOME` is inherited as a panel inherits it and
  is not written by the identity.
- **The plugin directory is a path of this computer.** The start adds
  `--plugin-dir` with the directory of Claude Code of this host instance, and only
  while it is a directory. The MCP server of that plugin reads the identity from
  the environment of the child, not from the command line.
- **No work variable, no named secret and no work-resume plugin.** The start adds
  none of them, so the program of the orchestrator holds no key of the person and
  writes no ledger of a turn.
- **The child is the same untrusted program.** Every rule of this page holds for
  it: one grammar, bounded texts, a refused line dropped whole, and no text of it
  in a log line, a path, a command line or a file.

## What the program of the front desk gets

The front desk is the conversation of the dock that belongs to no workspace. The
dock starts its program with `claude_for_front_desk`
(`crates/horizon-core/src/agent_bridge/program.rs`), and
[`docs/architecture/front-desk.md`](../../architecture/front-desk.md) states the
whole rule.

- **The actor is one constant, and it owns no workspace.** The actor is
  `horizon:front-desk` (`actor::FRONT_DESK`). It holds no local id, so no
  workspace of this board can match it. The host refuses every card request of
  that actor with `caller_unavailable`, so the program creates no card, reads no
  card and opens no panel (`app/block_requests/caller.rs`).
- **The model is a name and never an option.** The variable
  `HORIZON_FRONT_DESK_MODEL` reaches the command line as one value of `--model`,
  and only while it is a name of at most 64 bytes of the letters, digits, dot,
  hyphen and underscore that does not start with a hyphen. Every other value
  gives `haiku`. No byte of the value is read as another option of the program or
  as a word of a shell.
- **The system text is one constant of Horizon.** `FRONT_DESK_PROMPT` is a text
  of this repository. It holds no text of a program, of a page, of a file or of
  the person. The start passes it as one argument of `--append-system-prompt`.
- **The program has no built-in tool.** The start passes `--tools ""`, so the
  conversation answers with words and does no work itself. The next part of this
  work adds the tool that makes work items.
- **The child is the same untrusted program.** Every rule of this page holds for
  it: one grammar, bounded texts, a refused line dropped whole, and no text of it
  in a log line, a path, a command line or a file.
- **The conversation and its record keep the same rules.** Its file is
  `sessions/<session id>/conversations/front-desk.jsonl`, the folder is `0700`
  and the file is `0600`, and its record is `RuntimeState.front_desk`. The
  session id of that record is text of the program, so the next start resumes it
  only in the form of a UUID. A state file of an older build holds no such field
  and loads with a new session.

## What the dock draws

The dock is another reader of the untrusted text of the program, so the same
rules hold for it.

- **The text of the program is text.** A text of the agent is one label. The
  dock draws the marks of Markdown for bold, italic, code, a heading and a bullet
  as a weight, a slant, a font and a dot (`orchestrator_dock/markdown.rs`). A
  mark changes only how a span is drawn: no link, no image and no path in the
  text becomes active, and no span takes a press. A mark with no partner stays
  text. The person selects and copies the text.
- **One frame lays out a bounded number of entries.** The list lays out at most
  the last 200 entries of the conversation, and one notice above them names the
  entries that it does not draw. The conversation itself keeps
  `MAX_ENTRIES` entries, and each text of it is cut before it enters the
  snapshot.
- **No entry becomes a control.** An entry draws no button, no card and no
  click target. No text of an entry becomes a path, a command line, a log line or
  a file.
- **The words of Horizon are its own, with two named exceptions.** A notice that
  the dock adds, the state of the header and the cost are built by Horizon from
  typed values. Two notices carry text of a line after words of Horizon: the
  refusal of a tool (`<tool name> was denied: <message>`) and the end of the
  program (`the program ended with code N: <stderr tail>`). That text is cleaned
  and cut to `MAX_SUMMARY_CHARS` characters as every other summary is. The dock
  draws it as plain text, and no value of it becomes a command, a path or a
  setting.

## The stored conversation is untrusted when it is read back

The dock keeps the entries of one conversation in
`sessions/<session id>/conversations/<key>.jsonl`, one JSON object per line, where
the key is the local id of a workspace or the reserved key `front-desk`, so a
restart shows the conversation again
([the architecture page](../../architecture/agent-bridge.md#storage)). That file
is a file of the person's home: the process that wrote it can stop in the middle
of a line, a damaged block reaches it, and the person can edit it. A later start
therefore reads it as untrusted text.

- **One grammar, and one version of it.** A line must be one JSON object of
  `{"version":1,"entry":{…}}`, and the entry must be one of the four kinds of
  `Entry`. A line that does not parse, that names another version, or that is
  above `MAX_LINE_BYTES` bytes is dropped whole and counted. One bad line never
  costs the file, and the load never fails because of one.
- **The bounds of the read.** A load holds at most `MAX_ENTRIES` entries and one
  line of `MAX_LINE_BYTES` bytes of the file at a time. A file above twice
  `MAX_ENTRIES` entries is rewritten with its newest entries.
- **Every text of a stored entry keeps the bound of an event.** A stored text
  enters the conversation through `Conversation::restored`, which keeps the same
  `MAX_ENTRIES` list and the same cut of the texts, and the dock draws every
  stored text as plain text with the rules below.
- **A stored tool call is a claim, not a fact.** A call that the file records as
  running reads as failed with the result `interrupted`, and no entry of the file
  becomes a command, a path or a control.
- **The folder and the file are private.** The folder is `0700` and each file is
  `0600`, as the folder of a session is, and the name of a file is built from the
  local id of a workspace: no character of the id that a path can interpret
  reaches the name.
- **The record of the session id is text of the program.** `WorkspaceState
  .orchestrator` holds the session id that the program named and the name of its
  file, and `RuntimeState.front_desk` holds the same record for the front desk. The dock keeps the value as the conversation cut it, one line of
  `MAX_SUMMARY_CHARS` characters. The next start resumes it only when it is a
  UUID, and any other value starts a new session. It never becomes a word of a
  shell, a path or a log line, and a record whose program name differs from the
  program of this dock resumes nothing.
- **A removed workspace removes its file.** The dock removes the file of a
  workspace that closes, on its worker, and the folder of a session goes whole
  when the person deletes that saved session.

## The front desk passes text of one program to another

The conversation of the front desk holds the proposals that its program writes,
and its program reads the state of the work items of this Horizon. That state is
text of other programs, so one program can influence another through the front
desk.

- **The proposal is one entry of the conversation.** `Entry::Proposal` holds the
  id that Horizon made, the items and the state `open`, `started`, `declined` or
  `replaced`. The title and the task are cut when the entry enters the
  conversation, and the dock draws them as plain text.
- **Nothing starts at the entry.** A workspace starts only at a press of the
  person on the control of the row, on the frame that reads the answer of a check
  that ran on the worker of the dock.
- **The read is a read of the frame.** `work_items` answers the state that the
  frame already holds: the names of the workspaces, the state word of their rows,
  the text of the first question of their cards and the last text of their
  orchestrators. It reads no stored file and starts no process.
- **A passed message says who passed it.** `Entry::Person` holds the field
  `via_front_desk`, and the dock draws the caption `via the front desk` above
  that bubble. The field has a serde default, so a file of an earlier run loads
  with `false`.
- **The bound of the message and the interval.** A message is 1 byte to the bound
  of one message of the person, and one work item reads at most one message of the
  front desk in ten seconds. A conversation that runs a turn answers `busy`, and
  it reads no message of the front desk while it answers.
- **The notices of the work items are the words of Horizon.** Horizon writes
  `<name> needs you`, `<name> finished` and `<name> failed` for each change of
  state of a work item that the front desk started, with the name cut to 40
  characters. No part of that line comes from the program.

What this does not guarantee: that the front desk repeats the person faithfully,
and that a work item acts on a message that the front desk passed. The caption is
the only mark of the path that the message took, and a program that reads its
conversation can read it.

## What the bridge does not guarantee

- **What the program did.** The bridge cannot prove that a tool ran, that a text
  is true, or that a cost is right. An event is the claim of the program.
- **That the child stays the program that it started.** The program runs as the
  person, with the rights of the person. The bridge proves the process id and its
  start time, and nothing about what the program does with its own files, its own
  network or its own storage.
- **That a message reaches the model.** A killed turn leaves nothing resumable in
  Claude Code, and a line that the program did not read is lost with its process.
- **That every line becomes an event.** A type that this slice does not use gives
  nothing, and a result of a tool whose start the conversation did not see
  changes nothing.
- **What a later slice does with a snapshot.** The snapshot is memory of this
  process. A later change that saves it, draws it or sends it to another program
  must keep the same bounds.
