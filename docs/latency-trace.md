# Measure the latency of a key press and of a browser action

A far machine adds a round trip to every action. Horizon measures the hops of
two actions, so a report of a slow panel gives numbers and not a guess. Read
[remote connections](remote-connections.md) first for the bounds of a link:

- the press of a key in a terminal panel, from the window event to the painted
  echo of that key;
- an input action of a browser panel, from the UI event to the painted pixel.

Horizon writes these lines only when you ask for them. Each line is a `tracing`
INFO line on stderr, so it needs no log file and no setting of the window.

## 1. Turn on the lines

Set the switch of the action that you measure before you start Horizon:

| Action | Switch | Module that writes the line |
|---|---|---|
| A key press in a terminal panel | `HORIZON_TERMINAL_LATENCY=1` | `horizon_core::terminal::latency` |
| An input action of a browser panel | `HORIZON_BROWSER_LATENCY=1` | `horizon_browser::latency` |

```bash
HORIZON_TERMINAL_LATENCY=1 ./target/release/horizon
```

Result: Horizon starts and writes the lines of the switch that you set.

The switch is the only setting that these lines need:

- An empty value, or the value `0`, keeps the lines off.
- A value other than `0` turns the lines on.
- A switch that is on adds the module of its own line to the filter of the
  window. A `RUST_LOG` that would hide that module does not hide the line.
- A switch adds no other module, so the rest of the log keeps the level that
  `RUST_LOG` gives it.
- The switch works in a release build. It is not a part of the `trace-profiling`
  feature.
- While a switch is off, its hooks cost one relaxed atomic load.

## 2. Read the line of a key press

The target of the module comes before the message of the line. One line of a
key press has these fields, always in this order:

```
latency key seq=4 bytes=1 event_to_write_us=310 write_to_read_us=178400 read_to_request_us=1240 request_to_paint_us=8100 total_us=188050
```

| Field | From | To |
|---|---|---|
| `seq` | The number of the press of this terminal, from 1. |
| `bytes` | The number of bytes that the key wrote to the terminal. |
| `event_to_write_us` | The frame that took the key event. | The write of the bytes into the event loop of the terminal. |
| `write_to_read_us` | That write. | The first output of the terminal after it. For a shell of a far machine, this is about one round trip. |
| `read_to_request_us` | That read. | The frame that took the changed grid. |
| `request_to_paint_us` | That frame. | The painted grid. |
| `total_us` | The key event. | The pixel. |

A press that no frame painted within five seconds writes no line. A terminal
keeps the last sixteen presses that wait for a painted echo.

## 3. Read the lines of a browser action

One input action writes up to four lines:

| Line | What it says |
|---|---|
| `latency action` | The first picture after one input was written: the times from the UI event to the handoff to the UI, the size of that picture, its decode time, and whether the ack left before or after it. |
| `latency frame` | Every picture, with its bytes, its size and its decode time. |
| `latency paint` | The frame that applied a picture to a texture, so the last hop of the same action is measurable. |
| `latency stall` | A driver pass that blocked for more than 50 ms, with the name of the largest wait of that pass. |
| `latency picture` | The first picture of a session, with the rect of the body, the size of the picture and the rect that the window painted. A picture that does not fill the body is one of these two: a session that the panel has not resized yet, or a body rect that is not the rect of the panel. |

The times of these lines are microseconds from the UI event of the action, as
the line of a key press gives them. The line `latency picture` carries no time,
because it answers a question of geometry and not a question of time.

A page that never reaches the size of the panel is asked for it again at a slow
rate, one command every two seconds, so the picture and the input mapping come
back without a manual resize of the panel.

### What the `latency stall` line names

The line carries the largest wait of the pass, and every blocking step of a pass
names itself:

| Name | The wait |
|---|---|
| a CDP command, such as `Page.captureScreenshot` | The protocol owed that answer. The line names the command itself. |
| `drain-commands` | The channel of the commands of the window, and the work of the commands that it held. |
| `cdp-read` | One socket read of the protocol, with its timeout. |
| `http-bodies` | The response body that the page asked for. |
| `file-chooser`, `passkeys` | The dialog state and the passkey state of the page. |
| `manifest-write`, `audit-write` | A file write of the manifest or of the journal of the panel. |
| `title-fetch`, `viewport-resize`, `scrollbar-layout`, `screencast-restart` | The CDP calls of those steps. |
| `chrome-liveness`, `agent-signals`, `pending-requests` | The process check, the signals of an agent, and the requests that wait. |
| `idle-wait` | The wait for the next command at the end of a pass. |

The wait of `idle-wait` ends as soon as a command arrives, so the line shows it
only when a command came late. The read of the protocol has a bound of 4 ms and
the wait behind it a bound of 12 ms: an input that arrives while the page is
quiet reaches the socket within one frame.

## 4. Compare two runs

Compare runs that have the same workload:

- the same panel layout, the same interaction and the same build profile;
- the same `HOME`, so the session and the settings are the same;
- one action per run, so a table of the lines reads down the same columns.

The line of a key press answers the question "where did the time of this press
go". A `write_to_read_us` of about one round trip and a `read_to_request_us` of
about one frame are the healthy case. A `read_to_request_us` of about one
second was the poll of the window before the reader thread woke it.

## 5. Turn the lines off

Start Horizon again without the switch, or set the switch to `0`.

Result: Horizon writes no line of that action.
