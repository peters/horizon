---
applyTo: "crates/horizon-sync/src/**,crates/horizon-sync/Cargo.toml,crates/horizon-core/src/atomic_file.rs,crates/horizon-core/src/config/server.rs,crates/horizon-core/src/machines/registry.rs,crates/horizon-ui/src/app/server_sync.rs,packaging/horizon-server/**"
---

# Trust boundary of the server store and its link

The Horizon server holds the state that more than one computer agrees on, and a
computer reaches it over a link. Every byte on that link comes from another
computer, and the server acts on it. The module comment of
[`crates/horizon-sync/src/lib.rs`](../../../crates/horizon-sync/src/lib.rs) and
the design page [`docs/architecture/horizon-server.md`](../../architecture/horizon-server.md)
describe the same boundary.

Two programs touch the boundary. The server is `horizon --server`: it holds the
store and the socket of one server home and starts no other process. A computer
is `horizon --server-link`, one child of one `ssh` call of that computer: it
relays the bytes of its own standard input and output to the socket of the
server and reads no file of the person. The computer that holds the link runs the
child on a worker thread through `crates/horizon-sync/src/supervisor.rs` and
never on the UI thread.

## Untrusted: every byte of the link and every field of an intent

- **The stream is one computer.** The first frame must be a `hello`. A frame
  before it, a second `hello`, a frame that is not one document of the protocol,
  and a frame above 256 KiB end the link with a typed error frame.
- **The device name is a claim.** It is 1 to 40 characters of `a-z`, `0-9` and
  `-`, and a name that breaks the rule ends the link. The transport does not prove
  the name: whatever `ssh` let in may claim any device name.
- **An intent names its computer.** An intent whose device differs from the name
  of the link is refused, so no computer spends the counters of another one.
- **A record is checked at the server.** A machine record passes the validators of
  `horizon_core::machines::check_record`: the name rule, the route rule of an SSH
  terminal, the session rule and the provider line. A record that breaks a rule is
  `invalid_record` with the code of that rule, and nothing is written.
- **The intents are bounded.** One push holds at most 256 intents, the store
  keeps at most 64 machines and 64 internal probe objects, and one link holds at
  most 32 open waits. A push above its limit and a wait above its limit get a
  typed error.
- **A clock of a computer is never read.** An intent carries no time. The server
  stamps every change with its own clock.

## Untrusted: the setting that names the server

- **The target is one argument of `ssh`.** `server.ssh_target` is at most 255
  characters, it does not start with `-`, and it holds no space and no control
  character, because `ssh` takes it as one argument. `ssh` still reads the
  person's own SSH configuration for that name, as it does for a browser host.
- **The command is one line that the remote shell reads.** `server.command` is at
  most 200 characters of letters, digits and `._:/=@+-`, with at most one space
  between words, so it starts one program and no second command. It is empty by
  default, and then the link runs `horizon --server-link`.
- **Both values belong to the person.** A hand-edited setting runs the `ssh` that
  the person configured for that name. The boundary of this page starts at the
  bytes of the child, not at the setting.

## What a computer accepts from the server

- **A record is checked on the computer too.** A machine row of a pull passes
  `horizon_core::machines::check_record` before it reaches the mirror or the
  sidebar, so a record that the server holds in a state that breaks a rule is
  refused and logged. The server checks the same record before it stores it.
- **A restored or malicious server can change only the machine records.** It can
  add, change or remove a machine row. It cannot make the computer run a command:
  the only process is the `ssh` of the validated setting, and the tmux command of
  a terminal is rebuilt from the record by Horizon's own text.
- **It cannot make a computer use a key.** The key of a machine is a local
  binding in `machines/keys.json`, and it is never sent. A machine that only
  arrived from the server is marked in the mirror, so a click on it opens no
  login with the keys of the person; the section says
  `This computer has no key for <name> yet`.
- **It cannot make a computer write outside its own files.** The mirror, the
  queue and the device name are three files of the machine directory of the
  Horizon home, at 0600 in a 0700 directory, and an object id is never a path.
- **A refused intent is dropped, not retried for ever.** The queue of a computer
  drops an intent that the server refused and logs the reason with its code.

## The guarantees

- **One writer.** One thread owns the write connection. Each push is one
  `BEGIN IMMEDIATE` transaction with no network call inside it, and the answer
  leaves after the commit. A reader uses a read connection of its own, so a
  reader never holds the writer.
- **The index has no gaps.** Each decided intent takes the next log index inside
  its transaction.
- **A resend changes nothing.** An intent that the server already decided answers
  its first result, and the log keeps one row for it.
- **The version rule holds.** The first intent on a versioned object wins and a
  later intent on the same version is refused with both numbers.
- **No path and no command of a computer is run.** The server opens one file of
  its own, which the caller named when it opened the store. It starts no process,
  opens no network connection, and reads no path from a frame. An object id is 1
  to 64 characters of `a-z`, `0-9`, `-`, `_` and `.`, and it is never a path.
- **The server holds no secret.** It stores the machine list and, in later
  slices, public key lines, cards, clicks and waits. A machine record holds no key
  name, and the server never holds a private key, a token, a terminal content, a
  transcript, a browser profile or a setting of the person.
- **A refusal quotes a bounded part of a frame.** A value that the server copies
  into the log or into a refusal is at most 64 characters and then three dots, so
  one intent writes about a line of log and not a frame.
- **The link ends with a typed error.** A link that cannot continue stops writing
  before it closes, so the computer reads the error frame and not a reset.
- **The socket is private.** A Unix socket of the server is `0600` in a directory
  of the server user, a file that is not a socket is left alone, and a socket that
  a stopped server left is removed before the bind.
- **The queue file is private.** The queue of a computer is `0600` in a `0700`
  directory, written as a whole through a temporary file and a rename. The mirror
  of the machine list and the device name are written the same way.
- **The child is one program with bounded flags.** The computer runs `ssh -T`
  with `BatchMode=yes`, a connect bound of 5 seconds and keepalives of 5 seconds
  with two tries. It writes the frames to the standard input of that child and
  reads them from its standard output; the standard error of the child goes to
  the log, one bounded line at a time, and is never stored.
- **A stop of the server ends the links.** A stop of the server or of its
  process closes the socket of every open link, so a computer reads the end of
  the link and reaches the server again when it is back. It never opens a port
  and never starts a server of its own.

## What the guarantees do not cover

- **Who the computer is.** The store records the device name that the link
  claimed. It does not prove it, and it does not limit what a person who can reach
  the server may read or write.
- **Availability.** A computer that never closes its link holds one thread of the
  server, and the store answers one push at a time. Another process of the same
  server user can open the file, and nothing here prevents that.
- **The content of a body of an internal object.** The `probe` kind is bounded in
  number and in frame size, and its body is JSON that the server does not read.
  No person or agent sees it: a later slice replaces it with the kinds that a UI
  draws, each with its own validators.
- **A clock of the server.** The time of a change is the clock of the server. A
  server whose clock is wrong stamps wrong times, and a reader that compares two
  changes of two servers compares two clocks.
- **Who may reach the server.** The boundary is the SSH login of the server
  computer. Whatever `ssh` lets in may claim any device name and read and write
  the whole machine list. A hardware key on that login and a list of the
  computers that may reach it are not part of this slice.
- **A key on the server.** A later slice gives the server the public lines of the
  machine keys. Until then a machine that arrived from another computer has no
  key here, and the person adds it on this computer.
