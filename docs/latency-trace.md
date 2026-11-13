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
| `latency stall` | A driver pass that blocked on the protocol for more than 50 ms, with the name of the command that it waited for. |

The times of these lines are microseconds from the UI event of the action, as
the line of a key press gives them.

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
