---
applyTo: "crates/horizon-sync/src/**,crates/horizon-sync/Cargo.toml,crates/horizon-core/src/atomic_file.rs"
---

# Trust boundary of the server store and its link

The Horizon server holds the state that more than one computer agrees on, and a
computer reaches it over a link. Every byte on that link comes from another
computer, and the server acts on it. The module comment of
[`crates/horizon-sync/src/lib.rs`](../../../crates/horizon-sync/src/lib.rs) and
the design page [`docs/architecture/horizon-server.md`](../../architecture/horizon-server.md)
describe the same boundary.

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
  directory, written as a whole through a temporary file and a rename.

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
