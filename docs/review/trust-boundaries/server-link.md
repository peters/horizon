---
applyTo: "crates/horizon-sync/src/**,crates/horizon-sync/Cargo.toml,crates/horizon-core/src/atomic_file.rs,crates/horizon-core/src/config/server.rs,crates/horizon-core/src/settings_sync.rs,crates/horizon-core/src/config_store/**,crates/horizon-ui/src/app/settings_io.rs,crates/horizon-core/src/machines/registry.rs,crates/horizon-core/src/secret/**,crates/horizon-core/src/secret_store/**,crates/horizon-core/src/server_link.rs,crates/horizon-ui/src/app/server_sync.rs,crates/horizon-ui/src/app/block_machine_keys.rs,crates/horizon-ui/src/app/settings/remote_browsers.rs,crates/horizon-ui/src/app/settings/server.rs,packaging/horizon-server/**"
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

Three transports reach the server, and the bytes of all three are the same
frames:

- The **Unix socket** of the server home, and `horizon --server-link`, one child
  of one `ssh` call of a computer: the child relays the bytes of its own standard
  input and output to that socket and reads no file of the person. `ssh` is the
  login: a link of the socket carries no token.
- The **TCP listener**, which `horizon --server --listen <ip>:<port>` opens when a
  person asks for it. A computer that is not logged in sends a `join` with a code
  of an invitation, and a computer that is logged in sends a `hello` with the
  token of its device. The listener is off without `--listen`.

A computer can hold a second link on the same transport: the live link of its
shared terminals. A link becomes a live link with the request `open`, and then
it carries no frame of the store. This page states the transport, the greeting
and the login of that link. [`shared-terminals.md`](shared-terminals.md) states
the boundary of its frames.

Programs touch the boundary. The server is `horizon --server`: it holds the store
and the socket of one server home and starts no other process. The first
computer of a server is `horizon --server-invite`: it sends one request over the
Unix socket of the server home and prints a link. A computer that holds the link
runs it on a worker thread through `crates/horizon-sync/src/supervisor.rs` and
never on the UI thread.

## Untrusted: every byte of the link and every field of an intent

- **The stream is one computer.** The first frame must be a `hello`. A frame
  before it, a second `hello`, a frame that is not one document of the protocol,
  and a frame above 256 KiB end the link with a typed error frame.
- **The version is one number of each side.** The `hello` of a computer of this
  build names version 4. The server of this build answers version 9, and a
  `hello` above 9 ends the link with `protocol_version`. Version 5 changed no
  frame: a server from that version holds an idle wait for 15 seconds. Version
  6 adds the two frames of a live link. Versions 7, 8 and 9 add the facts of a
  screen, the heartbeat and the grid of a shared program; the page of the
  shared terminals names each one.
- **A link carries the store or the shared terminals, not the two.** A link
  whose first request after the greeting is `open` is a live link. A live
  request on another link ends that link with `not_live`. A frame of the store
  on a live link ends it with `live_link`. A computer sends a live frame only to
  a server that answered version 9 or higher.
- **The device name is a claim.** It is 1 to 40 characters of `a-z`, `0-9` and
  `-`, and a name that breaks the rule ends the link. The transport does not prove
  the name: whatever `ssh` let in may claim any device name.
- **An intent names its computer.** An intent whose device differs from the name
  of the link is refused, so no computer spends the counters of another one.
- **A record is checked at the server.** A machine record passes the validators of
  `horizon_core::machines::check_record`: the name rule, the route rule of an SSH
  terminal, the session rule, the provider line and the three facts of a rented
  machine, which are its `size`, its `place` and its `price_per_hour`. A record
  that breaks a rule is `invalid_record` with the code of that rule, and nothing is
  written. The body is one typed record with no field that this version does not
  know, so a server of an older version refuses a record that carries a field that
  it does not know instead of relaying it in part.
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

## Untrusted: the TCP link that a person opens with a code

