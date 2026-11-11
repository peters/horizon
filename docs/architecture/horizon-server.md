# The Horizon server and its link

This page gives the rules of the server that holds the state of more than one
computer, the file that it keeps, the frames of its link, and what happens when
it is away. The crate is `crates/horizon-sync`. The trust boundary of the same
code is [`docs/review/trust-boundaries/server-link.md`](../review/trust-boundaries/server-link.md).

A computer talks to the server through the stdio of one child process,
`ssh <host> horizon --server-link`. The child carries bytes only, so the protocol
below runs over any pair of byte streams. The store and the frames reached the
server through `horizon_sync::serve` before the two modes existed, and a test can
still give them two in-memory sockets.

## The two modes of the binary

The server and the link are the same `horizon` binary with one flag each, and
both modes run before Horizon prepares its home:

| Mode | What it does |
|---|---|
| `horizon --server` | Holds one server home: `server.sqlite` at mode 0600, with its `-wal` and `-shm` files at the same mode, `server.sock` at mode 0600 in a directory at 0700, and `server.lock`. It answers every link until `SIGTERM` or `SIGINT` sets its stop flag, and then it stops accepting, ends the links that are open, removes the socket and closes the store. |
| `horizon --server-link` | Relays its own standard input and output to the socket of that home, for one `ssh` child. It starts no server and creates no file. A home without a server answers one typed line and exit code 3. The mode ends as soon as the socket ends, which is the case when the server stops, and the entry of the mode ends the process at once: its standard input stays open, so a wait for it would leave one child of an idle computer alive while the server is away. |

The server home is `HORIZON_SERVER_HOME`, or `~/.horizon-server`. It is a
directory of its own, so neither mode reads or writes `~/.horizon` of the person:
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

```yaml
server:
  ssh_target: server-computer
  command: /opt/horizon --server-link
```

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

One worker thread of a computer owns one `ssh` child at a time. It starts
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

The thread publishes one state for the UI: `off`, `connecting`, `in_step`,
`behind`, or `away` with the number of intents that wait. The sidebar shows it,
and `machines` reports its code.

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
4. The key of a machine is a local binding of `machines/keys.json` and is never
   sent. A record that only arrived from the server is marked in the mirror, so a
   click on it opens nothing and the section says `This computer has no key for
   <name> yet`.

### The first link of a computer that has a list

A computer that kept its machine list in `machines/registry.json` before it
followed a server keeps that list at its first link. The first pull of the server
comes before any push, so the computer knows which records the server has:

- A local record whose name the server does not have is pushed.
- A name that both have keeps the record of the server, and the local record is
  dropped.

The mirror file records that this comparison happened, so a restart does not push
the same list again. Until that comparison, the file store of the computer stays
the source of its records.

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
   intent carries no time, so no clock of a computer is read.

The machine list is **last write wins**: two computers that add and remove
machines decide by the commit order of the server, and the version rule is not
read for it. The version rule holds for the objects that a later slice adds, such
as cards and waits, and it holds today for an internal `probe` object that a
person never sees.

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
| `objects` | `kind`, `id`, `body_json`, `version`, `deleted`, primary key `(kind, id)` | The state that a computer pulls. A removed object is a row with `deleted = 1`. |
| `devices` | `name`, `first_seen`, `last_seen`, `last_intent` | One row for each computer that sent an intent, with the highest counter that it spent. |

The schema version is one integer. A file of a newer Horizon is refused, never
migrated down. Only version 1 exists, and `MIGRATIONS` in
`crates/horizon-sync/src/store/schema.rs` is where a change from one version to
the next goes.

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
{"hello":{"protocol":1,"device":"mbp-7c1","store":"6f1c…"}}
{"hello_ok":{"protocol":1,"store":"6f1c…","now_millis":1760000000000}}
{"push":{"intents":[{"device":"mbp-7c1","id":42,"seen":118,"op":{"machine_put":{"record":{"name":"build-1","host":"build-1.example","user":"peters","port":2222,"session":"work","provider":"Hetzner, hel1","added_at_millis":1760000000000}}}}]}}
{"pushed":{"results":[{"id":42,"outcome":{"outcome":"applied","index":119}}],"next_expected":43}}
{"pull":{"since":118,"tip":"9f2c1d…"}}
{"changes":{"index":121,"reset":false,"rows":[{"kind":"machine","id":"build-1","body":{"name":"build-1"},"version":119,"deleted":false}]}}
{"wait":{"since":121}}
{"error":{"code":"protocol_version","message":"the computer speaks protocol 2, and this server speaks protocol 1"}}
```

| Frame | From | What it says |
|---|---|---|
| `hello` | computer | The protocol version, the device name, and the store id that the computer saw last. |
| `hello_ok` | server | The version of the server, its store id, and its time. |
| `push` | computer | Up to 256 intents. |
| `pushed` | server | One result for each intent, and the next counter that the server expects. |
| `pull` | computer | The index and the tip of the computer. |
| `changes` | server | The index of the server, whether the computer must reset, and the rows. |
| `wait` | computer | One pull that stays open until a change after the index, or 50 seconds. |
| `error` | server | A typed error of the link: `frame_too_large`, `bad_frame`, `protocol_version`, `bad_device`, `not_hello`, `too_many_intents`, `too_many_waits` or `store_failed`. |

A refusal of one intent is not a link error. It is the `outcome` of that intent:
`changed_since_seen`, `already_answered`, `too_many`, `invalid_record`,
`unknown_kind` or `invalid_intent`. A broken frame, an oversize frame, a higher
protocol version, a device name that breaks its rule, a second hello and a push of
more than 256 intents end the link. A wait above the maximum and a store that
could not answer keep the link.

The limits are: 64 machines, 64 internal probe objects, 256 intents in one push,
256 KiB in one frame and 32 open waits for each link.

## What the server never holds

The server holds the machine list and, in later slices, the machine public lines,
cards, clicks and waits. It holds no private key, no token, no terminal content,
no transcript, no browser profile, no session cookie and no settings of the
person. A machine record holds no key name: the binding of a key to a machine is
local to a computer.

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
