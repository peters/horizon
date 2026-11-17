---
applyTo: "crates/horizon-core/src/panel_holder/**,crates/horizon-core/src/panel/spawn/holder.rs,crates/horizon-core/src/panel/spawn/start.rs,crates/horizon-core/src/terminal/lifecycle.rs,crates/horizon-core/src/terminal/content.rs,crates/horizon-pty-tap/src/socket.rs,crates/horizon-pty-tap/src/panel.rs,crates/horizon-ui/src/cli.rs,crates/horizon-ui/src/main.rs,crates/horizon-ui/src/plugin_install/mod.rs,docs/architecture/panel-holder.md"
---

# Trust boundary of the holder of a panel

A holder is one process of Horizon in the mode `horizon --panel-holder`. It owns a
terminal, the program in that terminal, and a private Unix socket of the Horizon
home. It keeps the program alive when Horizon ends, and a new Horizon attaches
again. A panel uses a holder when the setting `features.keep_agents_through_restart`
is on, which is off at first.

The design is in [`docs/architecture/panel-holder.md`](../../architecture/panel-holder.md).
The socket idiom follows [`crates/horizon-sync/src/link.rs`](../../../crates/horizon-sync/src/link.rs).

## Untrusted: the program and what it writes

- **The program is not a program of Horizon.** A panel runs an agent, a shell or a
  command of the person. The holder starts it and copies its bytes. It reads no
  command, no path and no escape sequence of those bytes.
- **The bytes of the program reach the client unchanged.** The client is a
  terminal of Horizon, and it parses those bytes with `alacritty_terminal` under
  the rules that a terminal already applies to a local panel. The holder adds no
  parse and no filter: a byte that a program writes is a byte that the terminal of
  the client reads.
- **The program can end itself at any time, with any status.** The holder keeps
  that status and sends it in one exit frame. A program that ends leaves its text
  in the ring, and the holder keeps the ring and the status for the next client
  for one hour.

## Untrusted: any process of the same user

- **The socket is `0600` in a `0700` folder of the Horizon home.** That refuses
  every other user of the computer. It does not refuse a process that runs as the
  owner, and that is the stated limit of this part: a process of the same user can
  connect to the socket and become the one client of the holder.
- **A client of the holder can type into the program and read its output.** That
  is what a client is for. A process of the same user that attaches therefore
  reads the last screens of the panel and writes keys into the program, and the
  holder cannot tell it from Horizon.
- **One client at a time.** A second client gets one refusal frame and is closed,
  so a process of the same user cannot read the ring while Horizon holds the link.
- **A terminal that attaches trusts the holder of its socket.** The attached
  terminal parses the frames of that socket with `alacritty_terminal` under the
  rules of a local panel, and it sends the keys that a person types into the panel
  to the same socket. A process of the same user that serves that socket can
  therefore send any bytes to the terminal of the panel and read what the person
  types, and the attached terminal cannot tell it from the holder.
- **A stale socket is replaced, a live one is not.** A socket that answers a
  connect belongs to a holder that runs, so a second holder for the same panel
  refuses with `AlreadyServing` and ends. A socket that no process listens on is
  what a killed holder left, and the next holder removes it before its bind.

## Untrusted: the launch document

- **The launch arrives on the standard input of the holder, never in its
  arguments.** The environment of an agent panel holds secret values, and the
  arguments of a process are visible to every process of the computer. The holder
  writes no field of the document to a log line, an error text or an argument.
- **The document is one JSON object, bounded at 1 MiB, with no unknown field.** A
  document above the bound or with a field that this build does not know ends the
  holder with one typed error and starts no program.
- **The identity of the panel is not trusted.** The holder names its socket with
  the first 32 hexadecimal characters of the SHA-256 of the identity, so an
  identity is never a path part and the name has one length.
  A socket path above 100 bytes is refused with one typed error and is not cut: a
  cut path would name another panel.
- **The program, its arguments, its folder and its environment are the launch of
  the panel.** The holder starts exactly that program and no other. The mode takes
  no argument that names a program, a path or a socket.

## The guarantees

- **One client, and a detach is not an end.** A client that closes its link
  detaches. The program keeps running and the ring keeps filling, so the next
  client reads the text from before and during the detach.
- **Every frame is bounded.** One frame holds at most 1 MiB of payload. A frame
  above the bound, and a kind that this protocol does not know, end that client's
  link and leave the program running. The output frames hold bytes and no JSON.
- **The ring is bounded.** A holder keeps the newest 2 MiB of the output of its
  program, and no more.
- **The holder starts its own session.** It calls `setsid` in the mode of a
  holder, so the end of the Horizon that started it sends it no hangup. The
  `Close` frame, and only that frame, ends the program of a holder.
- **A `Close` ends the program as Horizon ends a panel.** The master of the
  terminal closes, `panel/termination.rs` sends `SIGTERM` to the process group
  after `AFTER_HANGUP`, and `SIGKILL` after `AFTER_TERM`. The holder then removes
  its socket and exits.
- **A `Close` is obeyed from every link that got a hello.** The holder reads the
  frames of a client before it writes the replay, and it obeys a `Close` of a
  client that it detached. This gives a process of the same user no new power:
  that process can attach and send a `Close` at any time.
- **The end of a holder that no panel shows has a bound.** A start sends the
  `Close`, reads what the socket sends, and waits at most 6 seconds for the end of
  the link, for at most 64 sockets, on a worker thread. A process that serves a
  socket and does not end the link costs that time and one line in the sidebar.
  It cannot stop a frame. The start does not remove a socket that accepts a link.
- **A Horizon takes the identity of a Horizon that ended only with its lease.**
  The tools of a held program send the identity of the Horizon that started the
  program, and a Horizon answers only its own identity. A start takes the
  identity of a host folder that a holder locks, and only after it has the
  exclusive lease of that folder. A Horizon that runs keeps its lease, so no
  second Horizon answers its agents. The name of the folder is read back with
  `local_id_of`, which refuses each name that `safe_local_id` did not write.
- **The socket folder is private and the holder makes it so.** The folder is
  0700 and the socket is 0600, whatever the umask of the process is.
- **The holder serves a socket of this computer only.** It opens no port, starts no
  other program, and reads no path of a frame.

## What the guarantees do not cover

- **It is not a sandbox.** The program has the rights of the person, as a program
  of a local panel has them today. The holder limits the link to the socket, not
  what the program does.
- **A process of the same user can attach.** The socket mode is the whole
  boundary. A local process of the owner can read the last 2 MiB of the output of
  a panel and type into its program while Horizon is detached, and through a
  program that is an agent it can reach everything that agent may do.
- **The replay is a byte tail, not a screen.** A program that painted once and
  then wrote differences can come back with a partial screen. A terminal of the
  client redraws what the tail holds, and the tail is cut at a byte, not at a line.
  The attach asks the program for a repaint with a size that changes, and a
  program that repaints at the window-change signal shows its full screen. The
  attach cannot give back a mode, such as the alternate screen or the mouse
  reports, that the program set before the first byte of the tail.
- **A holder that outlives its home.** If the Horizon home is deleted or moved
  while a holder runs, the holder keeps the program with a socket that no client
  reaches. The next start of Horizon cannot attach to it, and the program stays
  until the person ends it.
- **What a program inherits.** The environment of the launch is the environment of
  the Horizon that started the panel, not of the Horizon that attaches again.
- **No encryption and no authentication.** The link is a Unix socket of one
  computer. Horizon adds no handshake beyond the hello of the protocol, so the
  first client that connects is the client of the holder.