The TCP listener is the second transport, and it is opt-in. Its trust boundary is
the network that carries it, because Horizon encrypts nothing on it.

- **The address rule is the first check, on both sides.** The listener refuses to
  start on any address that is not loopback, inside `100.64.0.0/10`, or inside
  `fd7a:115c:a1e0::/48`. Those ranges belong to a private overlay network, whose
  own encryption is the encryption of this link. A wildcard address (`0.0.0.0`,
  `::`) and every other address are refused with a typed error that names the
  rule. The computer checks the same rule on every address that the name of a
  join link resolves to, before it sends one byte, and refuses the link with one
  plain sentence.
- **The invariant is that no byte leaves a computer or reaches the server outside
  those ranges.** Nothing in Horizon proves that the network encrypts. A person
  who listens on a loopback address and forwards that port to a public network
  breaks the rule outside Horizon, and Horizon cannot see it.
- **A code is a claim that the person has the server.** A `join` carries one code
  of 12 characters of Crockford base32, which the server keeps as its SHA-256
  only, for 10 minutes, for one use. A wrong code, an expired code and a used code
  answer the same `wrong_code`, after one second, so the answer is no oracle. The
  server deletes every open invitation after 20 wrong codes inside 10 minutes, and
  `join` then answers `join_locked` until a person makes a new invitation. The
  code is the whole proof: a person who reads it over a shoulder can join, and the
  server then gives that computer every machine key and every credential value.
- **A token is the whole login of one computer.** The `join` answers one token of
  32 bytes from the OS random source. The store keeps the SHA-256 of the token and
  never the token, and every later `hello` carries the token and hashes it to the
  row of that device. An unknown hash and a device that differs from the device of
  the token both answer `not_logged_in` and end the link, so a token is one device
  and no other. A token in the frame is a bearer value: the server never writes it
  to its log, its answers and its errors quote no token.
- **A device name is a claim, as it is on the socket.** A `join` whose name is
  not 1 to 40 characters of `a-z`, `0-9` and `-` answers `bad_device`. A name that
  already holds a token answers `name_taken`, so the person removes the old entry
  first and no join takes over the token of another computer.
- **The name of a join is the name of the link.** The joining computer writes that
  name to `machines/server-device` of its own machine directory in the same worker
  job and before it writes the token, and the supervisor that follows presents the
  stored name and no other. A hello whose name differs from the name of the token
  answers `not_logged_in`, so a computer that proposes a second name cannot use the
  token of its join. The name is read-only in the window while the token stands.
- **A row that no hello reached does not stay.** A `join` writes the row of its
  computer and the token together, and the row starts ungreeted. The server drops a
  row that no hello reached within one minute, so a join whose token never reached
  the computer that asked for it leaves no computer that a person must remove by
  hand.
- **The computer keeps the token where its other secrets live.** The token is one
  row of its `secrets.sqlite` under the id `server/token`, at the mode of that
  file. The secret ids of the protocol are never that id: `SecretId::parse`
  refuses it, so no server can send a token and no link sends one, and no token
  reaches a queue file, a mirror file or a line of the log.
- **A removal ends a link and nothing else.** `device_remove` deletes the token row
  of that device and ends its open links with `error{code: "removed"}`. The live
  link of that device is one of those links, and the computers that follow its
  terminals read that it is offline. The server
  keeps the name and the hash of the token of that device for 30 days, so a
  computer that comes back after its removal reads the same answer, and the kept
  hash is the hash of a token that no longer works: it proves the name of a removed
  computer and it is the login of nothing. A join of that name, a later removal and
  the end of the 30 days drop the record. The removed computer writes one sentence,
  deletes its own token row and stops until a person joins it again. It keeps every copy of every machine key and every credential
  value that it already had: the server rotates nothing. A person who loses a
  computer removes it, then makes new machine keys and new credential values. A
  computer of the socket comes back at its next link whatever a removal did,
  because `ssh` is the login of that transport, and a removal proves nothing about
  a login that `ssh` grants.
