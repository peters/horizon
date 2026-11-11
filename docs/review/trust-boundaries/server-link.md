---
applyTo: "crates/horizon-sync/src/**,crates/horizon-sync/Cargo.toml,crates/horizon-core/src/atomic_file.rs,crates/horizon-core/src/config/server.rs,crates/horizon-core/src/machines/registry.rs,crates/horizon-core/src/secret/**,crates/horizon-core/src/secret_store/**,crates/horizon-ui/src/app/server_sync.rs,crates/horizon-ui/src/app/block_machine_keys.rs,crates/horizon-ui/src/app/settings/remote_browsers.rs,packaging/horizon-server/**"
---

# Trust boundary of the server store and its link

The Horizon server holds the state that more than one computer agrees on, and a
computer reaches it over a link. Every byte on that link comes from another
computer, and the server acts on it.

**This boundary changed on purpose.** An earlier rule said that the server never
holds a private key. The person decided that a second computer must work at once:
it names the server, and it logs in to every machine and uses every provider
credential of the person. The server therefore holds every machine key and every
credential value, sealed with a key of its own home, and every linked computer
holds a copy of all of them. That is the cost of the decision, and it is not a
defect to report: a person or a process that can log in to the server account
reads every machine key and every credential value, because the key file is
beside the database, in the same home and under the same login. The module comment of
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
- **The intents are bounded.** One push holds at most 256 intents and 8 secrets,
  the store keeps at most 64 machines, 64 internal probe objects and 256 secrets,
  one secret body holds at most 16 KiB, and one link holds at most 32 open waits.
  A push above its limit and a wait above its limit get a typed error.
- **A secret id is checked at the server, and its body is not read.** An id is
  `machine-key/<name>` with the key-name rule, `credential/<reference>` with the
  reference rule, or `named/<NAME>` with the name rule of a named secret (1 to 64
  characters of `A-Z`, `0-9` and `_` that starts with `A-Z`, and no name that
  Horizon or the system own), so no id is a path, a command or a longer name than a
  name. The server stores the body as opaque bytes and never parses it.
- **A secret intent carries its value.** An intent of a secret without a body is
  `invalid_intent`, and a body above 16 KiB is refused: the server never asks a
  computer for a value.
- **A clock of a computer is never read.** An intent carries no time. The server
  stamps every change with its own clock.

## Untrusted: the setting that names the server

- **The target is one argument of `ssh`.** `server.ssh_target` is at most 255
  characters, it does not start with `-`, and it holds no space and no control
  character, because `ssh` takes it as one argument. `ssh` still reads the
  person's own SSH configuration for that name, as it does for a browser host.
- **The command is one line that the remote shell reads.** `server.command` is at
  most 200 characters of letters, digits and `._:/=@+-`, with at most one space
  between words, so it starts one program and no second command. A path of the
  person that holds an underscore, such as `/Users/x/my_tools/horizon`, is one
  such command. It is empty by default, and then the link runs
  `horizon --server-link`.
- **Both values belong to the person.** A hand-edited setting runs the `ssh` that
  the person configured for that name. The boundary of this page starts at the
  bytes of the child, not at the setting.

## What a computer accepts from the server

- **A record is checked on the computer too.** A machine row of a pull passes
  `horizon_core::machines::check_record` before it reaches the mirror or the
  sidebar, so a record that the server holds in a state that breaks a rule is
  refused and logged. The server checks the same record before it stores it.
- **A binding is checked on the computer too.** A `binding` row names a machine
  with the machine-name rule and a key with the key-name rule, and a row that
  breaks one is a line of the log and no binding.
- **A secret is checked twice, and written only where a value of that kind
  lives.** A `secret` row of a pull passes `SecretId::parse`, so its id is one
  machine key, one credential reference or one named secret. A machine key body is
  JSON of a key pair with a private key and a public line; a body that is not one
  is refused with a line of the log and the local value stays. A credential body is
  written to the secrets file of this computer under `credential/<reference>`, and a
  reference that no provider binds is refused with a line of the log. A named secret
  body is checked against the value rule of the named secrets and written under the
  id `named/<NAME>`, and a body that breaks the rule is refused.
- **A restored or malicious server can change the machine records, the key
  bindings and the values of the secrets.** That is what the person asked for,
  and it is bounded: an id is never a path and a body of the wrong kind is
  refused. A server cannot make the computer run a command: the only process is
  the `ssh` of the validated setting, and the tmux command of a terminal is
  rebuilt from the record by Horizon's own text.
