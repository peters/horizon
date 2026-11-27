---
applyTo: "crates/horizon-core/src/agent_bridge.rs,crates/horizon-core/src/agent_bridge/**,crates/horizon-ui/src/app/orchestrator_dock.rs,crates/horizon-ui/src/app/orchestrator_dock/**,scripts/device-smoke/orchestrator_standin.py"
---

# Trust boundary of the agent bridge

Horizon runs one agent program with no terminal, sends the messages of the person
to it as JSON lines, and reads its JSON lines as typed events. The program is
Claude Code, and its one caller is the orchestrator dock of the window
(`crates/horizon-ui/src/app/orchestrator_dock/`).

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
  without a `type` string, and a line above `MAX_LINE_BYTES` become
  `BridgeEvent::Dropped` with a typed reason. The event holds the reason and never
  the line. A refused line ends nothing: the turn, the conversation and the
  program go on.
- **A long line is read to its newline and held nowhere.** The reader takes at
  most `MAX_LINE_BYTES` of one line, reads the rest of it in chunks and keeps no
  byte of it.
- **Every text has a bound.** One text of the agent is cut at `MAX_TEXT_BYTES`
  bytes, and one summary, one refusal and one notice at `MAX_SUMMARY_CHARS`
  characters. The codec joins at most 256 message ids and at most `MAX_TEXT_BYTES`
  of one message.
- **No text of the program reaches a log line, a path, a command line or a file.**
  The bridge logs a start, an end and a dropped line with the reason. It writes no
  file. A tool summary is built from one fixed key of the tool input (`command`,
  `file_path`, or the tool name alone), and the whole input never becomes an
  event.
- **A tool name is a claim.** The name, the summary and the result of a tool come
  from the program. The dock draws them as plain text, and no value of them
  becomes a command, a path or a setting.

## What the bridge guarantees

- **One process, by its own id.** The bridge starts the program of a
  `ProgramSpec` with no shell around it, and its stop ends exactly that process
  id, guarded by the start time of the process, so no signal reaches a later
  process that the system gave the same id. `Bridge::stop` waits at most
  `STOP_TIMEOUT`.
- **The prompt is one line of the standard input.** The caller gives the text to
  `Bridge::send`, which builds the line with `serde_json`. The prompt never
  appears on the command line, in the process list or in a log line.
- **A name of a nested agent is removed.** Every inherited variable with the
  prefix `CLAUDE_CODE_` or `HORIZON_WORK_`, and the variable `CLAUDECODE`, is
  removed from the environment of the child before it starts, so a bridge inside
  an agent panel does not speak as that panel.
- **No line of the program waits on a frame.** The reader, the writer and the
  reader of the standard error are threads of their own. `poll` reads a channel
  with `try_iter`, and `send` queues a line. A caller runs `stop` off the frame,
  as the [UI thread guide](../../architecture/ui-thread.md) requires.

## What the dock draws

The dock is another reader of the untrusted text of the program, so the same
rules hold for it.

- **The text of the program is plain text.** A text of the agent is one label:
  no markdown, no link and no path in it becomes active. The person selects and
  copies it, and nothing else reads it.
- **One frame lays out a bounded number of entries.** The list lays out at most
  the last 200 entries of the conversation, and one notice above them names the
  entries that it does not draw. The conversation itself keeps
  `MAX_ENTRIES` entries, and each text of it is cut before it enters the
  snapshot.
- **No entry becomes a control.** An entry draws no button, no card and no
  click target, and no text of the program reaches a path, a command line, a log
  line or a file.
- **The words of Horizon are its own.** A notice that the dock adds, the state of
  the header and the cost are built by Horizon from typed values, and never from
  the text of a line.

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
