# The Horizon server and its link

This page gives the rules of the server that holds the state of more than one
computer, the files that it keeps, the frames of its link, and what happens when
it is away. The crate is `crates/horizon-sync`. The trust boundary of the same
code is [`docs/review/trust-boundaries/server-link.md`](../review/trust-boundaries/server-link.md).

A computer talks to the server through the stdio of one child process,
`ssh <host> horizon --server-link`. The child carries bytes only, so the protocol
below runs over any pair of byte streams. The store and the frames reached the
server through `horizon_sync::serve` before the two modes existed, and a test can
still give them two in-memory sockets.

## The three modes of the binary

The server, the link and the first invitation are the same `horizon` binary with
one flag each, and all three modes run before Horizon prepares its home:

| Mode | What it does |
|---|---|
| `horizon --server [--listen <ip>:<port>] [--name <dns-name>]` | Holds one server home: `server.sqlite` at mode 0600, with its `-wal` and `-shm` files at the same mode, `secret.key` at mode 0600, `server.sock` at mode 0600 in a directory at 0700, and `server.lock`. It answers every link until `SIGTERM` or `SIGINT` sets its stop flag, and then it stops accepting, ends the links that are open, removes the socket and closes the store. With `--listen` it also opens the TCP listener of [the listener](#the-listener-and-the-token), and `--name` gives the host that its links hold. |
| `horizon --server-invite` | Asks the server of this home for one invitation over the socket, and prints the link, the code, the minutes that it lives and the QR code of the link as blocks of text. It starts no server and writes no file, and it ends with code 0. |
| `horizon --server-link` | Relays its own standard input and output to the socket of that home, for one `ssh` child. It starts no server and creates no file. A home without a server answers one typed line and exit code 3. The mode ends as soon as the socket ends, which is the case when the server stops, and the entry of the mode ends the process at once: its standard input stays open, so a wait for it would leave one child of an idle computer alive while the server is away. |

The server home is `HORIZON_SERVER_HOME`, or `~/.horizon-server`. It is a
directory of its own, so no mode reads or writes `~/.horizon` of the person:
they read no setting, open no window and install no agent integration. A second
server of the same home finds the lock held and refuses with one typed line
instead of removing the socket of the first one. The module comment of
`crates/horizon-sync/src/host.rs` states the same rules.

A stop leaves its files in the home on purpose. `server.lock` stays, because the
lock is the open file of the running server and not the name of the file: a file
that no process holds blocks nothing. A killed server also leaves `server.sqlite`
with its `-wal` and `-shm` files, and SQLite recovers them at the next start. A
start removes a socket file that no process listens on, takes the free lock and
opens the store, so a home of a killed server starts again without a hand edit.

## The setting of a computer

The settings entry `server` names the computer that runs the server:

```json
{
  "server": {
    "ssh_target": "server-computer",
    "command": "/opt/horizon --server-link",
    "address": "server-computer:7000"
  }
}
```

`address` is the `host:port` of the TCP listener of a server. A successful join
writes it, and the link of that computer then runs over TCP. A computer with an
address and no token uses `ssh`, so the `ssh` link stays as a second transport,
and a person removes the address to go back to it.

`ssh_target` follows the rule of the browser host target: one argument of `ssh`,
no leading dash, no space and no control character. `command` is optional and is
`horizon --server-link` without it; it holds at most 200 characters of letters,
digits and `._:/=@+-`, with single spaces, so the remote shell reads it as one
program. The underscore is one of these characters, so a path such as
`/Users/x/my_tools/horizon` is one command. A person writes the whole path when
the `PATH` of a non-interactive `ssh` does not hold the binary. Without
`ssh_target` the entry names no server, and the machine list of the computer is
one file as before.

## The supervisor of the link

One worker thread of a computer owns one transport at a time: one `ssh` child, or
one socket to the listener of its server when its settings name an address and
its secrets file holds the token of its join. The rest of this section names the
`ssh` child, and the socket runs the same frames with the same bounds.

It starts
`ssh -T -o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=5
-o ServerAliveCountMax=2 <target> <command>` with the flags that the browser
forward already builds, and it speaks the frames over the standard input and
output of that child. The standard error of the child goes to the log, one
bounded line at a time.

The thread sends the hello, pulls once, then pushes what the queue holds and
waits with `wait`. A push that is ready leaves while a wait is open, so a change
of this computer does not wait for the answer of the idle wait: the server
answers the two frames separately. A local change wakes the thread at once
instead of at the next answer. When the child ends, the thread starts it again
on the delays of a lost machine terminal, 3, 6, 12 and 24 seconds and then every
30 seconds, and a link that was up for 10 seconds starts the delays again.

Every request has a bound, because a link over `ssh` does not always end: a
server that dies can leave the socket of the child open while the child stays
alive. A push and a pull are due at once, and the answer of either one after 20
seconds is a link that is lost. A wait stays open at the bound of the server,
which is 50 seconds, so its answer is due after 65 seconds: the bound of the
server and a margin of 15 seconds. A link that reaches a bound is ended and
started again at once: the supervisor kills and reaps the child, reports `away`
and starts the delay of the next attempt. The bounds are the constants of
`crates/horizon-sync/src/supervisor.rs`, and a test gives its link shorter ones.

A question of the window travels on the same link, one at a time: `invite`,
`devices` and `device_remove` reach the server over `ssh` as well as over the
listener, and the answer comes back on a channel that the frame polls. A link
that the server ends with `removed` or `not_logged_in` is the one end that does
not retry: the supervisor forgets the token row of this computer and stops until
a person joins it again.

The thread publishes one state for the UI: `off`, `connecting`, `in_step`,
`behind`, `away` with the number of intents that wait, `removed`, or
`not_logged_in`. The sidebar shows it,
and `machines` reports its code. `connecting` belongs to the first attempt of a
link alone: an attempt that follows a link that failed keeps `away` with the
number of intents that wait, until a hello answers. A server that does not answer
therefore reads `Server away`, and not `Server: connecting`, while the link waits
for it.

## The machine list of a computer

With a server in the settings, the machine list of a computer is the mirror of
the server and the queue of that computer in one view:

1. A record that an agent or the person adds or removes changes the view at
   once, so the sidebar changes at once.
2. The same call writes one intent to `machines/server-queue.json`. The record
   shows `pending` until the server commits that intent.
3. A change that arrives from the server replaces the row of the mirror, which
   is `machines/server-mirror.json`. Both files are private, and both survive a
   restart.
4. The key of a machine is a binding of `machines/keys.json`. The binding follows
   the server too, and the key value with it, so a machine that another computer
   added opens here as soon as the key arrived. A record whose key this computer
   does not hold opens nothing, and the section says `The key of <name> was
   deleted. Ask an agent to make a new key and install it.` while `machines`
   answers `key_missing: true` for it. A record that arrived from the server with
   no binding here says `This computer has no key for <name> yet` instead, because
   there is no key name to report.

### The first link of a computer that has a list, keys and values

A computer that kept its machine list in `machines/registry.json`, its key
bindings in `machines/keys.json`, its keys in `machine-keys/` and its credential
values in the secrets file of its home before it followed a server keeps all of them
at its first link. The first pull of the server comes before any push, so the computer knows
which records, bindings and secrets the server has:

- A local record, binding or secret whose name the server does not have is
  pushed, so the server holds the union.
- A name that both have keeps the value of the server. A local machine key that
  differs is kept as `<name>.conflict-<time>`, and a local credential value that
  differs is replaced with one notice for the person. A key whose private key and
  public line are the same is not a difference, whatever its known-hosts lines
  are.

The mirror file records that this comparison happened, so a restart does not push
the same values again. A server that returned after its `secret.key` was lost
asks for the values again, whatever the mirror says.

## The settings of every computer

The owner decided that every setting of Horizon is on the server too, so that no
computer has rules of its own. A change of a setting on one computer is on the
other computers within a few seconds, and there is no per-computer override.

The settings store keeps one row for each configuration section. One row is one
object of the kind `setting`: the id is the name of the section and the body is
the JSON of that section inside an envelope. The table of the sections and their
decisions is in
[the settings store page](settings-store.md#the-settings-that-every-computer-shares).
That page names the three sections that stay with one computer and the reason for
each one.

```json
{"format":1,"config_version":12,"section":{"theme":"light"}}
```

The envelope says which configuration version wrote the section. An older section
is migrated with the migrations of the computer that receives it, and a section of
a version above the newest version of that computer stays on the server: the
computer keeps its own row and the window says that it runs an older Horizon. The
server reads neither value. It checks the id (`[a-z0-9_]{1,40}`), the size of the
body and the number of sections, and it stores the JSON.

- **The store of a computer is the first read.** A start reads
  `settings.sqlite`, so a start never waits for the server. The link compares the
  sections when it is in step, and a section of the server replaces the local row
  through the write path of the settings store, on the worker thread of the link.
- **A change of the link applies the sections.** The window reads the store again
  through the watch of the settings file that it already has, and applies the new
  values through the path that the Settings window uses. A section that the person
  edits at that moment is not replaced and is not dropped: the footer of the
  Settings window offers the value of the other computer, and every other section
  applies at once. A section whose content is the content that this computer holds
  is not written: the computer takes the version of the server and keeps its row.
- **A local save queues one intent for each section that changed.** The settings
  worker writes the store, and then it writes one `setting` intent for each shared
  section whose value differs from the store that it wrote. The intent carries the
  version that this computer saw, so the version rule of the next list decides it.
- **A push that loses decides nothing by itself.** A push that the version rule
  refuses moves the mark of the computer back, and the row of the version that
  decided it says whether the value of this computer differs from that row. A row
  with another content replaces the row of this computer, and one line names the
  sections whose content differed. A row with the same content replaces nothing:
  the computer takes its version, writes no row, keeps no backup and reads no line,
  so nothing of that computer was lost.
- **The first link compares once.** For each shared section, a server that has no
  row takes the value of this computer, and a server that has a row decides it: the
  value of the server wins, and the value of this computer is kept one time in
  `settings.before-server.<time>.json` beside the store, at mode 0600, with one
  notice for the person. The mirror file of the settings records that the
  comparison happened, so a restart does not repeat it.
- **A section that fails a rule of the computer is not applied.** The merged
  configuration passes `Config::validate` before one row is written, so a server
  can never make a computer run a setting that its own settings could not hold. A
  refused section stays on the server, the computer keeps its own row, and the
  window shows one line about it.
- **No setting is lost.** A section that this build does not know is shared and
  kept, and a field of a known section that this build does not know survives the
  save of an older computer, because the body of a push is merged with the fields
  of the server's body.

`window`, `server` and `version` stay with one computer. Their values say where
the window of this computer stands, how it reaches its server, and which
configuration version its store holds, so no other computer can decide them.

## The listener and the token

`ssh` is the first transport of a server, and it stays the rule for a computer
that is trusted by its login. A person who wants a second computer to join
without an SSH key uses the second transport, which the owner asked for: a TCP
listener that a person turns on, one code that a logged-in computer shows as a QR
code, and one token for each computer after that.

- **Opt-in and bounded.** The listener exists only with `--listen`. It refuses to
  bind any address that is not loopback, inside `100.64.0.0/10`, or inside
  `fd7a:115c:a1e0::/48`, because those are the addresses of a private overlay
  network. Horizon encrypts nothing of its own on this link: the overlay does. A
  wildcard address and every other address are one typed error at start, and a
  computer refuses a link whose name resolves outside the same ranges before it
  sends one byte.
- **One invitation is one code.** A logged-in computer sends `invite` and gets
  `invited { code, link, expires_millis, now_millis }`. The code is 12 characters
  of Crockford base32 in three groups of four, which leaves out `I`, `L`, `O` and
  `U`, so no character reads as another one and no group reads as a word. The
  server keeps the SHA-256 of the code and never the code, for ten minutes and one
  use, and it keeps at most four open invitations: a fifth replaces the oldest.
- **A wrong code is slow and quiet.** A wrong code, an expired code and a used
  code answer the same `wrong_code`, after one second, so the answer is no
  oracle. Twenty wrong codes inside ten minutes delete every open invitation, and
  `join` then answers `join_locked` until a person makes a new invitation. The
  clock and the sleep of the server are values of `crates/horizon-sync/src/clock.rs`,
  so a test sees both without waiting.
- **The token is the login.** `join { code, device }` answers
  `joined { device, token }` and ends the link; the computer then connects again
  with `hello { ..., token }`. The token is 32 bytes from the OS random source as
  base64url text, and the server keeps its SHA-256 in the row of that device. An
  unknown hash, and a device that differs from the device of the token, answer
  `not_logged_in` and end the link. The computer keeps the token as the row
  `server/token` of its `secrets.sqlite`, outside the secret ids of the protocol,
  so no server can send a token and no link sends one.
- **The link of an invitation names the listener.** `--name` gives the host of
  every link that the listener hands out; without it the host is the address. The
  parser of a link and its rules live in
  `crates/horizon-core/src/server_link.rs`, which is one place for the untrusted
  text that a person pastes.
- **The list of the computers is one frame.** `devices` answers
  `device_list { rows, now_millis }`, one row for each computer with its name,
  the time that it joined, the time that the server saw it last, and the way that
  it logs in: `ssh` or `token`. The row of the computer that asked carries
  `caller: true`. The console of the server computer itself is one row of this
  list, under the name `server` and the login `ssh`, because
  `horizon --server-invite` claimed it. The window shows that row with the words
  "this server" and without a removal: a removal of that row ends the link of the
  server's own console and reads as if it removed the server, while the server
  keeps running.
- **A removal ends the links of that computer.** `device_remove { device }`
  deletes the row and the token of that computer and ends its open links with
  `error { code: "removed" }`, after the error frame reached them. The server keeps
  the name and the hash of that token in its `removed` table for 30 days, pruned at
  the next removal or join, so a computer that greets the server after its removal
  reads `removed` instead of `not_logged_in`: one act of a person has one answer. A
  join of that name clears the record. The removed computer writes
  `This computer was removed from the server.`, deletes its own token row and stops
  until a person joins it again. It keeps every copy of every machine key and every
  credential value that it already had, because the server rotates nothing: a
  person who loses a computer makes new keys and new values.

The frame of the hello carries the token only on the listener. A link of the
socket sends no token, because `ssh` let its computer in, and the server takes
the device name of that hello as the claim that it always was.

## The rules

1. **The server is the only writer.** A computer never opens the server file. It
   sends an *intent*, and the server decides it.
2. **One writer task.** One thread owns the write connection. Each push is one
   short `BEGIN IMMEDIATE` transaction in WAL mode with `synchronous=FULL`, and it
   holds no network call. Readers use their own read connections, so a reader
   never blocks the writer and the writer never waits for a reader.
3. **The log index has no gaps.** Each decided intent takes the next index inside
   the transaction of its push. The state of a computer is the last index that it
   has.
4. **An intent is named.** An intent carries the device name and a counter of that
   computer. The server keeps the identifiers that it decided. A repeated intent
   answers its first result again and changes nothing.
5. **A version decides.** An intent on a versioned object carries the version that
   the computer saw. The first intent on that version wins; a later one gets a
   typed refusal. A refusal writes no object and no version.
6. **A computer keeps its queue.** The unsent intents are one local file, written
   as a whole, and they are sent in order with the same identifiers. One push is in
   flight at a time.
7. **A restore tells the computers to reset.** A pull carries the index that the
   computer holds and the `tip` that names the server log at that index. A tip that
   does not match answers `reset: true` with every object, and the mirror of the
   computer equals the server afterwards.
8. **The server stamps the time.** Every change carries the time of the server. An
   intent carries no time, so no clock of a computer is read. The time that a
   machine record says it was added is the time of the first store of that name
   on the server, and a later change of the same machine keeps it. A computer
   that sends its own time changes nothing.

The machine list is **last write wins**: two computers that add and remove
machines decide by the commit order of the server, and the version rule is not
read for it. The version rule holds for a section of the settings, for the objects
that a later slice adds, such as cards and waits, and for an internal `probe`
object that a person never sees.

## The file

One SQLite file, at mode 0600 together with the `-wal` and `-shm` files that
SQLite keeps beside it. Its path is the choice of the caller; this slice does not
name one, and the server mode puts it in the server home. The file is made
private before SQLite opens it, because a file that SQLite creates itself takes
the mode of the umask of the process, and SQLite copies the mode of the database
file to the two files that it makes. `PRAGMA journal_mode=WAL`,
`synchronous=FULL`, `busy_timeout=5000`.

| Table | Columns | What it holds |
|---|---|---|
| `meta` | `key`, `value` | The store id, random at creation, and the schema version. |
| `log` | `idx`, `at_millis`, `device`, `intent`, `kind`, `object_id`, `result_json` | One row for each decided intent, with its result. A refused intent has a row and no object. |
| `applied` | `device`, `intent`, `idx`, unique on `(device, intent)` | The identifiers that the server decided, and the log row of each. |
| `objects` | `kind`, `id`, `body_json`, `version`, `deleted`, primary key `(kind, id)` | The state that a computer pulls: the `machine` rows, the `binding` rows, the `setting` rows and the internal `probe` rows. A removed object is a row with `deleted = 1`. |
| `secrets` | `id`, `nonce`, `sealed`, `version`, `deleted`, primary key `id` | One secret: a sealed body, and the log index of its last change. The body is never in `objects`, because that table holds JSON that a person's server copy would show. |
| `devices` | `name`, `first_seen`, `last_seen`, `last_intent` | One row for each computer that sent an intent, with the highest counter that it spent. |
| `removed` | `token_hash`, `name`, `removed_millis` | The name of a computer that a person removed and the hash of the token that it presented, for 30 days. The hash is the login of nothing: it answers `removed` to a greeting of that computer and it is pruned at the next removal or join. |

The schema version is one integer. A file of a newer Horizon is refused, never
migrated down. Version 2 adds the `secrets` table, version 3 adds the login of a
computer, version 4 adds the greeting of a join, version 5 keeps the name of a
removed computer, and version 6 adds the index that the bound of a kind reads.
`MIGRATIONS` in `crates/horizon-sync/src/store/schema.rs` is where a change from
one version to the next goes.

### The key of the secrets

Every body of `secrets` is sealed with XChaCha20-Poly1305, a random 24-byte nonce
per write and the id of the secret as associated data. The key is 32 bytes in
`secret.key` in the server home, at mode 0600. The server makes it at the first
start and makes a file that a copy left readable private again.

The key is **never part of `server.sqlite`** and the server never sends it to a
computer. A copy of the database alone therefore holds no readable secret, and a
person must back up `secret.key` separately. If the key is lost, the secrets are
lost: a live row that this key cannot open is reported as no row at all, the
server logs the id, and every computer that still holds the value sends it again.
The hello of a link says `secrets_lost` in that case, so the computers send their
own values even when a pull would have shown them nothing.

### The tip

```text
tip = lower hex of the first 16 bytes of
      SHA-256("horizon-sync-tip-v1\n" + store id + "\n" + decimal index)
```

The store id makes two server files different for the same index. The index makes
an older copy of one file different from a newer one. A tip is not a secret: it
only says whether two sides talk about the same log row. `horizon_sync::tip`
computes it on both sides.

## The frames

A frame is a 4-byte big-endian length and then one JSON document. The maximum is
256 KiB, and a longer frame is a typed error that ends the link. The first frame
is `hello`; after that a computer pushes, pulls and waits. Each example below is
one frame.

```json
{"hello":{"protocol":4,"device":"mbp-7c1","store":"6f1c…"}}
{"hello_ok":{"protocol":4,"store":"6f1c…","now_millis":1760000000000}}
{"join":{"code":"ABCD-EFGH-JKMN","device":"laptop-b"}}
{"joined":{"device":"laptop-b","token":"n7Qd…"}}
{"hello":{"protocol":4,"device":"laptop-b","token":"n7Qd…"}}
{"invite":{}}
{"invited":{"code":"ABCD-EFGH-JKMN","link":"horizon://server-computer:7000/join#ABCD-EFGH-JKMN","expires_millis":600000,"now_millis":1760000000000}}
{"devices":{}}
{"device_list":{"rows":[{"name":"laptop-a","created_millis":1760000000000,"last_seen_millis":1760000000000,"login":"ssh","caller":false}],"now_millis":1760000000000}}
{"device_remove":{"device":"laptop-b"}}
{"device_removed":{"device":"laptop-b","removed":true}}
{"push":{"intents":[{"device":"mbp-7c1","id":42,"seen":118,"op":{"machine_put":{"record":{"name":"build-1","host":"build-1.example","user":"peters","port":2222,"session":"work","provider":"Hetzner, hel1","added_at_millis":1760000000000}}}}]}}
{"pushed":{"results":[{"id":42,"outcome":{"outcome":"applied","index":119}}],"next_expected":43}}
{"pull":{"since":118,"tip":"9f2c1d…"}}
{"changes":{"index":121,"reset":false,"rows":[{"kind":"machine","id":"build-1","body":{"name":"build-1"},"version":119,"deleted":false}]}}
{"wait":{"since":121}}
{"error":{"code":"protocol_version","message":"the computer speaks protocol 2, and this server speaks protocol 1"}}
```

| Frame | From | What it says |
|---|---|---|
| `hello` | computer | The protocol version, the device name, the store id that the computer saw last, and the token of this computer on a link of the listener. |
| `hello_ok` | server | The version of the server, its store id, its time, and whether it can read the secrets that it holds. |
| `push` | computer | Up to 256 intents. |
| `pushed` | server | One result for each intent, and the next counter that the server expects. |
| `pull` | computer | The index and the tip of the computer. |
| `changes` (row) | server | One machine, one key binding, or one secret: `{"kind":"secret","id":"machine-key/build-1","body":"<the value>","version":9,"deleted":false}`. |
| `changes` | server | The index of the server, whether the computer must reset, and the rows. |
| `wait` | computer | One pull that stays open until a change after the index, or 50 seconds. |
| `join` | computer | One code of an invitation and the name that this computer takes. |
| `joined` | server | The name and the token of that computer. The link ends after this frame. |
| `invite` | computer | A request for one invitation. |
| `invited` | server | The code, the link that holds it, the milliseconds that it lives, and the time of the server. |
| `devices` | computer | A request for the list of the computers. |
| `device_list` | server | The rows of the computers, and the time of the server. |
| `device_remove` | computer | The name of the computer to remove. |
| `device_removed` | server | That name, and whether the server held it. |
| `error` | server | A typed error of the link: `frame_too_large`, `bad_frame`, `protocol_version`, `bad_device`, `not_hello`, `too_many_intents`, `too_many_waits`, `store_failed`, `no_listener`, `wrong_code`, `join_locked`, `name_taken`, `not_logged_in` or `removed`. |

A refusal of one intent is not a link error. It is the `outcome` of that intent:
`changed_since_seen`, `already_answered`, `too_many`, `invalid_record`,
`unknown_kind` or `invalid_intent`. A broken frame, an oversize frame, a higher
protocol version, a device name that breaks its rule, a second hello and a push of
more than 256 intents end the link. A wait above the maximum and a store that
could not answer keep the link.

The limits are: 64 machines, 64 settings, 64 internal probe objects, 256 secrets,
16 KiB of one secret body, 64 KiB of one setting, 256 intents and 8 secrets in one
push, 256 KiB in one frame and 32 open waits for each link. The protocol version
is 4: version 1 has no secret row and no `secrets_lost`, version 2 has no `join`,
no token and no invitation, and version 3 has no `setting` object.

## The secrets, and what the server holds

The person decided that a second computer must work at once: it names the server,
and it can log in to every machine and use every provider credential of the
person. The server therefore holds, beside the machine list:

- **One machine key per name**, as one secret of the id `machine-key/<name>`. The
  body holds the private key, its public line and the known-hosts lines of that
  key, so a login is the same on every computer.
- **The key binding of each machine**, as one object of the kind `binding` with the
  machine as its id. A machine record holds no key name, because a record is
  checked by the same code on both sides and the binding is the value that the
  server decides.
- **Every credential value that Horizon keeps in the secrets file of a computer**,
  as one secret of the id `credential/<reference>` of the remote browser
  configuration.
- **Every named secret of the person**, as one secret of the id `named/<NAME>`. The
  body is the value that the agent panels of a computer read as the environment
  variable `NAME`, so a second computer gives its agent panels the same variable
  without a step. The name is 1 to 64 characters of `A-Z`, `0-9` and `_` that
  starts with `A-Z`, it is not a name that Horizon or the system own, and the value
  is 1 to 16 KiB of text with no line break. See
  [the boundary of the named secrets](../review/trust-boundaries/named-secrets.md).

A computer writes such a value where its own code already reads it: a machine key
into `machine-keys/<name>` with the modes of a private key, a credential value into
the secrets file of its Horizon home under `credential/<reference>`, and a named
secret into the same file under `named/<NAME>`. All then work without the server
afterwards, and a named secret is in the environment of the next agent panel that
the computer starts. The settings of that computer list the name and never the
value.

**The local copy is in clear text.** One file, `<Horizon home>/secrets.sqlite` at
`0600` in a `0700` home, holds every value of the computer, and its file mode is
the whole protection. The server seals its own copy with `secret.key`, but that key
is a file of the same server home, so the sealing protects a copy of
`server.sqlite` alone. See
[the boundary of the named secrets](../review/trust-boundaries/named-secrets.md)
for what protects a value and what does not.

**One secret is one value, and the first write on a version wins.** A computer
that loses keeps its local value: a machine key as `<name>.conflict-<time>`,
which is not a key name and is never used by an agent, and a credential value as
one notice for the person, because the secrets file holds one value per reference.
The queue of a computer holds the id of a secret and never its body.

A value that the server holds the same way is no conflict. The private key and the
public line of a machine key decide that comparison, and the known-hosts lines of
the key do not: each computer grows them by meeting the host by itself. A
computer that pushes the same key again therefore reads no notice and writes no
copy, and a credential value that both computers hold the same way reads none
either. The value of the server that a refusal names comes into the next answer
of changes, even when the mark of the computer already passed it, so the
comparison is never against a value that the computer assumes.

### What never goes to the server

The server holds no ssh login key of the person, no provider CLI login (az, gcloud,
hcloud, aws), no browser profile, no session cookie, no agent token, no terminal
content, no transcript and no setting that stays with one computer. The shared
settings are on the server on purpose: they are the rules of every computer of the
person, and some of them name a command or a path. Their boundary is stated in
[the boundary of the link](../review/trust-boundaries/server-link.md#untrusted-the-settings-of-a-person). A named secret is the one value
that the person names for their own tools: they ask the server to hold it, and it is
sealed like the other two kinds. `secret.key` itself never leaves the server home.
The push path accepts three ids and no other: a path, a command, a name that is not
one key name, a reference that is not one reference and a name that is not one name
of a named secret are typed refusals, so no other value can arrive.

## When the server is away

A computer keeps working. An intent is written to its queue file before the
computer tries to send it, and the state of the link says `away` with the number
of intents that wait. The queue keeps its order and its counters across a restart
of Horizon. When the link returns, the computer sends the same intents with the
same identifiers and pulls the changes. There is no merge: the commit order of
the server decides, and the version rule refuses an answer that is too late.

A computer that is idle learns that the server is gone at the bound of its wait,
and not later, because a wait that nothing answers is the sign of a link that is
over. Its link then reports `away`, ends the child and starts the delays of a
lost link. A server that returns in that time brings the computer into step with
the changes that the other computers made while it was away, so its mirror and
the server hold the same rows again.

## A continuous copy of the file

The file is one SQLite database, so a tool that copies a SQLite write-ahead log
continuously gives a server that can be restored. [Litestream](https://litestream.io)
is one such tool, and this repository does not depend on it: the following
configuration is a sample of a copy to object storage, and it is not code of
Horizon. The database path is the file that the server opened, and the bucket
holds the generations of it. The server runs with `synchronous=FULL`, so a
committed change is on the disk before the answer leaves. Run one replicating
process for one database. Stop the server before the first start of the
replicator, and stop the replicator before a restore.

```yaml
dbs:
  - path: /home/<user>/.horizon-server/server.sqlite
    replicas:
      - type: s3
        bucket: <bucket>
        path: horizon-server/<server-name>
```

A restore stops the server, writes the newest generation over the file, and
starts the server again. Every computer whose index is above the restored index,
or whose tip names another log, then gets `reset: true` with the whole state of
the restored server.