- **The server still starts no program and opens no other port.** The listener
  answers frames only. It reads no path and runs no command of a frame, and it
  starts no `ssh`.

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
- **`server.address` is one `host:port`.** It is at most 255 characters, the host
  is an IP address or a DNS name of at most 253 characters of letters, digits,
  `.` and `-`, and the port is 1 to 65535. The computer resolves the name and
  checks the address rule above on every address that it gets, so a name of the
  person that points outside the allowed ranges ends the link with a plain
  sentence and sends nothing.
- **A join link is untrusted text that a person pastes.** The parser of
  `crates/horizon-core/src/server_link.rs` accepts exactly
  `horizon://<host>:<port>/join#<CODE>`, with no other scheme, no user part, no
  query, no other path and a code of 12 characters of Crockford base32 in either
  case and with or without dashes. Everything else is one typed refusal with a
  sentence, and nothing of the text reaches a file, a program or a frame.

## Untrusted: the settings of a person

The shared settings of the person are on the server, and a person who can write
the server can change them on every computer of that server. **This is the stated
trust of this feature**: the owner decided that no computer has rules of its own,
so the server holds the rules of all of them.

Some shared sections hold a command or a path of a computer, and a person who can
write the server can therefore name a program that a computer runs:

| Section | What it may hold |
|---|---|
| `presets` | A preset of the panel kind `shell` or of an agent kind holds the `command` line and its `args`, so a preset of the server names a program and its arguments. |
| `workspaces` | A workspace and each terminal of it hold a `cwd`, and a terminal holds a `command` line and its `args`. |
| `browser_host` | The SSH target and the profile of the browser host, which a browser panel uses. |
| `remote_hosts` | The hosts that the remote host section offers. |
| `browser` | The engine program of each browser backend, and the passkey command. |
| `speech` | The recorder line and the transcriber command, which are command lines that Horizon runs while the hotkey is down. |

The bounds that a computer keeps are these:

- **A section is a value of the settings, and nothing more.** It passes the same
  `Config::validate` as a value that the person typed, so the rule of a command
  line, of a path and of a target is the rule that this build already applies. A
  section that breaks a rule is not applied and not written.
- **A command of a section runs only when a person starts it.** A preset runs when
  the person starts a panel of it, the recorder runs while the hotkey is down, and
  a browser host is reached when the person opens a browser panel. No frame of the
  link runs a program of a section by itself.
- **A section of a newer configuration version is not migrated down.** It stays on
  the server, and this computer keeps its own row.

What the trust does not cover: the server cannot be told apart from a person who
edits it. A restored copy of a server, an attacker who reaches the server home, and
a person who is mistaken all change the settings of every computer in the same way,
and Horizon reports a change as a change of a section. The notice of the first link
and the backup file beside the store are the record of the values of one computer
before that link; they are no proof of who wrote a value.

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
- **A setting is checked on the computer, and only a shared section of this build
  is written.** A `setting` row of a pull passes `horizon_core::settings_sync`:
  the body is one section inside the envelope of format 1, the id is one section
  name, the section of an older configuration version is migrated, and the section
  of a newer version is refused with one sentence that says that this computer runs
  an older Horizon. The merged settings pass `Config::validate` before one row of
  the store is written, so a section that breaks a rule of this build leaves the
  row of this computer exactly as it was. A section of a local one, `window`,
  `server` or `version`, is never applied. A section that this build does not know
  is kept as it arrived, and it changes no behaviour of this build.
- **A save of this computer merges the section of the server's body and never the
  envelope.** The three fields of the envelope, `format`, `config_version` and
  `section`, stay outside the section, so no save nests one envelope inside the
  next and the row of the store stays the section that a computer without a server
  writes. Only the top level of the section is merged: a field of a newer Horizon
  survives the save of an older one, and a nested merge decides nothing.
