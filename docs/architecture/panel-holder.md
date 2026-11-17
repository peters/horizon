# The holder of a panel

A holder is one process of Horizon in the mode `horizon --panel-holder`. It owns a
terminal and the program in that terminal. Horizon talks to it over a private Unix
socket of the Horizon home. When Horizon ends, the holder and its program continue.
A new Horizon attaches again and reads the last output.

This page gives the design of the holder. The trust boundary is in
[`docs/review/trust-boundaries/panel-holder.md`](../review/trust-boundaries/panel-holder.md).

The setting `features.keep_agents_through_restart`, off at first, decides whether
a panel uses a holder. An agent panel of this computer uses one, and a panel of a
machine, an ssh panel and every other kind of panel start their program in the
process of Horizon as before. The worker of a panel start reads the setting, and it
runs the probe, the start of a holder and the link of the terminal, so no frame
waits for any of them (`crates/horizon-core/src/panel/spawn/holder.rs`).

A start of Horizon also ends the programs of the holders that no panel of the
restored board shows: the person closed the session another way, or the setting is
off now. One holder process per panel costs one thread of its own, so the count of
holders is the count of the agent panels that the board shows.

## The launch document

Horizon writes one JSON document to the standard input of the holder and closes it.
The document is never an argument of the holder: the environment of an agent panel
holds secret values, and the arguments of a process are public to the computer.

| Field | Meaning |
|---|---|
| `local_id` | The identity of the panel in the Horizon home. The first 32 hexadecimal characters of its SHA-256 name the socket. |
| `program` | The program to start. |
| `args` | The arguments of the program. |
| `cwd` | The folder of the program, or none. |
| `env` | The environment of the program. |
| `rows`, `cols` | The first size of the terminal. |
| `cell_width`, `cell_height` | The size of one cell of the terminal. |

The document has a bound of 1 MiB and admits no unknown field. The holder writes no
field of it to a log line or an error text.

## The socket

- The holders of a home live in `panel-holders/`, mode 0700.
- One holder owns `<encoded local_id>.sock`, mode 0600.
- A socket path above 100 bytes is refused with one error. It is not cut.
- A bind first connects to the socket. A socket that answers belongs to a live
  holder, and the bind refuses with `AlreadyServing`. A socket that no process
  listens on is stale and is removed.

## The protocol

One frame is a kind byte, a length of 4 bytes in big-endian order, and the payload.
The payload is at most 1 MiB.

| Kind | Direction | Payload | Purpose |
|---|---|---|---|
| 1 | client to holder | bytes | Input for the program. |
| 2 | client to holder | 8 bytes: rows, columns, cell width, cell height | The new size of the terminal. |
| 3 | client to holder | none | End the program and the holder. |
| 16 | holder to client | version, process id, start time, ring length | The hello of a new link. |
| 17 | holder to client | bytes | The newest output from before the link. |
| 18 | holder to client | bytes | Output of the program. |
| 19 | holder to client | one byte and a status | The program ended. |
| 20 | holder to client | none | Another client is attached. |

The hello names protocol version 1. A client that sends a frame above the bound, or
a kind that this protocol does not know, loses its link and the program continues.

## The life of a holder

1. The mode reads the launch document on the standard input.
2. It calls `setsid`, so the end of Horizon sends no hangup to the holder.
3. It prepares the socket folder, binds the socket, and starts the program in a new
   terminal.
4. It reads the output of the program on its own thread and appends each part to
   the ring.
5. The first client gets the hello, then the ring as replay frames of at most the
   frame bound, then each new part as an output frame. The hello gives the length
   of the ring, and the client reads replay frames until it has that length.
6. A client that closes its link detaches. The program continues and the ring
   continues to fill.
7. A second client while one is attached gets one refusal frame and is closed.
   A client that connects while the loop of the holder is behind waits in no
   queue of its own: the holder closes that link, and the client connects
   again.
8. A `Close` frame ends the program: the master of the terminal closes, and
   `panel/termination.rs` sends `SIGTERM`, then `SIGKILL`, to the process group of
   a program that outlives the hangup. The holder removes its socket and exits.
   The holder then ends the link, so the end of the link tells the client that
   the program is gone.
   The `Close` frame is the one frame that the holder does not lose. The holder
   reads the frames of a client before it writes the replay, and it obeys a
   `Close` of a client that it detached before. A client that takes no replay is
   detached after the client write bound, and its `Close` still ends the program.
