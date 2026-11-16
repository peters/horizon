# The holder of a panel

A holder is one process of Horizon in the mode `horizon --panel-holder`. It owns a
terminal and the program in that terminal. Horizon talks to it over a private Unix
socket of the Horizon home. When Horizon ends, the holder and its program continue.
A new Horizon attaches again and reads the last output.

This page gives the design of the holder. The trust boundary is in
[`docs/review/trust-boundaries/panel-holder.md`](../review/trust-boundaries/panel-holder.md).
The setting that uses a holder and the attach of a terminal are later parts, so no
panel uses a holder yet.

## The launch document

Horizon writes one JSON document to the standard input of the holder and closes it.
The document is never an argument of the holder: the environment of an agent panel
holds secret values, and the arguments of a process are public to the computer.

| Field | Meaning |
|---|---|
| `local_id` | The identity of the panel in the Horizon home. It names the socket, encoded with `safe_local_id`. |
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
5. The first client gets the hello, then the ring as one replay frame, then each
   new part as an output frame.
6. A client that closes its link detaches. The program continues and the ring
   continues to fill.
7. A second client while one is attached gets one refusal frame and is closed.
   A client that connects while the loop of the holder is behind waits in no
   queue of its own: the holder closes that link, and the client connects
   again.
8. A `Close` frame ends the program: the master of the terminal closes, and
   `panel/termination.rs` sends `SIGTERM`, then `SIGKILL`, to the process group of
   a program that outlives the hangup. The holder removes its socket and exits.
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
| Client write bound | 5 seconds | A client that takes no output is detached after this time, and the program continues. |
| Queue of happenings | 8 | The parts of output and the links that wait for the loop, which bounds the memory of a holder that is behind. |

## The functions of this part

- `panel_holder::serve` runs the mode. It is the only entry of the process.
- `panel_holder::start` starts a holder and answers its hello.
- `panel_holder::probe` answers the hello of a live holder, or nothing when no
  holder listens.

`start` and `probe` start a process and wait for a socket, so only a worker thread
calls them.