- **It cannot make a computer write outside its own files.** The mirror, the
  queue and the device name are three files of the machine directory of the
  Horizon home, at 0600 in a 0700 directory, and an object id is never a path. A
  secret value is a row of the secrets file of that home, at 0600, and never a file
  whose name a server chooses. A
  machine key is the three files of `machine-keys/<name>`, and a conflict copy is
  the same three files under `<name>.conflict-<time>`, a name that no key rule
  accepts.
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
- **A secret is sealed at rest.** The bodies of the table `secrets` are sealed
  with XChaCha20-Poly1305, a random 24-byte nonce per write and the id as
  associated data. The key is 32 bytes in `secret.key` in the server home, mode
  0600, made at the first start. It is never part of the database, never sent to a
  computer, and never in a copy of `server.sqlite`, so a copy of that file alone
  holds no readable secret. A person must back up `secret.key` separately.
- **A lost key is a reported loss, not a silent one.** A live secret row that the
  key of the server cannot open is no row of a pull, the server logs the id, and
  its hello says `secrets_lost`. Every computer that still holds a value sends it
  again, so a server whose key file was lost is a server whose values come back
  from the computers that kept them. If the key is lost and no computer holds a
  value, that value is lost.
- **No secret body reaches a file of a computer or a line of the log.** The queue
  of a computer holds the id of a secret and the version that it saw; the mirror
  holds no secret at all; the log row of a secret holds the kind, the id and the
  version; and the `Debug` of a secret body is redacted, so a value is in no
  `tracing` line, no error text and no refusal quote.
- **The server holds no other secret.** It holds no ssh login key of the person,
  no provider CLI login (az, gcloud, hcloud, aws), no browser profile, no session
  cookie, no agent token, no terminal content, no transcript and no setting of the
  person. The one value that a person names for their own tools is a named secret,
  which they asked the server to hold and which its boundary page states. A
  machine record holds no key name: the binding is an object of its own.
- **A refusal quotes a bounded part of a frame.** A value that the server copies
  into the log or into a refusal is at most 64 characters and then three dots, so
  one intent writes about a line of log and not a frame.
- **The link ends with a typed error.** A link that cannot continue stops writing
  before it closes, so the computer reads the error frame and not a reset.
- **The socket is private.** A Unix socket of the server is `0600` in a directory
  of the server user, a file that is not a socket is left alone, and a socket that
  a stopped server left is removed before the bind.
- **The store is private.** The file of the server store is `0600` before SQLite
  opens it, and SQLite gives its `-wal` and `-shm` files the mode of that file, so
  the three are the server user's alone whatever the umask of the process is. A
  file that an older Horizon made readable is made private at the start.
- **The queue file is private.** The queue of a computer is `0600` in a `0700`
  directory, written as a whole through a temporary file and a rename. The mirror
  of the machine list and the device name are written the same way.
- **The child is one program with bounded flags.** The computer runs `ssh -T`
  with `BatchMode=yes`, a connect bound of 5 seconds and keepalives of 5 seconds
  with two tries. It writes the frames to the standard input of that child and
  reads them from its standard output; the standard error of the child goes to
  the log, one bounded line at a time, and is never stored.
- **Every request of the link has a bound.** The server answers a push and a pull
  at once, and one that no answer reached after 20 seconds ends the link. A wait
  stays open at the 50 seconds of the server, so its answer is due after 65
  seconds. A link that reaches its bound is killed, reaped and reported `away`,
  and the next attempt starts on the delays of a lost link. A byte stream that
  stays open and answers nothing is therefore not a link that a computer keeps.
- **The relay of a link ends with its socket.** `horizon --server-link` returns
  as soon as the socket of the server ends or its read or write fails, and its
  entry ends the process there. A server that stops therefore ends the mode of
  every idle computer at once instead of leaving one process per computer with a
  standard input that nothing closes.
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
  computer. Whatever `ssh` lets in may claim any device name, read and write the
  whole machine list, and read every machine key and every credential value: the
  key of the secrets is a file of that same login. A hardware key on that login
  and a list of the computers that may reach it are not part of this slice.
- **A computer that is lost.** Every linked computer holds a copy of every secret.
  A computer that a person loses is therefore a computer that holds every machine
  key and every credential value: remove its ssh login at the server, then replace
  the keys and the credentials, because removing the login does not rotate them.
- **Which value is right.** Two computers that change one secret at the same time
  decide by the version rule, and the loser keeps its value as a local copy or a
  notice. Horizon does not tell the person which of the two values is the correct
  one.
- **A credential value that no provider of a computer binds.** It has no address
  on that computer, so it is refused with a line of the log and the value of the
  server stays unread there. The person binds the reference in the settings.
- **The local copy of a value is in clear text.** A secret body is sealed at the
  server and stored as it is in the secrets file of a computer, at 0600 in a 0700
  home. A person who reads that file, or a backup of it, reads every value.
  [`named-secrets.md`](named-secrets.md) states that boundary.