9. A program that ends by itself leaves its status with the holder. A client that
   is attached gets the status in one exit frame. The holder then waits for the
   client to close, at most 5 seconds, removes its socket and exits.
10. A program that ends with no client keeps its status for one hour. A client that
    attaches in that hour gets the hello, the replay and the status. After one hour
    the holder removes its socket and exits.

## The loop of a holder waits

The loop of a holder blocks in one channel and makes a turn only for a happening:
a part of the output of the program, the end of that output, a new client, the
frames of the client, or the end of that link. The reader of the program, the
listener of the socket and the reader of the client each have their own thread,
and they send a happening when one occurs. An idle holder with an attached client
therefore makes no turn, and it polls neither its socket nor its program.

The loop waits on that channel with a deadline in two cases: for the client to
close after the exit frame, and for the hour that keeps the status of a program
that ended with no client. After the output of the program ends, the holder waits
at most two seconds for the event of its child, because the system reports an exit
a moment after the hangup of the terminal.

One part of the loop blocks on a write: the holder writes the output of the program
to its client. The write has a bound of 5 seconds, so a client that does not read
its output is detached, the program continues, and the ring keeps the output. The
output that waits for that client is bounded by the ring and by the queue of
happenings.

## The constants

| Constant | Value | Reason |
|---|---|---|
| Ring bound | 2 MiB | The newest output of a program that a person reads after a restart. |
| Frame bound | 1 MiB | One frame holds one read of the terminal, and no frame lets a client hold the memory of the holder. |
| Status kept | 1 hour | A restart of Horizon in the same working day still reads the end of its program. |
| Close wait | 5 seconds | A client that reads the status closes its link, and a client that does not must not hold the holder for ever. |
| Socket path bound | 100 bytes | The bound of a Unix socket path is 104 bytes on macOS, and the holder refuses a longer path. |
| Start bound | 5 seconds | A start waits for the hello of a new holder and reports one error after this time. |
| Client write bound | 5 seconds | A client that takes no output is detached after this time, and the program continues. One frame can wait two times this bound. |
| End bound | 6 seconds | A start waits this time for the end of a holder that it closed: the hangup, the two signals and one read. |
| Queue of happenings | 8 | The parts of output and the links that wait for the loop, which bounds the memory of a holder that is behind. |

## The functions of this part

- `panel_holder::serve` runs the mode. It is the only entry of the process.
- `panel_holder::start` starts a holder and answers its hello.
- `panel_holder::linked` links to a holder and answers its hello and its replay, or
  nothing when no holder listens. A holder that still serves the client of a probe
  answers a refusal, so this tries again until its read bound passes.
- `panel_holder::probe` answers the hello of a live holder, or nothing when no
  holder listens.
- `panel_holder::close_unshown` ends the program of every holder whose panel is on
  no restored board. It reads at most 64 sockets in one start. For each holder it
  reads the hello, sends the `Close` frame, reads the replay and the output, and
  waits for the end of the link for the end bound. It answers the count of the
  holders that ended and the count of the holders that are alive after that time.
  The sidebar shows the two counts, and the log has one warning for the holders
  that are alive. It removes only a socket that refuses a link. It does not remove
  the socket of a holder that accepts a link, because nothing can find that holder
  again.

`start`, `linked`, `probe` and `close_unshown` start a process or wait for a socket,
so only a worker thread calls them.

## A panel that ends its program

A drop of the terminal of an attached panel is a detach: the link closes and the
program continues. Only these paths send the `Close` frame, which ends the program
as a local panel ends:

| Path | Where |
|---|---|
| The person closes the panel | `Board::close_panel_returning_teardown` |
| The person closes the workspace of the panel | `Board::close_panels_in_workspace`, which calls the row above |
| The person restarts the panel | `Board::begin_restart` ends the program, and the pre-flight of the restart starts a new holder |

The end of Horizon, a crash of it and a session that replaces the board send no
`Close` frame, so the program continues in its holder. A start of Horizon then
finds the holder of every panel of the restored board and attaches to it, and it
ends the program of a holder that no panel shows.

