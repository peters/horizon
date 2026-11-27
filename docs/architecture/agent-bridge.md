# Agent bridge

The agent bridge runs one agent program with no terminal. Horizon sends the
messages of the person to the program as JSON lines, and it reads the JSON lines
of the program as a small set of typed events. Claude Code is the only program of
this slice.

The module is `crates/horizon-core/src/agent_bridge/`. Its page
[`docs/review/trust-boundaries/agent-bridge.md`](../review/trust-boundaries/agent-bridge.md)
states its trust boundary. Nothing uses the module yet: its only callers are its
tests.

## Purpose

- One conversation has one program. The module starts the program, writes each
  message of the person to its standard input, and turns the lines of its
  standard output into events.
- The module holds no terminal, no screen and no escape sequence. The program
  draws nothing, so no panel and no PTY is part of this path.
- The module holds no storage and no setting. A caller gives it a
  `ProgramSpec`, one codec and one wake function.

## The threads

| Thread | Work |
|---|---|
| `agent-bridge-out` | Reads the standard output by lines, parses each line with the codec, sends the events of that line and calls the wake after each line that gave an event. |
| `agent-bridge-in` | Writes each line that `Bridge::send` queued to the standard input, and flushes it. The end of the queue closes the pipe. |
| `agent-bridge-err` | Reads the standard error and keeps its last bytes for the notice of the end. |

The end of the standard output is the end of the conversation. The reader waits
for the child, joins the reader of the standard error, sends
`BridgeEvent::Ended` with the exit code and the tail of the words, and calls the
wake. A turn of the program is not the end of its process: the `result` line ends
a turn, and the process can stay alive after it.

`Bridge::poll` reads the channel with `try_iter`, so a frame never waits.
`Bridge::send` queues one line and never waits for the program. `Bridge::stop`
closes the standard input, waits, then ends the child by its own process id when
the child ignores both steps, and joins the threads. `Drop` asks for the same
stop and waits for nothing.

## The bounds

| Name | Value | What it bounds |
|---|---|---|
| `MAX_LINE_BYTES` | 1 MiB | One line of the program. A longer line is read to its newline, dropped whole and reported as `DropReason::TooLong`. |
| `MAX_TEXT_BYTES` | 256 KiB | One text of the person or of the agent. |
| `MAX_SUMMARY_CHARS` | 400 | One summary of a tool, of a tool result, of a refusal and of a notice. |
| `MAX_ENTRIES` | 2000 | The entries of one conversation. |
| `STDERR_TAIL_BYTES` | 8 KiB | The last bytes of the standard error that the bridge keeps. |
| `STOP_TIMEOUT` | 5 s | One stop of one bridge. |

The codec joins the texts of the lines that share one `message.id`. Its memory has
two bounds: 256 tracked messages, and `MAX_TEXT_BYTES` for one message. A full
map is emptied, and a message id that the map lost starts a new text, which the
conversation replaces as before.

## The events

| Event | Line of the program | Fields |
|---|---|---|
| `SessionStarted` | `system` with `subtype` `init` | `session_id` |
| `TurnStarted` | the same `init` line | none |
| `AgentText` | `assistant` with a `text` block | `message_id`, the whole text of the message so far |
| `ToolStarted` | `assistant` with a `tool_use` block | `tool_id`, `name`, `summary` |
| `ToolEnded` | `user` with a `tool_result` block | `tool_id`, `failed`, `summary` |
| `PermissionDenied` | `system` with `subtype` `permission_denied` | `tool`, `message` |
| `Limit` | `rate_limit_event` | `status`, `using_overage`, `resets_at_unix` |
| `TurnEnded` | `result` | `failed`, `duration_ms`, `cost_usd` |
| `Dropped` | a line that the codec refused | `reason`: `TooLong`, `NotJson` or `UnknownShape` |
| `Ended` | the end of the standard output | `exit_code`, `stderr_tail` |

A summary of a tool is built from one fixed key of the tool input and nothing
else: `command` for `Bash`, `file_path` for a file tool, and the tool name for
every other tool. Control characters become one space, and the summary is cut to
`MAX_SUMMARY_CHARS`. The whole tool input never reaches an event.

`TurnEnded.cost_usd` is the cost of one turn. Claude Code reports
`total_cost_usd` for the whole process, so the codec sums the `costUSD` of the
models of `model_usage_since_last_result`.

## The Claude line format

The line format below was measured on Claude Code 2.1.332 with these arguments:

```
claude -p --input-format stream-json --output-format stream-json --verbose \
       --session-id <uuid> | --resume <uuid>
```

- **Input.** One JSON line for one message:
  `{"type":"user","message":{"role":"user","content":[{"type":"text","text":"…"}]}}`.
  The codec makes the line with `serde_json`, so every character is escaped and
  the line holds no newline. The prompt never goes on the command line.
- **Many turns in one process.** One process serves many turns. A turn ends with
  exactly one `result` line, and the process stays alive while its standard input
  is open.
- **A new `system/init` line at every turn.** The line names the session id. A
  resumed process reports the same id.
- **One content block per `assistant` line.** Several lines can hold one message,
  and they share `message.id`, so the codec joins their texts.
- **Tool call and result.** The call is an `assistant` line with a `tool_use`
  block (`id`, `name`, `input`). The result is a `user` line with a `tool_result`
  block (`tool_use_id`, `content`, `is_error`).
- **A refusal is a line, not a wait.** A refused tool gives
  `system/permission_denied`, then a `tool_result` with `is_error: true`, and the
  turn still ends with a `result` line.
- **The limit is one line per turn.** `rate_limit_event` holds the state of the
  window of the account and whether the account uses overage.
- **Lines that this slice does not use** give nothing: `stream_event`,
  `system/status`, `system/thinking_tokens`, `system/hook_started`,
  `system/hook_response`. A type of a later version also gives nothing.
- **No non-JSON line appeared on the standard output of any measured run.** The
  standard error of Claude Code was empty. The bridge still reads both, because
  another program or a later version can write there.

## What is not built yet

- No window, no card, no storage and no setting. No user of this module exists.
- One program: Claude Code. Nothing chooses the program for the person, and
  `ProgramSpec` holds no path of an installed program.
- A conversation does not resume after a restart of Horizon. The caller chooses
  `SessionChoice::New` or `SessionChoice::Resume` and holds the session id.
- A card, a browser tool and a wake line of a card are later slices. The module
  calls `crate::ui_thread::blocking` nowhere: it starts no work on a frame.