- **A path of the `browser` section is a value of this computer, not a command.**
  The section can name the download folder, the profile folder, a browser and a
  driver of another computer. This computer reads a folder or a program that it
  does not have as if the value were unset, and one line at the level INFO names
  the setting and no path: a shared section never makes this computer run or write
  in a program or a folder of another home.
- **A restored or malicious server can change the machine records, the key
  bindings, the values of the secrets and the shared settings.** That is what the person asked for,
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
  to 64 characters of `a-z`, `0-9`, `-`, `_` and `.`, and it is never a path. The
  id of a setting is 1 to 40 characters of `a-z`, `0-9` and `_`, which is one
  section name and no path.
- **A setting is stored unread and bounded.** The server checks the id, the size of
  one body (64 KiB) and the number of sections (64), and it writes the JSON that
  the computer sent. It reads no field of that JSON, so no value of a setting
  decides what the server does. A refused section is one refusal of one intent, so
  an oversize setting costs a line and never the link.
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
  cookie, no agent token and no transcript. It stores no terminal content. The
  screens, the output and the typed bytes of a shared terminal go through its
  memory only. It holds the
  shared settings of the person, which are the rules of every computer of that
  server; the section above states what they may contain. The one value that a person names for their own tools is a named secret,
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
  stays open at the 15 seconds of the server, so its answer is due after 21
  seconds; a server of an older version holds its 50 seconds, and such a link
  keeps the 65-second bound of the computer. A link that reaches its bound is
  killed, reaped and reported `away`, and the next attempt starts on the delays
  of a lost link. A byte stream that stays open and answers nothing is therefore
  not a link that a computer keeps. These bounds are the bounds of the sync
  link. A live link has no request with a bound, but each side of it sends a
  `heartbeat` while it has no other frame for 10 seconds, and 30 seconds of
  silence ends it with `live_silent`.
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
  server user can open the file, and nothing here prevents that. A live link
  holds one more thread for the frames that wait for it, and one server opens
  at most 32 live links.
- **The content of a shared terminal.** The server process holds each byte of a
  shared terminal that it relays, so a person who controls the server can read
  it. That person can also type in each terminal that a computer shares with
  typing on.
  [`shared-terminals.md`](shared-terminals.md) states that boundary.
- **A live link that went silent.** Each side of a live link ends it when it
  read no frame for 30 seconds, and the server ends its own side with
  `live_silent`. A computer that went away therefore leaves the list of the
  shared terminals in about half a minute, also while a program of this
  computer holds its transport open under the dead link.
- **The content of a body of an internal object.** The `probe` kind is bounded in
  number and in frame size, and its body is JSON that the server does not read.
  No person or agent sees it: a later slice replaces it with the kinds that a UI
  draws, each with its own validators.
- **A clock of the server.** The time of a change is the clock of the server. A
  server whose clock is wrong stamps wrong times, and a reader that compares two
  changes of two servers compares two clocks.
- **Who may reach the server.** On the socket the boundary is the SSH login of the
  server computer. Whatever `ssh` lets in may claim any device name, read and write
  the whole machine list, and read every machine key and every credential value:
  the key of the secrets is a file of that same login. On the TCP listener the
  boundary is a token that one `join` with a code handed out, and the code is a
  bearer value of 10 minutes. A hardware key on the login, a rotation of the
  tokens and a list of the computers that may reach the server are not part of
  this slice.
- **The encryption of the TCP link.** Horizon adds none. The link is plain frames
  on a network that the address rule trusts, so the overlay must encrypt and
  authenticate every byte. A person who moves the listener to another address
  breaks that rule, and a person who reads the code reads the token that follows.
- **What a removal does not do.** It deletes one token and ends the links that are
  open. The removed computer keeps every machine key and every credential value
  that it already had, and a lost computer is still a computer that holds all of
  them. Removal therefore needs new keys and new values to be a real repair, and
  the person makes both.
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