## What a held program keeps from the Horizon that started it

A program that a holder runs was started by one Horizon process. Its arguments,
its environment and the tool server that it started name things of that process.
The table gives each of them and what it names after that Horizon ended.

`<host>` is the identity of one Horizon process, a random value. `<host folder>`
is `runtime/agent-plugins/<encoded host>` in the Horizon home.

| Value | Who gets it | After a restart of Horizon |
|---|---|---|
| `--plugin-dir <host folder>/claude-code` | Claude | The folder stays. The next Horizon uses the same folder and writes its files again. |
| `--plugin-dir <host folder>/work-resume` | Claude, when the panel continues its work | The folder stays, so the hooks of the program run. |
| `-e <host folder>/work-report/pi-report.mjs` | pi | The file stays. |
| `HORIZON_BROWSER_HOST_INSTANCE=<host>` | Each agent, and the tool server that it starts | The next Horizon takes the same identity, so it answers the tools of the program. |
| `HORIZON_BROWSER_ACTOR` | Each agent | It names the panel, and it does not change. |
| `HORIZON_WORK_ROOT`, `HORIZON_WORK_PANEL`, `HORIZON_WORK_OWNER`, `HORIZON_WORK_KIND` | Claude and pi, when the panel continues its work | They name the home, the panel and the owner in the work ledger of the home. They do not change, and an attach writes no new owner. |
| `HORIZON_WORK_EXECUTABLE` | The same panels | The path of the program file of Horizon. It is correct while that file is on the disk. |
| `HORIZON_EXECUTABLE`, and `HORIZON_BROWSER_MCP_EXECUTABLE` of Grok | Each agent | On Linux it is `/proc/<pid>/exe` of the Horizon that ended, which names no file. On macOS it is the path of the program file. |
| `-c mcp_servers.horizon-browser.command=...` | Codex | The same value as `HORIZON_EXECUTABLE`. The tool server that Codex started continues. Codex cannot start it again on Linux. |
| `command` in `<host folder>/claude-code/.mcp.json` | Claude | The next Horizon writes the file again with its own program, so Claude can start its tool server again. |
| The named secrets of the computer | Each agent | The values of the start. A changed secret reaches only a new program. |

The rule has two parts, and one file gives the two of them:

1. **The folder of a held program stays while its holder lives.** A Horizon makes
   the file `<encoded host>.held` beside its host folder. Each holder that this
   Horizon starts keeps a shared lock on that file
   (`panel_holder/serve.rs`, `plugin_folder_lease`). The end of a Horizon and the
   prune of a later start remove a host folder only when no holder has that lock
   (`crates/horizon-ui/src/plugin_install/`, `held_by_a_holder`). A start after the
   last holder of that Horizon ended removes the folder, its lease and that file.
2. **The next Horizon takes the identity of a held program.** A start looks for a
   host folder that a holder locks and that no Horizon has the lease of
   (`kept_host`). It takes that lease and that identity before any part of it
   reads its identity. The lease is exclusive, so two Horizons that run at the
   same time never have the same identity.

### What a kept agent can do after a restart

- It continues its conversation in the same process, and it gets the keys of the
  panel.
- Its hooks run, because the folders of its `--plugin-dir` arguments stay.
- Its Horizon tools (`browser_*`, `block_panel`, `device_panel` and the others)
  reach the new Horizon, because the new Horizon has the identity that the tool
  server sends.

### What a kept agent cannot do after a restart

- A tool call that the old Horizon held when it ended gets no answer. An example
  is a `block_panel` call with the operation `next_click` that waits for a click.
  The call ends at its own time limit, and the next call works.
- On Linux, `HORIZON_EXECUTABLE` names no file, and Codex cannot start its tool
  server again. A restart of the panel repairs the two.
- If two Horizons of one home ended and each left held programs, the next Horizon
  takes the identity of one of them. The tools of the programs of the other get no
  answer, and their folder stays until their holders end.
- A Horizon with `HORIZON_AGENT_INTEGRATIONS=off` makes no host folder, so the
  next Horizon does not take its identity, and the tools of its held programs get
  no answer after a restart.
- No test of this repository starts the real Claude or Codex program. The live
  procedure proves the hooks and the tool calls.
